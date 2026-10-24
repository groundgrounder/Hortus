import AppKit
import HortusCore

/// 真实窗口验收；仅在显式命令行参数下触发，不给日常运行增加轮询。
@MainActor
enum EditingInspection {
    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        guard let document = workspace.current?.document, let path = workspace.selected else { throw CocoaError(.coderValueNotFound) }
        if document.reading { await workspace.toggleCurrentView() }
        for _ in 0..<500 {
            if DiagramInspection.current()?.isReading == false { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        var view = try await DiagramInspection.settled()
        view.window?.makeFirstResponder(view)
        let mode = arguments.first(where: { $0.hasPrefix("--edit-mode=") }).map { String($0.dropFirst("--edit-mode=".count)) } ?? "lifecycle"
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
            view = try await DiagramInspection.settled()
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
        try await wait { DiagramInspection.current()?.representedPath == path && DiagramInspection.current()?.diagramPending == false }
        view = try await DiagramInspection.settled()
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
        try await wait { DiagramInspection.current()?.representedDocument == workspace.current?.document.map(ObjectIdentifier.init) && DiagramInspection.current()?.diagramPending == false }
        view = try await DiagramInspection.settled()
        guard let reopened = workspace.current?.document, let originalRoot = workspace.root else { throw CocoaError(.coderValueNotFound) }
        view.insertText("切换库", replacementRange: NSRange(location: 0, length: 0))
        let librarySource = view.string
        let alternate = URL(fileURLWithPath: originalRoot).deletingLastPathComponent().appendingPathComponent("alternate")
        await workspace.openFolder(alternate.path)
        let librarySave = try String(contentsOf: reopened.io.url, encoding: .utf8) == librarySource
        await workspace.openFolder(originalRoot)
        try await wait { workspace.current?.document != nil && workspace.current?.loading == false }
        try await wait { DiagramInspection.current()?.representedDocument == workspace.current?.document.map(ObjectIdentifier.init) && DiagramInspection.current()?.diagramPending == false }
        view = try await DiagramInspection.settled()
        // 给退出留下真实脏内容，由 applicationShouldTerminate 完成最后一次保存。
        view.insertText("退出保存", replacementRange: NSRange(location: 0, length: 0))
        return ["autosave_delay_seconds": autosaveDelay, "command_save": commandSave, "focus_save": focusSave,
                "switch_save": switchSave, "library_save": librarySave, "close_save": closeSaved, "external_delay_seconds": externalDelay,
                "selection_preserved": caret == selected, "undo_cleared": undoCleared,
                "dirty_before_exit": workspace.current?.document?.dirty == true]
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
