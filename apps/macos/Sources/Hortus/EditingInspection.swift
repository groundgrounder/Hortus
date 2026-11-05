import AppKit
import ApplicationServices
import Carbon
import HortusCore

/// 真实窗口验收；仅在显式命令行参数下触发，不给日常运行增加轮询。
@MainActor
enum EditingInspection {
    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        guard let document = workspace.current?.document, let path = workspace.selected else { throw CocoaError(.coderValueNotFound) }
        let mode = arguments.first(where: { $0.hasPrefix("--edit-mode=") }).map { String($0.dropFirst("--edit-mode=".count)) } ?? "lifecycle"
        // 真实编辑检查只走 SCIM ITABC 的纯 keycode 事件；每个阶段等待外部读盘 ack。
        if mode == "composition-focus-loss" { return try await compositionFocusLoss(workspace: workspace, arguments: arguments) }
        if mode == "composition-tab-switch" { return try await compositionDocumentSwitch(workspace: workspace, arguments: arguments, route: .tab) }
        if mode == "composition-search-switch" { return try await compositionDocumentSwitch(workspace: workspace, arguments: arguments, route: .search) }
        if mode == "undo-redo" { return try await undoRedo(workspace: workspace, arguments: arguments) }
        if mode == "undo-matrix" { return try await undoMatrix(workspace: workspace, arguments: arguments) }
        if document.reading { await workspace.toggleCurrentView() }
        for _ in 0..<500 {
            if DiagramInspection.current(document: workspace.current?.document)?.isReading == false { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        var view = try await DiagramInspection.settled(document: workspace.current?.document)
        view.window?.makeFirstResponder(view)
        if mode == "recovery" {
            guard document.issue == .recovery else { throw CocoaError(.coderValueNotFound) }
            await document.recover(true)
            let restored = view.string.hasPrefix("退出恢复😀")
            await document.saveNow()
            return ["recovery_prompt": true, "recovered": restored, "saved": !document.dirty]
        }
        guard view.isEditable else { throw CocoaError(.fileWriteNoPermission) }
        if mode == "preview" {
            guard let block = view.blocks.first else { throw CocoaError(.coderValueNotFound) }
            view.onEditSource?(block.ordinal)
            view.layoutSubtreeIfNeeded()
            return ["expanded_source": view.expanded.contains(block.ordinal), "preview_visible": view.diagramViews[block.ordinal]?.isHidden == false]
        }
        if mode == "shutdown-failure" {
            view.insertText("退出恢复😀", replacementRange: NSRange(location: 0, length: 0))
            // 笔记写入失败，但恢复稿可写：退出应快速放行。
            try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: document.io.url.path)
            return ["dirty_before_exit": document.dirty]
        }
        if mode == "shutdown-blocked" {
            view.insertText("退出被拦😀", replacementRange: NSRange(location: 0, length: 0))
            // 笔记与恢复稿都写不进去：退出必须被拦下并等待用户决定。
            try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: document.io.url.path)
            if let cache = ProcessInfo.processInfo.environment["HORTUS_CACHE_DIR"] {
                let recovery = URL(fileURLWithPath: cache).appendingPathComponent("recovery")
                try FileManager.default.createDirectory(at: recovery, withIntermediateDirectories: true)
                try FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: recovery.path)
            }
            return ["dirty_before_exit": document.dirty, "blocked_exit": true]
        }
        if mode == "metadata-failure" {
            view.insertText("替换应失败", replacementRange: NSRange(location: 0, length: 0))
            await document.saveNow()
            return ["save_failed": document.issue == .saveFailed, "dirty": document.dirty]
        }
        if mode == "performance" || mode == "styled-performance" {
            try await wait { view.sourceStyling?.pending == false }
            guard let styling = view.sourceStyling, workspace.noteOpenStartedAt > 0,
                  styling.openDisplayedAt >= workspace.noteOpenStartedAt else { throw CocoaError(.coderValueNotFound) }
            // 从打开笔记动作计时，包含后台读取、原生存储初始化和首次可见绘制。
            let openStyle = (styling.openDisplayedAt - workspace.noteOpenStartedAt) * 1000
            let openPipeline = styling.openMilliseconds
            var styleLatencies: [Double] = []
            var patchSizes: [Int] = []
            var stylePhases: [[String: Double]] = []
            if mode == "styled-performance" {
                for _ in 0..<30 {
                    let before = view.sourceStyling?.appliedCharacters ?? 0
                    let start = ProcessInfo.processInfo.systemUptime
                    view.insertText("字", replacementRange: NSRange(location: 0, length: 0))
                    let insertedAt = ProcessInfo.processInfo.systemUptime
                    for _ in 0..<5000 {
                        if view.sourceStyling?.pending == false && view.sourceStyling?.appliedRevision == document.revision { break }
                        try await Task.sleep(for: .milliseconds(1))
                    }
                    view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
                    styleLatencies.append((ProcessInfo.processInfo.systemUptime - start) * 1000)
                    patchSizes.append((view.sourceStyling?.appliedCharacters ?? 0) - before)
                    var phases = view.sourceStyling?.lastPhases ?? [:]
                    phases["input_ms"] = (insertedAt - start) * 1000
                    phases["scheduled_to_display_ms"] = view.sourceStyling?.lastLatencyMilliseconds ?? 0
                    stylePhases.append(phases)
                }
            }
            var times: [Double] = []
            for _ in 0..<300 {
                let start = ProcessInfo.processInfo.systemUptime
                view.insertText("字", replacementRange: NSRange(location: 0, length: 0))
                view.layoutSubtreeIfNeeded()
                times.append((ProcessInfo.processInfo.systemUptime - start) * 1000)
                // 模拟连续输入，把原生布局与后台任务交错放进真实主事件循环。
                try await Task.sleep(for: .milliseconds(5))
            }
            view = try await DiagramInspection.settled(document: workspace.current?.document)
            let snapshot = try await document.io.perform { try $0.snapshot() }
            let started = ProcessInfo.processInfo.systemUptime
            await document.saveNow()
            let elapsed = (ProcessInfo.processInfo.systemUptime - started) * 1000
            times.sort()
            return ["keystrokes": times.count, "utf8_bytes": snapshot.text.utf8.count,
                    "core_matches_native": snapshot.text == view.string,
                    "key_p95_ms": times[Int(Double(times.count - 1) * 0.95)], "key_max_ms": times.last ?? 0,
                    "save_wall_ms": elapsed, "save_core_ms": document.saveMilliseconds,
                    "saved": !document.dirty, "blocks": view.blocks.count,
                    "open_styles_ms": openStyle, "open_style_pipeline_ms": openPipeline,
                    "open_core_ms": view.sourceStyling?.openCoreMilliseconds ?? 0,
                    "open_phases": view.sourceStyling?.openPhases ?? [:], "open_note_phases": workspace.noteOpenPhases,
                    "open_ffi_ms": view.sourceStyling?.openFFIMilliseconds ?? 0, "style_latencies_ms": styleLatencies,
                    "style_update_max_ms": styleLatencies.max() ?? 0, "style_patch_characters": patchSizes,
                    "style_phases": stylePhases,
                    "diagramPending": view.diagramPending, "content_width": view.layoutContentWidth,
                    "stable_content_width": (view.enclosingScrollView as? DiagramScrollView)?.stableContentWidth ?? view.frame.width,
                    "scrollbar_setting": UserDefaults.standard.string(forKey: "AppleShowScrollBars") ?? "system",
                    "actual_scroller_style": DiagramInspection.scrollerStyleName(view.formulaScrollerStyle)]
        }
        view.insertText("自动保存😀", replacementRange: NSRange(location: 0, length: 0))
        let autosaveStart = ProcessInfo.processInfo.systemUptime
        try await wait { !document.dirty }
        let autosaveDelay = ProcessInfo.processInfo.systemUptime - autosaveStart
        fputs("编辑验收：自动保存完成\n", stderr)
        if mode == "idle" {
            return ["saved": true, "autosave_delay_seconds": autosaveDelay,
                    "scrollbar_setting": UserDefaults.standard.string(forKey: "AppleShowScrollBars") ?? "system",
                    "actual_scroller_style": DiagramInspection.scrollerStyleName(view.formulaScrollerStyle)]
        }
        view.insertText("命令保存", replacementRange: NSRange(location: 0, length: 0))
        NSApp.mainMenu?.update()
        if let fileMenu = NSApp.mainMenu?.items.first(where: { $0.submenu?.items.contains(where: { $0.keyEquivalent == "s" }) == true })?.submenu {
            fileMenu.delegate?.menuNeedsUpdate?(fileMenu)
            fileMenu.update()
        }
        await Task.yield()
        guard let item = saveItem(NSApp.mainMenu), let action = item.action,
              NSApp.sendAction(action, to: item.target, from: item) else { throw NSError(domain: "EditingInspection", code: 1, userInfo: [NSLocalizedDescriptionKey: "没有可执行的保存菜单项"]) }
        try await wait { !document.dirty }
        let commandSave = try diskMatches(document, view)
        fputs("编辑验收：命令保存完成\n", stderr)
        view.insertText("失焦保存", replacementRange: NSRange(location: 0, length: 0))
        view.window?.resignKey()
        try await wait { !document.dirty }
        let focusSave = try diskMatches(document, view)
        view.window?.makeKey()
        fputs("编辑验收：失焦保存完成\n", stderr)
        view.insertText("切换标签", replacementRange: NSRange(location: 0, length: 0))
        workspace.openNote("other.md")
        try await wait { !document.dirty }
        workspace.openNote(path)
        try await wait { DiagramInspection.current(document: workspace.current?.document)?.representedPath == path && DiagramInspection.current(document: workspace.current?.document)?.diagramPending == false }
        view = try await DiagramInspection.settled(document: workspace.current?.document)
        let switchSave = try diskMatches(document, view)
        fputs("编辑验收：切换标签完成\n", stderr)
        let selected = min(3, view.sourceUTF16Length)
        view.setSelectedRange(NSRange(location: selected, length: 0))
        let external = "外部更新\n" + view.string
        let externalStart = ProcessInfo.processInfo.systemUptime
        try Data(external.utf8).write(to: document.io.url)
        try await wait { view.string == external }
        let externalDelay = ProcessInfo.processInfo.systemUptime - externalStart
        let caret = view.selectedRange().location
        let undoCleared = view.undoManager?.canUndo != true
        fputs("编辑验收：外部载入完成\n", stderr)
        view.insertText("关闭标签", replacementRange: NSRange(location: 0, length: 0))
        let closingSource = view.string
        workspace.closeTab(path)
        try await wait { !workspace.tabs.contains(where: { $0.path == path }) }
        let closeSaved = try String(contentsOf: document.io.url, encoding: .utf8) == closingSource
        fputs("编辑验收：关闭标签完成\n", stderr)
        workspace.openNote(path)
        try await wait { workspace.current?.document != nil && workspace.current?.loading == false }
        try await wait { DiagramInspection.current(document: workspace.current?.document)?.representedDocument == workspace.current?.document.map(ObjectIdentifier.init) && DiagramInspection.current(document: workspace.current?.document)?.diagramPending == false }
        view = try await DiagramInspection.settled(document: workspace.current?.document)
        guard let reopened = workspace.current?.document, let originalRoot = workspace.root else { throw CocoaError(.coderValueNotFound) }
        view.insertText("切换库", replacementRange: NSRange(location: 0, length: 0))
        let librarySource = view.string
        let alternate = URL(fileURLWithPath: originalRoot).deletingLastPathComponent().appendingPathComponent("alternate")
        await workspace.openFolder(alternate.path)
        let librarySave = try String(contentsOf: reopened.io.url, encoding: .utf8) == librarySource
        await workspace.openFolder(originalRoot)
        try await wait { workspace.current?.document != nil && workspace.current?.loading == false }
        try await wait { DiagramInspection.current(document: workspace.current?.document)?.representedDocument == workspace.current?.document.map(ObjectIdentifier.init) && DiagramInspection.current(document: workspace.current?.document)?.diagramPending == false }
        view = try await DiagramInspection.settled(document: workspace.current?.document)
        // 给退出留下真实脏内容，由 applicationShouldTerminate 完成最后一次保存。
        view.insertText("退出保存", replacementRange: NSRange(location: 0, length: 0))
        return ["autosave_delay_seconds": autosaveDelay, "command_save": commandSave, "focus_save": focusSave,
                "switch_save": switchSave, "library_save": librarySave, "close_save": closeSaved, "external_delay_seconds": externalDelay,
                "selection_preserved": caret == selected, "undo_cleared": undoCleared,
                "dirty_before_exit": workspace.current?.document?.dirty == true,
                "scrollbar_setting": UserDefaults.standard.string(forKey: "AppleShowScrollBars") ?? "system",
                "actual_scroller_style": DiagramInspection.scrollerStyleName(view.formulaScrollerStyle)]
    }

    private enum DocumentSwitchRoute: String, Equatable {
        case tab
        case search
    }

    /// 从公开双标签会话开始，经真实 IME 组合后点击标签或已显示搜索结果。
    /// 外部驱动在每个阶段读取两篇笔记；本函数不读磁盘，也不以内部状态替代用户路径。
    private static func compositionDocumentSwitch(
        workspace: Workspace, arguments: [String], route: DocumentSwitchRoute
    ) async throws -> [String: Any] {
        let mode = "composition-\(route.rawValue)-switch"
        let bridge = try PhaseBridge(arguments: arguments)
        defer { bridge.close() }
        let originalInputSource = selectedInputSourceID()
        var result: [String: Any] = ["mode": mode, "event_tap": "cghidEventTap"]
        defer { _ = restoreInputSource(originalInputSource) }
        let alphaPath = "alpha.md", betaPath = "beta.md"
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
              let alpha = workspace.tabs.first(where: { $0.path == alphaPath })?.document,
              let beta = workspace.tabs.first(where: { $0.path == betaPath })?.document,
              workspace.selected == alphaPath else {
            result["status"] = "environment_failed"
            result["prerequisites"] = ["window": false, "alpha_document": false, "beta_document": false,
                                       "alpha_selected": workspace.selected == alphaPath]
            return result
        }
        let access = CGPreflightPostEventAccess()
        let activation = await activate(window)
        var prerequisites: [String: Any] = [
            "event_access": access, "application_active": NSApp.isActive,
            "activation_converged": activation, "key_window": window.isKeyWindow,
            "alpha_loaded": !(workspace.tabs.first(where: { $0.path == alphaPath })?.loading ?? true),
            "beta_loaded": !(workspace.tabs.first(where: { $0.path == betaPath })?.loading ?? true),
            "alpha_selected": workspace.selected == alphaPath,
            "input_source_before": originalInputSource ?? "",
        ]
        result["input_source_before"] = originalInputSource ?? ""
        result["focus_event_tap"] = "cghidEventTap"
        guard access, activation, window.isKeyWindow,
              workspace.tabs.first(where: { $0.path == alphaPath })?.loading == false,
              workspace.tabs.first(where: { $0.path == betaPath })?.loading == false else {
            result["status"] = "environment_failed"
            result["prerequisites"] = prerequisites
            return result
        }

        let alphaView = try await DiagramInspection.settled(document: alpha)
        guard !alpha.reading, alphaView.isEditable else {
            result["status"] = "environment_failed"
            result["prerequisites"] = prerequisites.merging(["alpha_editing": false]) { _, new in new }
            return result
        }

        var tabTarget: [String: Any]?
        var searchTarget: AccessibilityTarget?
        if route == .tab {
            guard let button = tabButton(in: window, path: betaPath) else {
                result["status"] = "environment_failed"
                result["prerequisites"] = prerequisites.merging(["beta_tab_visible": false]) { _, new in new }
                return result
            }
            tabTarget = ["path": button.path, "label": button.accessibilityLabel() ?? "",
                         "frame": NSStringFromRect(window.convertToScreen(button.convert(button.bounds, to: nil))),
                         "selected": button.title]
        } else {
            let lastEvent = await press(3, flags: [.maskCommand, .maskShift])
            guard let content = window.contentView,
                  await waitUntil(5, { firstView(of: NativeSearchField.self, in: content)?.isHiddenOrHasHiddenAncestor == false }),
                  let field = firstView(of: NativeSearchField.self, in: content) else {
                result["status"] = "environment_failed"
                result["prerequisites"] = prerequisites.merging(["search_panel": false]) { _, new in new }
                return result
            }
            let latin = selectInputSource(ABCInputSourceID)
            prerequisites["search_input_source_selected"] = latin.available && latin.selected
            prerequisites["search_panel"] = true
            result["search_input_source_selected"] = latin.selected
            guard latin.available, latin.selected else {
                result["status"] = "environment_failed"
                result["prerequisites"] = prerequisites
                return result
            }
            await click(center(of: field, in: window), window: window)
            let fieldFocused = await waitUntil(5) { field.currentEditor() != nil }
            prerequisites["search_field_focused"] = fieldFocused
            guard fieldFocused else {
                result["status"] = "environment_failed"
                result["prerequisites"] = prerequisites
                return result
            }
            await typeText("searchb")
            _ = await waitUntil(5) {
                workspace.searchQuery == "searchb" && !workspace.searchLoading
                    && workspace.searchResults.map(\.path) == [betaPath]
            }
            searchTarget = await waitForAccessibilityButton(in: window, containing: "searchb", seconds: 5)
            result["search_results"] = workspace.searchResults.map(\.path)
            result["search_query"] = workspace.searchQuery
            if let searchTarget {
                result["search_target"] = searchTarget.evidence
            } else {
                result["accessibility_candidates"] = accessibilityCandidates(in: window)
            }
            let validResults = workspace.searchResults.count == 1 && workspace.searchResults[0].path == betaPath
            prerequisites["single_beta_result"] = validResults
            prerequisites["beta_result_visible"] = searchTarget != nil
            guard validResults, searchTarget != nil else {
                result["status"] = "environment_failed"
                result["prerequisites"] = prerequisites
                return result
            }
            await click(textPoint(of: alphaView, in: window), window: window)
            let returnedToEditor = await waitUntil(5) {
                window.firstResponder === alphaView && (field.currentEditor() as? NSTextView)?.hasMarkedText() != true
            }
            let scim = selectSCIM()
            prerequisites["editor_focused"] = returnedToEditor
            prerequisites["input_source_available"] = scim.available
            prerequisites["input_source_selected"] = scim.selected
            prerequisites["search_not_refocused"] = !(field.currentEditor() != nil && window.firstResponder === field.currentEditor())
            result["input_source_status"] = scim.status
            result["input_source_available"] = scim.available
            result["input_source_selected"] = scim.selected
            result["input_source_after_select"] = selectedInputSourceID() ?? ""
            guard returnedToEditor, scim.available, scim.selected else {
                result["status"] = "environment_failed"
                result["prerequisites"] = prerequisites
                return result
            }
            result["search_last_preparation_event"] = lastEvent
        }

        if route == .tab {
            let scim = selectSCIM()
            prerequisites["input_source_available"] = scim.available
            prerequisites["input_source_selected"] = scim.selected
            result["input_source_status"] = scim.status
            result["input_source_available"] = scim.available
            result["input_source_selected"] = scim.selected
            result["input_source_after_select"] = selectedInputSourceID() ?? ""
            guard scim.available, scim.selected else {
                result["status"] = "environment_failed"
                result["prerequisites"] = prerequisites
                return result
            }
            await click(textPoint(of: alphaView, in: window), window: window)
            let focused = await waitUntil(5) { window.firstResponder === alphaView }
            prerequisites["editor_focused"] = focused
            guard focused else {
                result["status"] = "environment_failed"
                result["prerequisites"] = prerequisites
                return result
            }
        }

        try await bridge.emit("before_composition", state: switchSnapshot(
            workspace: workspace, window: window, trackedView: alphaView, lastEvent: [:], field: firstSearchField(in: window)))
        let compositionEvent = await typeText("nihao")
        let compositionStarted = await waitUntil(5) { alphaView.hasMarkedText() && alpha.marked }
        let beforeClick = switchSnapshot(workspace: workspace, window: window, trackedView: alphaView,
                                         lastEvent: compositionEvent, field: firstSearchField(in: window))
        let beforeClickTargetOK: Bool
        if route == .tab {
            beforeClickTargetOK = window.firstResponder === alphaView && alphaView.hasMarkedText() && alpha.marked
                && (beforeClick["search_editor_active"] as? Bool) != true
        } else {
            let visible = accessibilityButton(in: window, containing: "searchb") != nil
            beforeClickTargetOK = window.firstResponder === alphaView && alphaView.hasMarkedText() && alpha.marked
                && (beforeClick["search_editor_active"] as? Bool) == false && visible
        }
        prerequisites["composition_started"] = compositionStarted
        prerequisites["switch_preconditions"] = beforeClickTargetOK
        result["prerequisites"] = prerequisites
        result["tab_target"] = tabTarget
        try await bridge.emit("composition_active", state: beforeClick)
        guard compositionStarted, beforeClickTargetOK else {
            result["status"] = "environment_failed"
            if alphaView.hasMarkedText() { _ = await press(53, flags: []) }
            return result
        }

        let switchedAt = ProcessInfo.processInfo.systemUptime
        if route == .tab {
            guard let button = tabButton(in: window, path: betaPath) else {
                result["status"] = "environment_failed"
                return result
            }
            await click(center(of: button, in: window), window: window)
        } else if let searchTarget {
            await click(accessibilityFrame: searchTarget.frame, window: window)
        }
        let switched = await waitUntil(5) {
            workspace.selected == betaPath && workspace.current?.document === beta
                && DiagramInspection.current(document: beta)?.representedPath == betaPath
                && DiagramInspection.current(document: beta)?.string == beta.text
        }
        result["switched_to_beta"] = switched
        try await bridge.emit("after_switch", state: switchSnapshot(
            workspace: workspace, window: window, trackedView: alphaView, lastEvent: [:], field: firstSearchField(in: window)))

        let autosaved = await waitUntil(2.0) { !alpha.marked && !alpha.dirty }
        result["alpha_autosave_seconds"] = ProcessInfo.processInfo.systemUptime - switchedAt
        result["alpha_autosaved"] = autosaved
        try await bridge.emit("after_autosave", state: switchSnapshot(
            workspace: workspace, window: window, trackedView: alphaView, lastEvent: [:], field: firstSearchField(in: window)))

        var betaInput = false
        if let betaView = try? await DiagramInspection.settled(document: beta) {
            await click(textPoint(of: betaView, in: window), window: window)
            _ = await waitUntil(5) { window.firstResponder === betaView && betaView.inputContext === NSTextInputContext.current }
            await typeText("zhong")
            let lastEvent = await press(49, flags: [])
            betaInput = await waitUntil(8) { !betaView.hasMarkedText() && !beta.marked && !beta.dirty }
            result["beta_input_settled"] = betaInput
            try await bridge.emit("after_beta_input", state: switchSnapshot(
                workspace: workspace, window: window, trackedView: alphaView, lastEvent: lastEvent, field: firstSearchField(in: window)))
        } else {
            result["beta_input_settled"] = false
        }

        if let button = tabButton(in: window, path: alphaPath) {
            await click(center(of: button, in: window), window: window)
        }
        let returned = await waitUntil(5) {
            workspace.selected == alphaPath && DiagramInspection.current(document: alpha)?.representedPath == alphaPath
        }
        result["switched_back_to_alpha"] = returned
        let alphaViewAfter = (try? await DiagramInspection.settled(document: alpha)) ?? alphaView
        if returned { await click(textPoint(of: alphaViewAfter, in: window), window: window) }
        _ = await waitUntil(2) { window.firstResponder === alphaViewAfter }
        try await bridge.emit("switched_back", state: switchSnapshot(
            workspace: workspace, window: window, trackedView: alphaViewAfter, lastEvent: [:], field: firstSearchField(in: window)))

        await typeText("zhong")
        var lastEvent = await press(49, flags: [])
        let alphaInput = await waitUntil(5) {
            !alphaViewAfter.hasMarkedText() && !alpha.marked
                && alphaViewAfter.string == "中" + "ni hao" + alpha.text
        }
        result["alpha_return_input_settled"] = alphaInput
        try await bridge.emit("alpha_refocused_input", state: switchSnapshot(
            workspace: workspace, window: window, trackedView: alphaViewAfter, lastEvent: lastEvent, field: firstSearchField(in: window)))

        lastEvent = await press(1, flags: .maskCommand)
        let saved = await waitUntil(5) { !alpha.marked && !alpha.dirty }
        result["command_s_after_return"] = saved
        try await bridge.emit("command_s_after_return", state: switchSnapshot(
            workspace: workspace, window: window, trackedView: alphaViewAfter, lastEvent: lastEvent, field: firstSearchField(in: window)))
        lastEvent = await press(14, flags: .maskCommand)
        let reading = await waitUntil(5) { alpha.reading }
        result["command_e_after_return"] = reading
        try await bridge.emit("command_e_after_return", state: switchSnapshot(
            workspace: workspace, window: window, trackedView: alphaViewAfter, lastEvent: lastEvent, field: firstSearchField(in: window)))

        result["status"] = "completed"
        result["phase_count"] = bridge.phaseCount
        result["ack_count"] = bridge.ackCount
        result["input_source_restored"] = restoreInputSource(originalInputSource)
        result["input_source_after_run"] = selectedInputSourceID() ?? ""
        return result
    }

    private struct AccessibilityTarget {
        let object: AnyObject
        let role: String
        let label: String
        let help: String
        let frame: NSRect
        var evidence: [String: Any] {
            ["role": role, "label": label, "help": help, "frame": NSStringFromRect(frame),
             "object": String(describing: ObjectIdentifier(object))]
        }
    }

    private struct AccessibilityInfo {
        let role: String
        let label: String
        let help: String
        let frame: NSRect
    }

    private static func accessibilityInfo(_ object: Any) -> AccessibilityInfo? {
        if let view = object as? NSView {
            return AccessibilityInfo(role: view.accessibilityRole()?.rawValue ?? "",
                                     label: view.accessibilityLabel() ?? "", help: view.accessibilityHelp() ?? "",
                                     frame: view.accessibilityFrame())
        }
        if let element = object as? NSAccessibilityElement {
            return AccessibilityInfo(role: element.accessibilityRole()?.rawValue ?? "",
                                     label: element.accessibilityLabel() ?? "", help: element.accessibilityHelp() ?? "",
                                     frame: element.accessibilityFrame())
        }
        return nil
    }

    private static func accessibilityChildren(_ object: Any) -> [Any] {
        if let view = object as? NSView { return view.accessibilityChildren() ?? [] }
        if let element = object as? NSAccessibilityElement { return element.accessibilityChildren() ?? [] }
        return []
    }

    private static func accessibilityTargets(in window: NSWindow, limit: Int = 1_500) -> [AccessibilityTarget] {
        guard let root = window.contentView else { return [] }
        var queue: [(Any, Int)] = [(root, 0)]
        var seen = Set<ObjectIdentifier>()
        var values: [AccessibilityTarget] = []
        while !queue.isEmpty, values.count < limit {
            let (object, depth) = queue.removeFirst()
            guard depth <= 24 else { continue }
            if let object = object as AnyObject?, seen.insert(ObjectIdentifier(object)).inserted == false { continue }
            if let info = accessibilityInfo(object), !info.role.isEmpty {
                values.append(AccessibilityTarget(object: object as AnyObject, role: info.role,
                                                  label: info.label, help: info.help, frame: info.frame))
            }
            if depth < 24 {
                queue.append(contentsOf: accessibilityChildren(object).map { ($0, depth + 1) })
            }
        }
        for target in axAccessibilityTargets(limit: limit) where !values.contains(where: {
            $0.role == target.role && $0.label == target.label && $0.help == target.help
                && abs($0.frame.midX - target.frame.midX) < 1 && abs($0.frame.midY - target.frame.midY) < 1
        }) {
            values.append(target)
        }
        return values
    }

    private static func axAttribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
        return value
    }

    private static func axAccessibilityTargets(limit: Int) -> [AccessibilityTarget] {
        let app = AXUIElementCreateApplication(getpid())
        var queue: [(AXUIElement, Int)] = [(app, 0)]
        var seen = Set<CFHashCode>()
        var values: [AccessibilityTarget] = []
        let top = NSScreen.screens.max { $0.frame.maxY < $1.frame.maxY }?.frame.maxY
            ?? NSScreen.main?.frame.height ?? 0
        while !queue.isEmpty, values.count < limit {
            let (element, depth) = queue.removeFirst()
            guard depth <= 32, seen.insert(CFHash(element)).inserted else { continue }
            let role = (axAttribute(element, kAXRoleAttribute) as? String) ?? ""
            let texts = [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute, kAXHelpAttribute]
                .compactMap { axAttribute(element, $0) as? String }
                .filter { !$0.isEmpty }
            if !role.isEmpty {
                var frame = NSRect.zero
                if let positionRaw = axAttribute(element, kAXPositionAttribute),
                   let sizeRaw = axAttribute(element, kAXSizeAttribute),
                   CFGetTypeID(positionRaw) == AXValueGetTypeID(),
                   CFGetTypeID(sizeRaw) == AXValueGetTypeID() {
                    let positionValue = unsafeDowncast(positionRaw, to: AXValue.self)
                    let sizeValue = unsafeDowncast(sizeRaw, to: AXValue.self)
                    var point = CGPoint.zero, size = CGSize.zero
                    if AXValueGetValue(positionValue, .cgPoint, &point), AXValueGetValue(sizeValue, .cgSize, &size) {
                        frame = NSRect(x: point.x, y: top - point.y - size.height, width: size.width, height: size.height)
                    }
                }
                let label = texts.first ?? ""
                let help = (axAttribute(element, kAXHelpAttribute) as? String) ?? ""
                values.append(AccessibilityTarget(object: element, role: role, label: label, help: help, frame: frame))
            }
            if depth < 32, let children = axAttribute(element, kAXChildrenAttribute) as? [AXUIElement] {
                queue.append(contentsOf: children.map { ($0, depth + 1) })
            }
        }
        return values
    }

    private static func accessibilityButton(in window: NSWindow, containing text: String) -> AccessibilityTarget? {
        let query = text.lowercased()
        let screen = window.screen?.frame ?? NSScreen.main?.frame ?? .zero
        return accessibilityTargets(in: window).first { target in
            target.role == NSAccessibility.Role.button.rawValue
                && (target.label.lowercased().contains(query) || target.help.lowercased().contains(query))
                && target.frame.width > 0 && target.frame.height > 0 && screen.intersects(target.frame)
        }
    }

    private static func waitForAccessibilityButton(in window: NSWindow, containing text: String,
                                                    seconds: Double) async -> AccessibilityTarget? {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while ProcessInfo.processInfo.systemUptime < deadline {
            if let target = accessibilityButton(in: window, containing: text) { return target }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return accessibilityButton(in: window, containing: text)
    }

    private static func accessibilityCandidates(in window: NSWindow) -> [[String: Any]] {
        accessibilityTargets(in: window)
            .filter { $0.role == NSAccessibility.Role.button.rawValue }
            .prefix(80)
            .map(\.evidence)
    }

    private static func tabButton(in window: NSWindow, path: String) -> TabDragButton.Button? {
        guard let toolbar = window.toolbar?.items.first(where: { $0.itemIdentifier.rawValue == "editor-tabs" })?.view else {
            return nil
        }
        func find(in view: NSView) -> TabDragButton.Button? {
            if let button = view as? TabDragButton.Button, button.path == path { return button }
            for child in view.subviews {
                if let found = find(in: child) { return found }
            }
            return nil
        }
        return find(in: toolbar)
    }

    private static func firstSearchField(in window: NSWindow) -> NativeSearchField? {
        window.contentView.flatMap { firstView(of: NativeSearchField.self, in: $0) }
    }

    private static func switchSnapshot(workspace: Workspace, window: NSWindow, trackedView: DiagramTextView?,
                                       lastEvent: [String: Any], field: NativeSearchField?) -> [String: Any] {
        var state = snapshot(workspace: workspace, window: window, lastEvent: lastEvent)
        state["selected_path"] = workspace.selected ?? ""
        state["tabs"] = workspace.tabs.map(\.path)
        var documents: [String: Any] = [:]
        for tab in workspace.tabs {
            guard let document = tab.document else {
                documents[tab.path] = ["present": false, "loading": tab.loading]
                continue
            }
            let id = ObjectIdentifier(document)
            let view = DiagramInspection.current(document: document)
            var value: [String: Any] = [
                "present": true, "document_id": String(describing: id), "loading": tab.loading,
                "marked": document.marked, "dirty": document.dirty, "revision": document.revision,
                "text_revision": document.textRevision, "reading": document.reading,
                "issue": String(describing: document.issue), "stored_text": document.text,
                "view_present": view != nil,
            ]
            if let view { value["view"] = diagramViewState(view) }
            documents[tab.path] = value
        }
        state["documents"] = documents
        state["tracked_view"] = trackedView.map(diagramViewState) ?? [:]
        state["search_field_focused"] = field?.currentEditor() != nil
        return state
    }

    private static func diagramViewState(_ view: DiagramTextView) -> [String: Any] {
        let selection = view.selectedRange()
        let context = view.inputContext
        return [
            "object": String(describing: ObjectIdentifier(view)),
            "path": view.representedPath,
            "document": view.representedDocument.map(String.init(describing:)) ?? "",
            "text": view.string, "length": view.textStorage?.length ?? -1,
            "marked_text": view.hasMarkedText(), "composition": view.compositionActive,
            "selection": ["location": selection.location, "length": selection.length],
            "editable": view.isEditable, "reading": view.isReading,
            "marked_range": view.markedRange().location == NSNotFound
                ? NSNull()
                : ["location": view.markedRange().location, "length": view.markedRange().length],
            "input_context_id": context.map { String(describing: ObjectIdentifier($0)) } ?? "",
            "current_input_context_id": NSTextInputContext.current.map { String(describing: ObjectIdentifier($0)) } ?? "",
            "input_context_current": context != nil && context === NSTextInputContext.current,
        ]
    }
    /// 真实点击编辑器建立焦点，由 SCIM ITABC 建立组合，再真实点击底部终端让焦点离开。
    /// 每个阶段都由外部驱动读盘并 ack；本函数不读取磁盘，也不替外部结论作判定。
    private static func compositionFocusLoss(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        let bridge = try PhaseBridge(arguments: arguments)
        defer { bridge.close() }
        let originalInputSource = selectedInputSourceID()
        var result: [String: Any] = ["mode": "composition-focus-loss", "event_tap": "cghidEventTap"]
        defer { _ = restoreInputSource(originalInputSource) }
        guard let document = workspace.current?.document,
              let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else {
            result["status"] = "environment_failed"
            result["prerequisites"] = ["window": false, "document": false]
            return result
        }
        let access = CGPreflightPostEventAccess()
        result["post_event_access"] = access
        result["input_source_requested"] = SCIMInputSourceID
        result["input_source_before"] = originalInputSource ?? ""
        result["focus_event_tap"] = "cghidEventTap"
        guard access else {
            result["status"] = "environment_failed"
            result["prerequisites"] = ["event_access": access]
            return result
        }
        let activation = await activate(window)
        let input = selectSCIM()
        result["input_source_status"] = input.status
        result["input_source_available"] = input.available
        result["input_source_selected"] = input.selected
        result["input_source_after_select"] = selectedInputSourceID() ?? ""
        var terminal: NSView?
        if let content = window.contentView {
            terminal = await waitUntil(5) {
                firstView(named: "HortusTerminalView", in: content).flatMap { !$0.isHiddenOrHasHiddenAncestor ? $0 : nil } != nil
            } ? firstView(named: "HortusTerminalView", in: content) : nil
        }
        let terminalVisible = terminal.map { !$0.isHiddenOrHasHiddenAncestor } ?? false
        var prerequisites: [String: Any] = [
            "event_access": access, "application_active": NSApp.isActive,
            "activation_converged": activation, "key_window": window.isKeyWindow,
            "terminal_found": terminal != nil, "terminal_visible": terminalVisible,
            "input_source_available": input.available, "input_source_selected": input.selected,
            "input_source_before": originalInputSource ?? "",
        ]
        guard activation, terminal != nil else {
            result["status"] = "environment_failed"
            result["prerequisites"] = prerequisites
            return result
        }
        let view = try await DiagramInspection.settled(document: document)
        await click(textPoint(of: view, in: window), window: window)
        let focused = await waitUntil(5) { window.firstResponder === view }
        prerequisites["editor_focused"] = focused
        prerequisites["focus_event_tap"] = "cghidEventTap"
        result["prerequisites"] = prerequisites
        guard focused else {
            result["status"] = "environment_failed"
            return result
        }
        var lastEvent: [String: Any] = [:]
        await typeText("zhong")
        lastEvent = await press(49, flags: [])
        let committed = await waitUntil(10) { !view.hasMarkedText() && !document.marked && !document.dirty }
        result["initial_commit"] = committed
        try await bridge.emit("before_composition", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        await typeText("nihao")
        let composition = await waitUntil(5) { view.hasMarkedText() && document.marked }
        result["composition_started"] = composition
        try await bridge.emit("composition_active", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        lastEvent = await press(1, flags: .maskCommand)
        try? await Task.sleep(for: .milliseconds(700))
        try await bridge.emit("command_s_while_composing", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        lastEvent = await press(14, flags: .maskCommand)
        try? await Task.sleep(for: .milliseconds(700))
        try await bridge.emit("command_e_while_composing", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        await click(center(of: terminal!, in: window), window: window)
        let focusLeft = await waitUntil(5) { window.firstResponder !== view }
        result["focus_left_editor"] = focusLeft
        try await bridge.emit("focus_left", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        let autoSaved = await waitUntil(5) { !view.hasMarkedText() && !document.marked && !document.dirty }
        result["autosave_after_focus_loss"] = autoSaved
        try await bridge.emit("autosave_after_focus_loss", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        // 第二次组合在失焦后立即用真实 ⌘S 保存，避免把已经干净的磁盘误当成命令生效。
        await click(textPoint(of: view, in: window), window: window)
        let refocused = await waitUntil(5) {
            window.firstResponder === view
                && view.inputContext === NSTextInputContext.current
                && view.selectedRange() == NSRange(location: 0, length: 0)
                && !view.hasMarkedText() && !document.marked
        }
        result["second_editor_focus"] = refocused
        result["second_editor_context_converged"] = view.inputContext === NSTextInputContext.current
        try await bridge.emit("second_editor_focused", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        if refocused {
            await typeText("hao")
            let secondComposition = await waitUntil(4) { view.hasMarkedText() && document.marked }
            result["second_composition_started"] = secondComposition
            try await bridge.emit("second_composition_active", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
            await click(center(of: terminal!, in: window), window: window)
            let secondFocusLeft = await waitUntil(4) { window.firstResponder !== view }
            result["second_focus_left_editor"] = secondFocusLeft
            try await bridge.emit("second_focus_left", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
            lastEvent = await press(1, flags: .maskCommand)
            let saveSettled = await waitUntil(5) { !view.hasMarkedText() && !document.marked && !document.dirty }
            result["command_s_after_focus_loss"] = saveSettled
            try await bridge.emit("command_s_after_focus_loss", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        }

        lastEvent = await press(14, flags: .maskCommand)
        let toggled = await waitUntil(5) { document.reading }
        result["command_e_after_focus_loss"] = toggled
        try await bridge.emit("command_e_after_focus_loss", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        result["status"] = prerequisites.values.allSatisfy { ($0 as? Bool) != false } ? "completed" : "environment_failed"
        result["phase_count"] = bridge.phaseCount
        result["ack_count"] = bridge.ackCount
        result["input_source_restored"] = restoreInputSource(originalInputSource)
        result["input_source_after_run"] = selectedInputSourceID() ?? ""
        return result
    }

    /// 独立撤销场景：先真实提交，再由真实 ⌘Z/⇧⌘Z 回退和恢复；最后验证组合中 ⌘Z 不改状态。
    private static func undoRedo(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        let bridge = try PhaseBridge(arguments: arguments)
        defer { bridge.close() }
        let originalInputSource = selectedInputSourceID()
        var result: [String: Any] = ["mode": "undo-redo", "event_tap": "cghidEventTap"]
        defer { _ = restoreInputSource(originalInputSource) }
        guard let document = workspace.current?.document,
              let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else {
            result["status"] = "environment_failed"
            return result
        }
        let access = CGPreflightPostEventAccess()
        let activation = await activate(window)
        let input = selectSCIM()
        let view = try await DiagramInspection.settled(document: document)
        await click(textPoint(of: view, in: window), window: window)
        let focused = await waitUntil(5) { window.firstResponder === view }
        let prerequisites: [String: Any] = ["event_access": access, "application_active": NSApp.isActive,
                                             "activation_converged": activation, "key_window": window.isKeyWindow,
                                             "editor_focused": focused, "input_source_available": input.available,
                                             "input_source_selected": input.selected,
                                             "input_source_before": originalInputSource ?? ""]
        result["prerequisites"] = prerequisites
        result["input_source_status"] = input.status
        result["input_source_after_select"] = selectedInputSourceID() ?? ""
        result["focus_event_tap"] = "cghidEventTap"
        guard access, input.available, input.selected, activation, focused else {
            result["status"] = "environment_failed"
            result["failure_state"] = snapshot(workspace: workspace, window: window, lastEvent: [:])
            return result
        }
        var lastEvent: [String: Any] = [:]
        await typeText("zhong")
        lastEvent = await press(49, flags: [])
        _ = await waitUntil(10) { !view.hasMarkedText() && !document.marked && !document.dirty }
        try await bridge.emit("commit_ready", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        let commitRevision = document.revision
        lastEvent = await press(6, flags: .maskCommand)
        let undone = await waitUntil(5) { document.revision != commitRevision && !document.dirty }
        try? await Task.sleep(for: .milliseconds(300))
        result["undo_changed_document"] = undone
        try await bridge.emit("after_undo", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        let undoRevision = document.revision
        lastEvent = await press(6, flags: [.maskCommand, .maskShift])
        let redone = await waitUntil(5) { document.revision != undoRevision && !document.dirty }
        try? await Task.sleep(for: .milliseconds(300))
        result["redo_changed_document"] = redone
        try await bridge.emit("after_redo", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        await typeText("nihao")
        let composing = await waitUntil(5) { view.hasMarkedText() && document.marked }
        let beforeCompositeUndo = snapshot(workspace: workspace, window: window, lastEvent: lastEvent)
        result["composition_undo_prerequisite"] = composing && (view.undoManager?.canUndo == true)
        try await bridge.emit("composition_undo_ready", state: beforeCompositeUndo)
        lastEvent = await press(6, flags: .maskCommand)
        try? await Task.sleep(for: .milliseconds(800))
        let afterCompositeUndo = snapshot(workspace: workspace, window: window, lastEvent: lastEvent)
        result["composition_undo_unchanged"] = beforeCompositeUndo["view_text"] as? String == afterCompositeUndo["view_text"] as? String
            && beforeCompositeUndo["revision"] as? UInt64 == afterCompositeUndo["revision"] as? UInt64
            && afterCompositeUndo["marked_text"] as? Bool == true
        try await bridge.emit("after_composition_undo", state: afterCompositeUndo)

        // 清理组合，避免退出时留下恢复稿；清理动作不作为通过证据。
        if view.hasMarkedText() {
            _ = await press(53, flags: [])
            _ = await waitUntil(2) { !view.hasMarkedText() }
        }
        await click(textPoint(of: noteView(in: window) ?? view, in: window), window: window)
        result["status"] = prerequisites.values.allSatisfy { ($0 as? Bool) != false } ? "completed" : "environment_failed"
        result["phase_count"] = bridge.phaseCount
        result["ack_count"] = bridge.ackCount
        result["input_source_restored"] = restoreInputSource(originalInputSource)
        result["input_source_after_run"] = selectedInputSourceID() ?? ""
        return result
    }

    /// 同一撤销矩阵覆盖编辑、阅读、终端和搜索框；阅读与终端只验证不误触笔记撤销。
    private static func undoMatrix(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        let bridge = try PhaseBridge(arguments: arguments)
        defer { bridge.close() }
        let originalInputSource = selectedInputSourceID()
        var result: [String: Any] = ["mode": "undo-matrix", "event_tap": "cghidEventTap"]
        defer { _ = restoreInputSource(originalInputSource) }
        guard let document = workspace.current?.document,
              let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else {
            result["status"] = "environment_failed"
            return result
        }
        let access = CGPreflightPostEventAccess()
        let activation = await activate(window)
        let input = selectSCIM()
        let view = try await DiagramInspection.settled(document: document)
        await click(textPoint(of: view, in: window), window: window)
        let focused = await waitUntil(5) { window.firstResponder === view }
        let prerequisites: [String: Any] = ["event_access": access, "application_active": NSApp.isActive,
                                             "activation_converged": activation, "key_window": window.isKeyWindow,
                                             "editor_focused": focused, "input_source_available": input.available,
                                             "input_source_selected": input.selected,
                                             "input_source_before": originalInputSource ?? ""]
        result["prerequisites"] = prerequisites
        result["input_source_status"] = input.status
        result["input_source_after_select"] = selectedInputSourceID() ?? ""
        result["focus_event_tap"] = "cghidEventTap"
        guard access, input.available, input.selected, activation, focused else {
            result["status"] = "environment_failed"
            result["failure_state"] = snapshot(workspace: workspace, window: window, lastEvent: [:])
            return result
        }
        var lastEvent: [String: Any] = [:]
        await typeText("zhong")
        lastEvent = await press(49, flags: [])
        _ = await waitUntil(10) { !view.hasMarkedText() && !document.marked && !document.dirty }
        try await bridge.emit("editor", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        lastEvent = await press(14, flags: .maskCommand)
        _ = await waitUntil(5) { document.reading }
        let readingView = try await DiagramInspection.settled(document: document)
        await click(textPoint(of: readingView, in: window), window: window)
        _ = await waitUntil(5) { window.firstResponder === readingView }
        try await bridge.emit("reading", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        lastEvent = await press(6, flags: .maskCommand)
        try? await Task.sleep(for: .milliseconds(800))
        try await bridge.emit("after_reading_undo", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        lastEvent = await press(14, flags: .maskCommand)
        _ = await waitUntil(5) { !document.reading }
        guard let content = window.contentView,
              let terminal = firstView(named: "HortusTerminalView", in: content) else {
            result["status"] = "environment_failed"
            return result
        }
        await click(center(of: terminal, in: window), window: window)
        _ = await waitUntil(5) { window.firstResponder === terminal }
        try await bridge.emit("terminal", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        lastEvent = await press(6, flags: .maskCommand)
        try? await Task.sleep(for: .milliseconds(800))
        try await bridge.emit("after_terminal_undo", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))

        lastEvent = await press(3, flags: [.maskCommand, .maskShift])
        guard let content = window.contentView,
              await waitUntil(5, { firstView(of: NativeSearchField.self, in: content)?.isHiddenOrHasHiddenAncestor == false }) else {
            result["status"] = "environment_failed"
            return result
        }
        guard let field = firstView(of: NativeSearchField.self, in: content) else {
            result["status"] = "environment_failed"
            return result
        }
        let latin = selectInputSource(ABCInputSourceID)
        result["search_input_source_selected"] = latin.selected
        guard latin.available, latin.selected else {
            result["status"] = "environment_failed"
            return result
        }
        await click(center(of: field, in: window), window: window)
        _ = await waitUntil(5) { field.currentEditor() != nil }
        await typeText("note")
        _ = await waitUntil(5) { workspace.searchQuery == "note" }
        let searchText = workspace.searchQuery
        try await bridge.emit("search_typed", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        lastEvent = await press(6, flags: .maskCommand)
        _ = await waitUntil(3) { workspace.searchQuery != searchText }
        try await bridge.emit("after_search_undo", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        lastEvent = await press(6, flags: [.maskCommand, .maskShift])
        _ = await waitUntil(3) { workspace.searchQuery == searchText }
        try await bridge.emit("after_search_redo", state: snapshot(workspace: workspace, window: window, lastEvent: lastEvent))
        result["status"] = "completed"
        result["phase_count"] = bridge.phaseCount
        result["ack_count"] = bridge.ackCount
        result["input_source_restored"] = restoreInputSource(originalInputSource)
        result["input_source_after_run"] = selectedInputSourceID() ?? ""
        return result
    }

    private static func snapshot(workspace: Workspace, window: NSWindow, lastEvent: [String: Any]) -> [String: Any] {
        let document = workspace.current?.document
        let view = document.flatMap(DiagramInspection.current(document:))
        let responder = window.firstResponder
        let responderText = responder as? NSTextView
        let currentUndo = responder?.undoManager
        let noteUndo = view?.undoManager
        NSApp.mainMenu?.update()
        let items = undoMenuItems(NSApp.mainMenu)
        let editable = responderText?.isEditable ?? false
        let marked = responderText?.hasMarkedText() ?? false
        let canUndo = currentUndo?.canUndo ?? false
        let canRedo = currentUndo?.canRedo ?? false
        var state: [String: Any] = [
            "last_event": lastEvent,
            "first_responder": responder.map { String(describing: type(of: $0)) } ?? "",
            "responder_chain": responderChain(responder),
            "key_window": window.isKeyWindow,
            "main_active": NSApp.isActive,
            "current_editable": editable,
            "current_marked_text": marked,
            "current_text_length": responderText?.string.utf16.count ?? 0,
            "current_text": responderText?.string ?? "",
            "undo_manager_class": currentUndo.map { String(describing: type(of: $0)) } ?? "",
            "undo_manager_id": currentUndo.map { String(describing: ObjectIdentifier($0)) } ?? "",
            "can_undo": canUndo,
            "can_redo": canRedo,
            "menu_undo_enabled": items.undo?.isEnabled ?? false,
            "menu_redo_enabled": items.redo?.isEnabled ?? false,
            "menu_undo_title": items.undo?.title ?? "",
            "menu_redo_title": items.redo?.title ?? "",
            "menu_undo_action": items.undo?.action.map(NSStringFromSelector) ?? "",
            "menu_redo_action": items.redo?.action.map(NSStringFromSelector) ?? "",
            "menu_undo_target": items.undo?.target.map { String(describing: type(of: $0)) } ?? "",
            "menu_redo_target": items.redo?.target.map { String(describing: type(of: $0)) } ?? "",
            "expected_menu_undo": editable && !marked && canUndo,
            "expected_menu_redo": editable && !marked && canRedo,
            "input_source": selectedInputSourceID() ?? "",
        ]
        if let document {
            state["reading"] = document.reading
            state["dirty"] = document.dirty
            state["marked"] = document.marked
            state["revision"] = document.revision
            state["issue"] = String(describing: document.issue)
        }
        if let view {
            let selection = view.selectedRange()
            let markedRange = view.markedRange()
            let inputContext = view.inputContext
            let currentInputContext = NSTextInputContext.current
            state["diagram_view"] = true
            state["reading_view"] = view.isReading
            state["view_editable"] = view.isEditable
            state["marked_text"] = view.hasMarkedText()
            state["composition"] = view.compositionActive
            state["view_text"] = view.string
            state["view_length"] = view.textStorage?.length ?? -1
            state["selection"] = ["location": selection.location, "length": selection.length]
            state["marked_range"] = markedRange.location == NSNotFound
                ? NSNull()
                : ["location": markedRange.location, "length": markedRange.length]
            state["input_context_id"] = inputContext.map { String(describing: ObjectIdentifier($0)) } ?? ""
            state["current_input_context_id"] = currentInputContext.map { String(describing: ObjectIdentifier($0)) } ?? ""
            state["input_context_current"] = inputContext != nil && inputContext === currentInputContext
            state["input_context_client"] = inputContext.map { String(describing: type(of: $0.client)) } ?? ""
            state["input_context_selected_source"] = inputContext?.selectedKeyboardInputSource ?? ""
            state["text_revision"] = document?.textRevision ?? 0
            state["note_undo_manager_id"] = noteUndo.map { String(describing: ObjectIdentifier($0)) } ?? ""
            state["note_can_undo"] = noteUndo?.canUndo ?? false
            state["note_can_redo"] = noteUndo?.canRedo ?? false
            state["undo_manager_is_note_view"] = currentUndo.flatMap { current in
                noteUndo.map { current === $0 }
            } ?? false
        }
        state["search_query"] = workspace.searchQuery
        if let field = window.contentView.flatMap({ firstView(of: NativeSearchField.self, in: $0) }) {
            state["search_field"] = true
            state["search_editor_active"] = field.currentEditor() != nil
            state["search_field_text"] = field.stringValue
        }
        return state
    }

    private static func noteView(in window: NSWindow) -> DiagramTextView? {
        guard let content = window.contentView else { return nil }
        return firstView(of: DiagramTextView.self, in: content)
    }

    private static func activate(_ window: NSWindow) async -> Bool {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        return await waitUntil(8) { NSApp.isActive && window.isKeyWindow }
    }

    private static func waitUntil(_ seconds: Double, _ predicate: () -> Bool) async -> Bool {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while ProcessInfo.processInfo.systemUptime < deadline {
            if predicate() { return true }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return predicate()
    }

    @discardableResult
    private static func typeText(_ word: String) async -> [String: Any] {
        let keys: [Character: CGKeyCode] = ["a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5,
            "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14,
            "r": 15, "y": 16, "t": 17, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37,
            "j": 38, "k": 40, "n": 45, "m": 46]
        var lastEvent: [String: Any] = [:]
        for character in word {
            guard let key = keys[character] else { continue }
            lastEvent = await press(key, flags: [])
        }
        return lastEvent
    }

    @discardableResult
    private static func press(_ key: CGKeyCode, flags: CGEventFlags) async -> [String: Any] {
        let source = CGEventSource(stateID: .hidSystemState)
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: down)
            event?.flags = flags
            event?.post(tap: .cghidEventTap)
            try? await Task.sleep(for: .milliseconds(down ? 18 : 55))
        }
        return ["kind": "keyboard", "keycode": Int(key), "flags": flagNames(flags), "tap": "cghidEventTap"]
    }

    private static func click(_ point: CGPoint, window: NSWindow) async {
        let screen = window.convertPoint(toScreen: point)
        let height = NSScreen.screens.max { $0.frame.maxY < $1.frame.maxY }?.frame.maxY ?? NSScreen.main?.frame.height ?? 0
        let location = CGPoint(x: screen.x, y: height - screen.y)
        let source = CGEventSource(stateID: .hidSystemState)
        for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
            let event = CGEvent(mouseEventSource: source, mouseType: type,
                                mouseCursorPosition: location, mouseButton: .left)
            event?.flags = []
            event?.post(tap: .cghidEventTap)
            try? await Task.sleep(for: .milliseconds(30))
        }
    }

    private static func click(accessibilityFrame: NSRect, window: NSWindow) async {
        let point = NSPoint(x: accessibilityFrame.midX, y: accessibilityFrame.midY)
        let top = NSScreen.screens.max { $0.frame.maxY < $1.frame.maxY }?.frame.maxY
            ?? NSScreen.main?.frame.height ?? 0
        let location = CGPoint(x: point.x, y: top - point.y)
        let source = CGEventSource(stateID: .hidSystemState)
        for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
            let event = CGEvent(mouseEventSource: source, mouseType: type,
                                mouseCursorPosition: location, mouseButton: .left)
            event?.flags = []
            event?.post(tap: .cghidEventTap)
            try? await Task.sleep(for: .milliseconds(30))
        }
    }

    private static func center(of view: NSView, in window: NSWindow) -> CGPoint {
        let bounds = view.convert(view.bounds, to: nil)
        return CGPoint(x: bounds.midX, y: bounds.midY)
    }

    private static func textPoint(of view: NSTextView, in window: NSWindow) -> CGPoint {
        let range = NSRange(location: 0, length: min(1, view.string.utf16.count))
        if range.length == 1 {
            var actual = NSRange()
            let screen = view.firstRect(forCharacterRange: range, actualRange: &actual)
            let point = window.convertPoint(fromScreen: screen.origin)
            return CGPoint(x: point.x + min(8, screen.width * 0.25), y: point.y + min(8, screen.height * 0.5))
        }
        return CGPoint(x: view.textContainerInset.width + 8, y: view.textContainerInset.height + 8)
    }

    private static func firstView(named name: String, in view: NSView) -> NSView? {
        if String(describing: type(of: view)) == name { return view }
        for subview in view.subviews {
            if let found = firstView(named: name, in: subview) { return found }
        }
        return nil
    }

    private static func firstView<T: NSView>(of type: T.Type, in view: NSView) -> T? {
        if let match = view as? T { return match }
        for subview in view.subviews {
            if let found = firstView(of: type, in: subview) { return found }
        }
        return nil
    }

    private static let SCIMInputSourceID = "com.apple.inputmethod.SCIM.ITABC"
    private static let ABCInputSourceID = "com.apple.keylayout.ABC"

    private static func inputSources() -> [TISInputSource] {
        (TISCreateInputSourceList(nil, false)?.takeRetainedValue() as? [TISInputSource]) ?? []
    }

    private static func inputSourceID(_ source: TISInputSource) -> String? {
        guard let pointer = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return nil }
        return Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String
    }

    private static func inputSourceBool(_ source: TISInputSource, _ key: CFString) -> Bool {
        guard let pointer = TISGetInputSourceProperty(source, key) else { return false }
        return (Unmanaged<CFBoolean>.fromOpaque(pointer).takeUnretainedValue() as NSNumber).boolValue
    }

    private static func selectedInputSourceID() -> String? {
        inputSources().first { inputSourceBool($0, kTISPropertyInputSourceIsSelected) }.flatMap(inputSourceID)
    }

    private static func selectSCIM() -> (available: Bool, selected: Bool, status: Int32) {
        selectInputSource(SCIMInputSourceID)
    }

    private static func selectInputSource(_ identifier: String) -> (available: Bool, selected: Bool, status: Int32) {
        guard let source = inputSources().first(where: { inputSourceID($0) == identifier }) else {
            return (false, false, -1)
        }
        let available = inputSourceBool(source, kTISPropertyInputSourceIsEnabled)
            && inputSourceBool(source, kTISPropertyInputSourceIsSelectCapable)
        let status = TISSelectInputSource(source)
        usleep(250_000)
        return (available, selectedInputSourceID() == identifier, status)
    }

    private static func restoreInputSource(_ identifier: String?) -> Bool {
        guard let identifier,
              let source = inputSources().first(where: { inputSourceID($0) == identifier }) else { return false }
        let status = TISSelectInputSource(source)
        usleep(250_000)
        return status == 0 && selectedInputSourceID() == identifier
    }

    private static func flagNames(_ flags: CGEventFlags) -> [String] {
        var names: [String] = []
        if flags.contains(.maskCommand) { names.append("command") }
        if flags.contains(.maskShift) { names.append("shift") }
        if flags.contains(.maskControl) { names.append("control") }
        if flags.contains(.maskAlternate) { names.append("option") }
        return names
    }

    private static func responderChain(_ responder: NSResponder?) -> String {
        var names: [String] = []
        var current = responder
        for _ in 0..<12 {
            guard let next = current else { break }
            names.append(String(describing: type(of: next)))
            current = next.nextResponder
        }
        return names.joined(separator: ">")
    }

    private static func undoMenuItems(_ menu: NSMenu?) -> (undo: NSMenuItem?, redo: NSMenuItem?) {
        var undo: NSMenuItem?
        var redo: NSMenuItem?
        func visit(_ menu: NSMenu) {
            for item in menu.items {
                if item.keyEquivalent.lowercased() == "z" {
                    if item.keyEquivalentModifierMask == .command { undo = undo ?? item }
                    if item.keyEquivalentModifierMask == [.command, .shift] { redo = redo ?? item }
                }
                if let submenu = item.submenu { visit(submenu) }
            }
        }
        if let menu { visit(menu) }
        return (undo, redo)
    }

    @MainActor
    private final class PhaseBridge {
        private let runID: String
        private let file: FileHandle
        private let ackDirectory: URL
        private(set) var phaseCount = 0
        private(set) var ackCount = 0

        init(arguments: [String]) throws {
            func argument(_ prefix: String) -> String? {
                arguments.first(where: { $0.hasPrefix(prefix) }).map { String($0.dropFirst(prefix.count)) }
            }
            guard let runID = argument("--edit-run-id="),
                  let logPath = argument("--edit-phase-log="),
                  let ackPath = argument("--edit-ack-dir=") else { throw CocoaError(.coderInvalidValue) }
            let logURL = URL(fileURLWithPath: logPath)
            ackDirectory = URL(fileURLWithPath: ackPath, isDirectory: true)
            self.runID = runID
            FileManager.default.createFile(atPath: logURL.path, contents: nil)
            guard let file = FileHandle(forWritingAtPath: logURL.path) else { throw CocoaError(.fileWriteUnknown) }
            self.file = file
            try FileManager.default.createDirectory(at: ackDirectory, withIntermediateDirectories: true)
        }

        func emit(_ phase: String, state: [String: Any]) async throws {
            phaseCount += 1
            let sequence = phaseCount
            let line: [String: Any] = ["kind": "phase", "run_id": runID, "sequence": sequence,
                                       "phase": phase, "t": ProcessInfo.processInfo.systemUptime, "state": state]
            let data = try JSONSerialization.data(withJSONObject: line, options: [.sortedKeys])
            try file.write(contentsOf: data + Data([0x0a]))
            try file.synchronize()
            let ack = ackDirectory.appendingPathComponent(String(format: "%03d.json", sequence))
            let deadline = ProcessInfo.processInfo.systemUptime + 30
            while !FileManager.default.fileExists(atPath: ack.path) {
                guard ProcessInfo.processInfo.systemUptime < deadline else {
                    throw NSError(domain: "EditingInspection", code: 2,
                                  userInfo: [NSLocalizedDescriptionKey: "等待外部 ack 超时：\(phase)"])
                }
                try await Task.sleep(for: .milliseconds(50))
            }
            let ackData = try Data(contentsOf: ack)
            guard let value = try JSONSerialization.jsonObject(with: ackData) as? [String: Any],
                  value["run_id"] as? String == runID,
                  value["sequence"] as? Int == sequence,
                  value["phase"] as? String == phase else { throw CocoaError(.coderInvalidValue) }
            ackCount += 1
        }

        func close() { try? file.close() }
    }
    private static func diskMatches(_ document: NoteDocument, _ view: DiagramTextView) throws -> Bool {
        try String(contentsOf: document.io.url, encoding: .utf8) == view.string
    }
    private static func saveItem(_ menu: NSMenu?) -> NSMenuItem? {
        for item in menu?.items ?? [] {
            if item.keyEquivalent == "s", item.keyEquivalentModifierMask == .command { return item }
            if let nested = saveItem(item.submenu) { return nested }
        }
        return nil
    }
    private static func wait(_ predicate: () -> Bool) async throws {
        for _ in 0..<500 {
            if predicate() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        throw CocoaError(.coderValueNotFound)
    }
}
