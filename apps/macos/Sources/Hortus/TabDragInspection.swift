#if DEBUG
import AppKit
import Darwin
import ScreenCaptureKit

/// 以 CGEvent 驱动窗口标签操作并核对释放计数。
@MainActor
enum TabDragInspection {
    private static var stage = "start"
    static func run(_ application: ApplicationWorkspace, output: String) async throws {
        var result: [String: Any] = [:]
        var checks: [String: Bool] = [:]
        func persistEvidence() {
            result["stage"] = stage; result["checks"] = checks
            result["drag_trace"] = application.tabDragTrace
            result["final_windows"] = application.records.map(describe)
            if let data = try? JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: URL(fileURLWithPath: output))
            }
        }
        defer { persistEvidence() }
        func check(_ name: String, _ value: Bool) throws {
            checks[name] = value
            if !value { fputs("Tab drag check failed: \(stage): \(name)\n", stderr); throw CocoaError(.coderValueNotFound) }
        }
        stage = "settle"
        try await settled(application)
        let args = ProcessInfo.processInfo.arguments
        if args.contains("--tab-drag-restore") {
            result["windows"] = application.records.map(describe)
            result["active_id"] = application.activeID
            result["session"] = try JSONSerialization.jsonObject(with: JSONEncoder().encode(application.snapshot()))
            result["diagnostics"] = application.restorationDiagnostics
            result["contexts"] = application.contexts.values.map { ["windows": $0.windowCount, "listeners": $0.listenerCount] }
            try await screenshots(application.records, args: args, name: "restored")
            if args.contains("--tab-drag-close-restored"), let last = application.records.last {
                application.requestClose(last)
                try await wait { application.records.count == 1 }
                await application.persistSession()
                result["after_close"] = application.records.map(describe)
            }
            stage = "restored"
            persistEvidence()
            NSApp.terminate(nil)
            return
        }
        let first = application.initial
        guard let window = first.window, let screen = NSScreen.main?.visibleFrame,
              let root = first.workspace.root, let context = first.workspace.context,
              let document = first.workspace.current?.document else { throw CocoaError(.coderValueNotFound) }
        first.workspace.openNote("stay.md")
        try await settled(application)
        first.workspace.openNote("move.md")
        document.reading = false
        window.setFrame(NSRect(x: screen.minX, y: screen.maxY - 650, width: 900, height: 650), display: true)
        application.activate(first)
        try await wait { first.workspace.textView?.isEditable == true }
        first.workspace.openNote("stay.md")
        try await click(first, path: "move.md")
        try check("click_selects_tab", first.workspace.selected == "move.md")
        try await settled(application)
        result["diagram_before_drag"] = diagramState(first)
        let outside = CGPoint(x: screen.maxX - 35, y: screen.minY + 35)
        stage = "source drop"
        try await drag(first, path: "move.md", to: window.frame.center)
        try check("source_drop_noop", application.records.count == 1 && first.workspace.tabs.count == 2)
        stage = "escape"
        try await drag(first, path: "move.md", to: outside, escape: true)
        try check("escape_noop", application.records.count == 1 && first.workspace.tabs.count == 2 && application.tabDrags.isEmpty)
        stage = "system cancel"
        try await drag(first, path: "move.md", to: outside, systemCancel: true)
        try check("system_cancel_noop", application.records.count == 1 && first.workspace.tabs.count == 2 && application.tabDrags.isEmpty)
        stage = "marked"
        document.marked = true
        try await drag(first, path: "move.md", to: outside)
        try check("marked_noop", application.records.count == 1 && first.workspace.tabs.count == 2)
        document.compositionChanged(false)
        let loadingPath = "loading.md"
        first.workspace.tabs.append(NoteTab(path: loadingPath))
        try check("loading_rejected", application.beginTabDrag(workspace: first.workspace, path: loadingPath) == nil)
        first.workspace.tabs.removeAll { $0.path == loadingPath }
        stage = "dirty real drag"
        guard let native = first.workspace.textView else { throw CocoaError(.coderValueNotFound) }
        let originalText = native.string
        window.makeFirstResponder(native)
        native.setSelectedRange(NSRange(location: 2, length: 0))
        native.insertText("Moved unsaved ", replacementRange: native.selectedRange())
        native.undoManager?.endUndoGroupingIfNeededForInspection()
        let revision = document.revision
        document.sourceAnchor = 2; document.sourceSelectionLength = 3
        result["before"] = documentState(document)
        try check("dirty_before_drag", document.dirty)
        try await drag(first, path: "move.md", to: outside, fast: true, repeatUp: true)
        try await wait { application.records.count == 2 && application.records.last?.workspace.textView != nil }
        guard let second = application.records.last else { throw CocoaError(.coderValueNotFound) }
        let moved = second.workspace.current?.document
        result["after"] = documentState(document)
        try check("same_objects", moved === document && moved?.io === document.io && moved?.io.session === document.io.session)
        try check("same_native_editor", second.workspace.textView === native && document.viewOwner === native.editingCoordinator)
        try check("body_revision_preserved", native.string == originalText.replacingCharacters(in: originalText.index(originalText.startIndex, offsetBy: 2)..<originalText.index(originalText.startIndex, offsetBy: 2), with: "Moved unsaved ") && document.revision == revision)
        try check("source_removed_target_selected", first.workspace.tabs.map(\.path) == ["stay.md"] && second.workspace.selected == "move.md")
        try check("shared_context", first.workspace.context === second.workspace.context && context.windowCount == 2 && context.listenerCount == 1 && application.contexts.count == 1)
        try check("repeated_mouse_up_noop", application.records.count == 2 && application.tabDrags.isEmpty)
        // 同一 NSTextView 自己的 UndoManager 随移动保留。
        stage = "undo save callbacks"
        native.undoManager?.undo()
        try await Task.sleep(for: .milliseconds(100))
        try check("undo_preserved", native.string == originalText)
        native.undoManager?.redo()
        await document.saveNow()
        try check("target_save", !document.dirty && document.persistence == .persisted && (try String(contentsOfFile: root + "/move.md", encoding: .utf8)) == native.string)
        try check("callbacks_rebound", document.onReload != nil && document.onClosing != nil && document.onStateChange != nil && document.onCopy != nil)
        second.window?.setFrame(NSRect(x: screen.maxX - 900, y: screen.minY, width: 900, height: 650), display: true)
        stage = "existing window drop"
        try await drag(first, path: "stay.md", to: second.window!.frame.center)
        try check("other_window_drop_noop", application.records.count == 2 && first.workspace.tabs.map(\.path) == ["stay.md"])
        stage = "routing"
        first.workspace.presentQuickOpen()
        first.workspace.updateQuickOpen(query: "move", marked: false)
        await first.workspace.waitForSearchTasks()
        first.workspace.activateQuickOpenSelection()
        try check("quick_open_route", application.activeID == second.id && first.workspace.tabs.count == 1)
        first.workspace.openSearchResult(path: "move.md", start: 2, length: 4)
        try check("search_range_route", application.activeID == second.id && document.sourceAnchor == 2 && document.sourceSelectionLength == 4)
        try await wait { document.outlineRevision == document.revision && !document.headings.isEmpty }
        if let heading = document.headings.first {
            first.workspace.revealHeading(heading, path: "move.md")
            try check("heading_route", application.activeID == second.id && document.sourceAnchor == Int(heading.start))
        }
        // 普通打开和副本回调都应使用当前所有者。
        first.workspace.openNote("move.md")
        try check("common_open_route", application.activeID == second.id && first.workspace.tabs.count == 1)
        document.onCopy?("copy.md")
        try await settled(application)
        try check("copy_callback_destination", second.workspace.tabs.map(\.path) == ["move.md", "copy.md"] && first.workspace.tabs.count == 1)
        try await click(second, path: "copy.md", close: true)
        try await wait { second.workspace.tabs.count == 1 }
        try check("close_button_works", second.workspace.tabs.map(\.path) == ["move.md"])
        stage = "moved conflict copy"
        try await wait { second.workspace.textView?.isEditable == true }
        guard let conflictNative = second.workspace.textView else { throw CocoaError(.coderValueNotFound) }
        let diskText = conflictNative.string + "\nDisk conflict fixture.\n"
        conflictNative.insertText("Local conflict fixture. ", replacementRange: NSRange(location: 0, length: 0))
        try diskText.write(toFile: root + "/move.md", atomically: true, encoding: .utf8)
        await document.saveNow()
        try check("moved_conflict_detected", document.issue == .conflict)
        await document.saveNow(.conflictCopy)
        try await settled(application)
        guard let copy = second.workspace.tabs.first(where: { $0.path != "move.md" }) else { throw CocoaError(.coderValueNotFound) }
        result["conflict_copy"] = copy.path
        try check("real_conflict_copy_destination", first.workspace.tabs.map(\.path) == ["stay.md"] && (try String(contentsOfFile: root + "/" + copy.path, encoding: .utf8)).contains("Local conflict fixture."))
        first.workspace.openNote(copy.path)
        try check("conflict_copy_unique_route", application.activeID == second.id && first.workspace.tabs.count == 1 && second.workspace.tabs.count == 2)
        second.workspace.closeTab(copy.path)
        try await wait { second.workspace.tabs.count == 1 }
        second.workspace.openNote("move.md")
        application.activate(first); application.activate(second)
        await first.workspace.openFolder(root + "/.")
        try check("folder_mru", application.activeID == second.id && application.records.count == 2)
        stage = "external events"
        let sourceRefresh = first.workspace.current?.document?.refreshStartedAt
        let started = ProcessInfo.processInfo.systemUptime
        try (document.text + "\nExternal shared update.\n").write(toFile: root + "/move.md", atomically: true, encoding: .utf8)
        try "# Shared tree\n".write(toFile: root + "/event.md", atomically: true, encoding: .utf8)
        try await wait { document.text.contains("External shared update.") && first.workspace.entries["event.md"] != nil }
        result["external_update_ms"] = (ProcessInfo.processInfo.systemUptime - started) * 1000
        try check("event_owner_only", first.workspace.current?.document?.refreshStartedAt == sourceRefresh)
        try check("tree_shared", first.workspace.treeVersion == second.workspace.treeVersion && second.workspace.entries["event.md"] != nil)
        try check("external_under_second", (result["external_update_ms"] as? Double ?? 1000) < 1000)
        stage = "terminal layout"
        for record in [first, second] { record.layout.setTerminalVisible(true) }
        try await wait { first.workspace.terminal.inspectionShellPid > 0 && second.workspace.terminal.inspectionShellPid > 0 }
        let firstPID = first.workspace.terminal.inspectionShellPid, secondPID = second.workspace.terminal.inspectionShellPid
        result["terminal_pids"] = [firstPID, secondPID]
        try check("terminal_independent", firstPID != secondPID && first.workspace.terminal.root == root && second.workspace.terminal.root == root)
        let originalBuffer = terminalText(first.workspace.terminal)
        second.workspace.terminal.clearScreen()
        second.workspace.terminal.focus()
        try check("terminal_clear_focus_isolated", first.workspace.terminal.inspectionShellPid == firstPID && terminalText(first.workspace.terminal) == originalBuffer)
        first.layout.visible[.left] = false
        second.layout.visible[.left] = false
        second.layout.visible[.left] = true
        first.layout.visible[.left] = true
        first.layout.visible[.left] = false
        first.layout.size[.left] = 335
        for _ in 0..<3 {
            try check("visible_snapshot_last_write", application.snapshot().recent.first { $0.path == root }?.visible?["left"] == false)
        }
        try check("visible_local", second.layout.visible[.left] == true && first.layout.visible[.left] == false)
        try check("size_existing_local", second.layout.size[.left] != 335)
        result["visible_write_order"] = ["first=false", "second=false", "second=true", "first=true", "first=false"]
        try await screenshots([first, second], args: args, name: "two-windows")
        stage = "lifecycle"
        var cycles: [[String: Any]] = []
        for index in 0..<20 {
            let probes = try await cycle(application, source: first, point: NSPoint(x: screen.maxX - 35, y: screen.maxY - 35), realDrag: index < 2)
            try await Task.sleep(for: .milliseconds(160))
            cycles.append(["index": index, "real_drag": index < 2, "window_released": probes.window.value == nil,
                           "workspace_released": probes.workspace.value == nil, "record_released": probes.record.value == nil,
                           "shell_exited": kill(probes.pid, 0) != 0, "pid": probes.pid,
                           "listeners": context.listenerCount, "windows": context.windowCount,
                           "open_threads": probes.threads, "closed_threads": try WorkspaceInspection.threadCount()])
        }
        result["cycles"] = cycles
        try check("twenty_cycles_released", cycles.allSatisfy { ($0["window_released"] as? Bool) == true && ($0["workspace_released"] as? Bool) == true && ($0["record_released"] as? Bool) == true && ($0["shell_exited"] as? Bool) == true && ($0["listeners"] as? Int) == 1 && ($0["windows"] as? Int) == 2 })
        let counts = cycles.compactMap { $0["closed_threads"] as? Int }
        result["thread_growth"] = (counts.suffix(5).max() ?? 0) - (counts.prefix(5).max() ?? 0)
        try check("threads_no_growth", (result["thread_growth"] as? Int ?? 10) <= 2)
        stage = "save"
        second.workspace.openNote("move.md")
        document.sourceAnchor = 2; document.sourceSelectionLength = 0
        application.activate(second)
        await application.persistSession()
        result["windows"] = application.records.map(describe)
        result["session"] = try JSONSerialization.jsonObject(with: JSONEncoder().encode(application.snapshot()))
        result["context"] = ["identity": String(describing: ObjectIdentifier(context)), "handle": String(describing: ObjectIdentifier(context.handle)), "windows": context.windowCount, "listeners": context.listenerCount]
        result["real_cgevent_drag"] = true
        stage = "complete"
        persistEvidence()
        NSApp.terminate(nil)
    }

    private final class WeakObject {
        weak var value: AnyObject?
        init(_ value: AnyObject?) { self.value = value }
    }
    private static func cycle(_ application: ApplicationWorkspace, source: WindowRecord, point: NSPoint, realDrag: Bool) async throws
        -> (window: WeakObject, workspace: WeakObject, record: WeakObject, pid: Int32, threads: Int) {
        source.workspace.openNote("cycle.md")
        try await settled(application)
        let target: WindowRecord
        if realDrag {
            try await drag(source, path: "cycle.md", to: point, fast: true, repeatUp: true)
            try await wait { application.records.count == 3 }
            guard let moved = application.records.last else { throw CocoaError(.coderValueNotFound) }
            target = moved
        } else {
            guard let payload = application.beginTabDrag(workspace: source.workspace, path: "cycle.md"),
                  let moved = application.finishTabDrag(payload, at: point) else { throw CocoaError(.coderValueNotFound) }
            target = moved
            // 已消费的令牌不能再次创建窗口。
            guard application.finishTabDrag(payload, at: point) == nil else { throw CocoaError(.coderValueNotFound) }
        }
        target.layout.setTerminalVisible(true)
        try await wait { target.window != nil && target.workspace.terminal.inspectionShellPid > 0 }
        let pid = target.workspace.terminal.inspectionShellPid
        let probes = (WeakObject(target.window), WeakObject(target.workspace), WeakObject(target), pid, try WorkspaceInspection.threadCount())
        application.requestClose(target)
        try await wait { application.records.count == 2 }
        return probes
    }
    private static func terminalText(_ session: TerminalSession) -> String {
        guard let view = session.inspectionView else { return "" }
        return view.visibleRowsText(0..<view.terminalDimensions.rows).joined(separator: "\n")
    }
    private static func documentState(_ document: NoteDocument) -> [String: Any] {
        ["document": String(describing: ObjectIdentifier(document)), "io": String(describing: ObjectIdentifier(document.io)),
         "session": String(describing: ObjectIdentifier(document.io.session)), "revision": document.revision,
         "dirty": document.dirty, "persistence": String(describing: document.persistence), "issue": String(describing: document.issue),
         "reading": document.reading, "anchor": document.sourceAnchor, "selection_length": document.sourceSelectionLength]
    }
    private static func describe(_ record: WindowRecord) -> [String: Any] {
        ["id": record.id, "root": record.workspace.root ?? "", "tabs": record.workspace.tabs.map(\.path),
         "selected": record.workspace.selected ?? "", "views": record.workspace.sessionViews(),
         "documents": record.workspace.tabs.compactMap { $0.document.map(documentState) },
         "diagrams": diagramState(record),
         "terminal_pid": record.workspace.terminal.inspectionShellPid, "terminal_root": record.workspace.terminal.root ?? "",
         "visible": Dictionary(uniqueKeysWithValues: record.layout.visible.map { ($0.key.rawValue, $0.value) }),
         "frame": record.window.map { NSStringFromRect($0.frame) } ?? ""]
    }

    private static func diagramState(_ record: WindowRecord) -> [[String: Any]] {
        guard let document = record.workspace.current?.document,
              let view = DiagramInspection.current(document: document) else { return [] }
        return view.blocks.map { block in
            let prepared = block.prepared
            let nodes = prepared?.scene.hitRegions.count ?? 0
            let edges = prepared?.scene.primitives.filter { primitive in
                if case .graphPath = primitive { return true }
                return false
            }.count ?? 0
            return ["ordinal": block.ordinal, "revision": block.revision,
                    "scene_version": block.sceneVersion, "nodes": nodes, "edges": edges,
                    "reason": block.reason ?? "", "diagnostic": block.diagnostic ?? "",
                    "signature": prepared.map { DiagramInspection.sceneSignature($0.scene) } ?? ""]
        }
    }
    private static func settled(_ application: ApplicationWorkspace) async throws {
        try await wait { application.ready && application.records.allSatisfy { $0.window != nil && !$0.workspace.scanning && !$0.workspace.tabs.contains(where: \.loading) } }
        try await Task.sleep(for: .milliseconds(120))
    }
    private static func wait(_ condition: () -> Bool) async throws {
        for _ in 0..<600 {
            if condition() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        fputs("Tab drag timeout: \(stage)\n", stderr)
        throw CocoaError(.coderValueNotFound)
    }
    private static func button(in view: NSView, path: String) -> TabDragButton.Button? {
        if let button = view as? TabDragButton.Button, button.path == path { return button }
        return view.subviews.lazy.compactMap { button(in: $0, path: path) }.first
    }
    private static func drag(_ record: WindowRecord, path: String, to point: NSPoint,
                             escape: Bool = false, fast: Bool = false, repeatUp: Bool = false, systemCancel: Bool = false) async throws {
        guard let window = record.window,
              let toolbar = window.toolbar?.items.first(where: { $0.itemIdentifier.rawValue == "editor-tabs" })?.view,
              let source = button(in: toolbar, path: path) else { throw CocoaError(.coderValueNotFound) }
        record.workspace.application?.activate(record)
        NSApp.activate(ignoringOtherApps: true)
        try await wait { NSApp.isActive && window.isKeyWindow }
        try await Task.sleep(for: .milliseconds(fast ? 20 : 100))
        let rect = window.convertToScreen(source.convert(source.bounds, to: nil))
        let from = rect.center
        await withCheckedContinuation { continuation in
            sendDrag(from: from, to: point, escape: escape, fast: fast, repeatUp: repeatUp, systemCancel: systemCancel) { continuation.resume() }
        }
        if systemCancel { record.workspace.application?.activate(record) }
        try await Task.sleep(for: .milliseconds(fast ? 80 : 160))
    }
    private static func click(_ record: WindowRecord, path: String, close: Bool = false) async throws {
        guard let window = record.window,
              let toolbar = window.toolbar?.items.first(where: { $0.itemIdentifier.rawValue == "editor-tabs" })?.view,
              let source = button(in: toolbar, path: path) else { throw CocoaError(.coderValueNotFound) }
        record.workspace.application?.activate(record)
        NSApp.activate(ignoringOtherApps: true)
        record.workspace.application?.tabDragTrace.append("activate active=\(NSApp.isActive) key=\(window.isKeyWindow) post=\(CGPreflightPostEventAccess())")
        try await wait { NSApp.isActive && window.isKeyWindow }
        try await Task.sleep(for: .milliseconds(120))
        let rect = window.convertToScreen(source.convert(source.bounds, to: nil))
        let point = NSPoint(x: close ? rect.maxX + 12 : rect.midX, y: rect.midY)
        record.workspace.application?.tabDragTrace.append("click \(path) close=\(close) active=\(NSApp.isActive) key=\(window.isKeyWindow) post=\(CGPreflightPostEventAccess()) rect=\(NSStringFromRect(rect))")
        await withCheckedContinuation { continuation in
            DispatchQueue.global().async {
                let location = CGPoint(x: point.x, y: CGDisplayBounds(CGMainDisplayID()).height - point.y)
                for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
                    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: location, mouseButton: .left)?.post(tap: .cghidEventTap)
                    Thread.sleep(forTimeInterval: 0.04)
                }
                continuation.resume()
            }
        }
        try await Task.sleep(for: .milliseconds(160))
    }
    nonisolated private static func sendDrag(from: CGPoint, to: CGPoint, escape: Bool, fast: Bool, repeatUp: Bool, systemCancel: Bool,
                                            completion: @escaping @Sendable () -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            let top = CGDisplayBounds(CGMainDisplayID()).height
            func post(_ type: CGEventType, _ point: CGPoint) {
                CGEvent(mouseEventSource: nil, mouseType: type,
                        mouseCursorPosition: CGPoint(x: point.x, y: top - point.y), mouseButton: .left)?.post(tap: .cghidEventTap)
            }
            post(.mouseMoved, from); Thread.sleep(forTimeInterval: 0.03)
            post(.leftMouseDown, from); Thread.sleep(forTimeInterval: 0.03)
            for step in 1...20 {
                if systemCancel && step == 6 {
                    let process = Process()
                    process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
                    process.arguments = ["-a", "Finder"]
                    try? process.run(); process.waitUntilExit()
                    Thread.sleep(forTimeInterval: 0.15)
                }
                let fraction = CGFloat(step) / 20
                post(.leftMouseDragged, CGPoint(x: from.x + (to.x - from.x) * fraction, y: from.y + (to.y - from.y) * fraction))
                Thread.sleep(forTimeInterval: fast ? 0.004 : 0.012)
            }
            if escape {
                CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: true)?.post(tap: .cghidEventTap)
                CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: false)?.post(tap: .cghidEventTap)
                Thread.sleep(forTimeInterval: 0.04)
            }
            post(.leftMouseUp, to)
            if repeatUp { Thread.sleep(forTimeInterval: 0.04); post(.leftMouseUp, to) }
            completion()
        }
    }
    private static func screenshots(_ records: [WindowRecord], args: [String], name: String) async throws {
        guard let arg = args.first(where: { $0.hasPrefix("--tab-drag-media=") }) else { return }
        let directory = URL(fileURLWithPath: String(arg.dropFirst("--tab-drag-media=".count)))
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let content = try await SCShareableContent.currentProcess
        let ids = Set(records.compactMap { $0.window.map { CGWindowID($0.windowNumber) } })
        guard let display = content.displays.first else { throw CocoaError(.fileWriteUnknown) }
        let filter = SCContentFilter(display: display, including: content.windows.filter { ids.contains($0.windowID) })
        let configuration = SCStreamConfiguration()
        configuration.width = display.width; configuration.height = display.height; configuration.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
        guard let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
        try data.write(to: directory.appendingPathComponent(name + ".png"))
    }
}

private extension NSRect {
    var center: NSPoint { NSPoint(x: midX, y: midY) }
}
private extension UndoManager {
    func endUndoGroupingIfNeededForInspection() {
        if groupingLevel > 0 { endUndoGrouping() }
    }
}
#endif
