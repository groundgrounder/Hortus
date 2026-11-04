import AppKit
import HortusCore

/// 真实窗口验收；仅在显式命令行参数下触发，不给日常运行增加轮询。
@MainActor
enum EditingInspection {
    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        guard let document = workspace.current?.document, let path = workspace.selected else { throw CocoaError(.coderValueNotFound) }
        let mode = arguments.first(where: { $0.hasPrefix("--edit-mode=") }).map { String($0.dropFirst("--edit-mode=".count)) } ?? "lifecycle"
        // 观察模式不发送任何编辑、焦点或保存动作；状态只由外部的真实键盘和鼠标驱动。
        if mode == "observe" { return try await observe(workspace: workspace, arguments: arguments) }
        // 组合在焦点离开编辑器后的保存与快捷键回归：点击和 ⌘E 是真实 CGEvent。
        if mode == "composition-focus-loss" { return try await compositionFocusLoss(workspace: workspace) }
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
                    "style_phases": stylePhases]
        }
        view.insertText("自动保存😀", replacementRange: NSRange(location: 0, length: 0))
        let autosaveStart = ProcessInfo.processInfo.systemUptime
        try await wait { !document.dirty }
        let autosaveDelay = ProcessInfo.processInfo.systemUptime - autosaveStart
        fputs("编辑验收：自动保存完成\n", stderr)
        if mode == "idle" { return ["saved": true, "autosave_delay_seconds": autosaveDelay] }
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
                "dirty_before_exit": workspace.current?.document?.dirty == true]
    }
    /// 只读取主线程状态并按变化追加 JSONL；不发起任何编辑、焦点、菜单刷新或保存动作。
    /// 真实按键由外部探针通过 CGEvent 发送，本模式只提供同一时钟下的内部状态对照。
    private static func observe(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        guard let argument = arguments.first(where: { $0.hasPrefix("--observe-log=") }) else { throw CocoaError(.coderInvalidValue) }
        let path = String(argument.dropFirst("--observe-log=".count))
        let seconds = arguments.first(where: { $0.hasPrefix("--observe-seconds=") })
            .flatMap { Double($0.dropFirst("--observe-seconds=".count)) } ?? 20
        let salt = arguments.first(where: { $0.hasPrefix("--observe-salt=") })
            .map { String($0.dropFirst("--observe-salt=".count)) } ?? ""
        FileManager.default.createFile(atPath: path, contents: nil)
        guard let file = FileHandle(forWritingAtPath: path) else { throw CocoaError(.fileWriteUnknown) }
        defer { try? file.close() }
        var written = 0
        var last: [String: Any] = [:]
        let started = ProcessInfo.processInfo.systemUptime
        var ticks = 0
        // 先写一行起点，外部探针据此判断检查器已经接管。
        try append(["kind": "start", "t": started, "seconds": seconds], to: file)
        written += 1
        while ProcessInfo.processInfo.systemUptime - started < seconds {
            ticks += 1
            if var sample = sample(workspace: workspace, salt: salt) {
                sample.removeValue(forKey: "t")
                if sample as NSDictionary != last as NSDictionary {
                    var line = sample
                    line["kind"] = "state"
                    line["t"] = ProcessInfo.processInfo.systemUptime
                    try append(line, to: file)
                    written += 1
                    last = sample
                }
            }
            try await Task.sleep(for: .milliseconds(50))
        }
        let ended = ProcessInfo.processInfo.systemUptime
        try append(["kind": "end", "t": ended, "ticks": ticks], to: file)
        written += 1
        return ["observe_log": path, "samples": written, "ticks": ticks, "seconds": ended - started,
                "document_reading": workspace.current?.document?.reading == true,
                "document_dirty": workspace.current?.document?.dirty == true,
                "document_marked": workspace.current?.document?.marked == true]
    }
    private static func sample(workspace: Workspace, salt: String) -> [String: Any]? {
        guard let document = workspace.current?.document else { return nil }
        let view = DiagramInspection.current(document: document)
        let window = NSApp.keyWindow ?? NSApp.windows.first(where: \.isVisible)
        var sample: [String: Any] = [
            "tabs": workspace.tabs.count,
            "selected_index": workspace.tabs.firstIndex(where: { $0.path == workspace.selected }) ?? -1,
            "reading": document.reading,
            "dirty": document.dirty,
            "marked": document.marked,
            "issue": String(describing: document.issue),
            "message": document.message,
            "persistence": String(describing: document.persistence),
            "revision": document.revision,
            "text_revision": document.textRevision,
            "read_only": document.readOnlyReason ?? "",
            "key_window": window?.isKeyWindow == true,
            "main_active": NSApp.isActive,
            "first_responder": window.flatMap { $0.firstResponder.map { String(describing: type(of: $0)) } } ?? "",
            "save_enabled": saveItem(NSApp.mainMenu)?.isEnabled ?? false,
            "save_action": saveItem(NSApp.mainMenu)?.action.map(NSStringFromSelector) ?? "",
        ]
        // 只解析当前响应者链上的命令目标，不触发菜单校验或刷新。
        sample["responder_chain"] = chain(window?.firstResponder)
        for (name, selector) in [("undo", "undo:"), ("copy", "copy:"), ("select_all", "selectAll:"),
                                 ("paste", "paste:"), ("insert_newline", "insertNewline:"),
                                 ("toggle_sidebar", "toggleSidebar:")] {
            sample["target_\(name)"] = NSApp.target(forAction: Selector(selector), to: nil, from: nil)
                .map { String(describing: type(of: $0)) } ?? ""
        }
        if let view {
            sample["editable"] = view.isEditable
            sample["marked_text"] = view.hasMarkedText()
            sample["composition"] = view.compositionActive
            sample["view_length"] = view.textStorage?.length ?? -1
            sample["view_hash"] = digest(view.string, salt: salt)
            sample["blocks"] = view.blocks.count
            sample["pending"] = view.diagramPending
            sample["is_reading"] = view.isReading
            sample["selection_length"] = view.selectedRange().length
            sample["can_undo"] = view.undoManager?.canUndo ?? false
            sample["can_redo"] = view.undoManager?.canRedo ?? false
            sample["undo_groups"] = view.undoManager?.undoCount ?? 0
        }
        for key in ["z", "a", "c", "v", "x"] {
            let item = menuItem(key, NSApp.mainMenu)
            sample["menu_\(key)_title"] = item?.title ?? ""
            sample["menu_\(key)_enabled"] = item?.isEnabled ?? false
            sample["menu_\(key)_action"] = item?.action.map(NSStringFromSelector) ?? ""
        }
        if let item = toggleItem(NSApp.mainMenu) {
            sample["switch_enabled"] = item.isEnabled
            sample["switch_title"] = item.title
            sample["switch_action"] = item.action.map(NSStringFromSelector) ?? ""
        }
        let url = document.io.url
        if let attributes = try? FileManager.default.attributesOfItem(atPath: url.path) {
            sample["disk_bytes"] = (attributes[.size] as? NSNumber)?.intValue ?? -1
            sample["disk_mtime"] = (attributes[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
        } else {
            sample["disk_bytes"] = -1
        }
        sample["disk_hash"] = (try? String(contentsOf: url, encoding: .utf8)).map { digest($0, salt: salt) } ?? "missing"
        return sample
    }
    private static func chain(_ responder: NSResponder?) -> String {
        var names: [String] = []
        var current: NSResponder? = responder
        for _ in 0..<12 {
            guard let next = current else { break }
            names.append(String(describing: type(of: next)))
            current = next.nextResponder
        }
        return names.joined(separator: ">")
    }
    private static func append(_ line: [String: Any], to file: FileHandle) throws {
        let data = try JSONSerialization.data(withJSONObject: line, options: [.sortedKeys])
        try file.write(contentsOf: data + Data([0x0a]))
        try file.synchronize()
    }
    /// 只用于比较两段正文是否相同；私人根内的日志才保存结果。
    private static func digest(_ text: String, salt: String) -> String {
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in (salt + "\u{0}" + text).utf8 {
            hash = (hash ^ UInt64(byte)) &* 0x100000001b3
        }
        return String(format: "%016llx", hash)
    }
    private static func menuItem(_ key: String, _ menu: NSMenu?) -> NSMenuItem? {
        for item in menu?.items ?? [] {
            if item.keyEquivalent == key, item.keyEquivalentModifierMask == .command { return item }
            if let nested = menuItem(key, item.submenu) { return nested }
        }
        return nil
    }
    private static func toggleItem(_ menu: NSMenu?) -> NSMenuItem? {
        for item in menu?.items ?? [] {
            if item.keyEquivalent == "e", item.keyEquivalentModifierMask == .command { return item }
            if let nested = toggleItem(item.submenu) { return nested }
        }
        return nil
    }
    /// 真实点击编辑器建立焦点，由输入法入口建立组合，再真实点击底部终端让焦点离开。
    /// 断言只落在用户可观察结果：是否结束组合、是否完成保存、磁盘是否更新、⌘E 是否生效。
    private static func compositionFocusLoss(workspace: Workspace) async throws -> [String: Any] {
        var result: [String: Any] = ["post_event_access": CGPreflightPostEventAccess(),
                                     "windows": NSApp.windows.map { "\($0.className)|visible=\($0.isVisible)|key=\($0.isKeyWindow)" }]
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.isKeyWindow })
                ?? NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else {
            result["window"] = "missing"
            return result
        }
        var terminal: NSView?
        for _ in 0..<60 {
            if let content = window.contentView { terminal = firstView(named: "HortusTerminalView", in: content) }
            if terminal != nil { break }
            try? await Task.sleep(for: .milliseconds(50))
        }
        result["terminal_found"] = terminal != nil
        guard let terminal, let document = workspace.current?.document else {
            result["document"] = workspace.current?.document != nil
            return result
        }
        let view = try await DiagramInspection.settled(document: document)
        result["terminal_visible"] = !terminal.isHiddenOrHasHiddenAncestor
        // 背景窗口第一次点击只用于激活，之后重复一次确保落到目标视图。
        for _ in 0..<3 {
            click(center(of: view, in: window), window: window)
            try? await Task.sleep(for: .milliseconds(250))
            if window.firstResponder === view { break }
        }
        result["focused"] = window.firstResponder === view
        guard window.firstResponder === view else { return result }
        let before = try String(contentsOf: document.io.url, encoding: .utf8)
        // 输入法入口：与真实输入法相同，组合只写进原生存储并由文本通知进入文档状态。
        let selection = view.selectedRange()
        view.setMarkedText("nihao", selectedRange: NSRange(location: 5, length: 0), replacementRange: selection)
        try? await Task.sleep(for: .milliseconds(200))
        result["composing"] = view.hasMarkedText()
        result["marked_during_composition"] = document.marked
        result["dirty_during_composition"] = document.dirty
        guard view.hasMarkedText() else { return result }
        // 组合进行中保存与切换必须仍被阻止，修复不能靠放开这道保护通过。
        attemptShortcut(14, flags: .maskCommand)
        try? await Task.sleep(for: .milliseconds(400))
        result["save_blocked_while_composing"] = document.dirty && document.marked
        result["toggle_blocked_while_composing"] = !document.reading
        // 真实点击底部终端：焦点离开编辑器。
        click(center(of: terminal, in: window), window: window)
        try? await Task.sleep(for: .milliseconds(300))
        result["focus_left_editor"] = window.firstResponder !== view
        // 自动保存窗口：1 秒防抖 + 余量。
        try? await Task.sleep(for: .seconds(2))
        result["marked_cleared"] = !document.marked && !view.hasMarkedText()
        result["save_completed"] = !document.dirty && document.issue == .none
        let after = try String(contentsOf: document.io.url, encoding: .utf8)
        result["disk_updated"] = after != before && after.contains("nihao")
        result["disk_matches_editor"] = after == view.string
        // 真实 ⌘E：保存成功后才允许切换视图。
        attemptShortcut(14, flags: .maskCommand)
        try? await Task.sleep(for: .milliseconds(800))
        result["toggle_worked"] = document.reading
        result["issue"] = String(describing: document.issue)
        return result
    }
    private static func attemptShortcut(_ key: CGKeyCode, flags: CGEventFlags) {
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: CGEventSource(stateID: .hidSystemState), virtualKey: key, keyDown: down)
            event?.flags = flags
            event?.post(tap: .cghidEventTap)
            usleep(20_000)
        }
    }
    private static func click(_ point: CGPoint, window: NSWindow) {
        let screen = window.convertPoint(toScreen: point)
        // CGEvent 的坐标原点在主显示器左上角；窗口坐标的 y 轴向上。
        let height = NSScreen.screens.max { $0.frame.maxY < $1.frame.maxY }?.frame.maxY ?? NSScreen.main?.frame.height ?? 0
        let location = CGPoint(x: screen.x, y: height - screen.y)
        for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
            CGEvent(mouseEventSource: CGEventSource(stateID: .hidSystemState), mouseType: type,
                    mouseCursorPosition: location, mouseButton: .left)?.post(tap: .cghidEventTap)
            usleep(25_000)
        }
    }
    private static func center(of view: NSView, in window: NSWindow) -> CGPoint {
        let bounds = view.convert(view.bounds, to: nil)
        return CGPoint(x: bounds.midX, y: bounds.midY)
    }
    private static func firstView(named name: String, in view: NSView) -> NSView? {
        if String(describing: type(of: view)) == name { return view }
        for subview in view.subviews {
            if let found = firstView(named: name, in: subview) { return found }
        }
        return nil
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
