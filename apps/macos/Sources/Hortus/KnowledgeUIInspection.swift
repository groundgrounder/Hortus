#if DEBUG
import AppKit
import CryptoKit
import HortusCore

@MainActor
enum KnowledgeUIInspection {
    static let argumentPrefix = "--knowledge-ui-inspection="
    static let requested = ProcessInfo.processInfo.arguments.contains { $0.hasPrefix(argumentPrefix) }

    private static func failure(_ detail: String) -> NSError {
        NSError(domain: "HortusKnowledge", code: 1, userInfo: [NSLocalizedDescriptionKey: detail])
    }

    private static func require<T>(_ value: T?, _ label: String) throws -> T {
        guard let value else { throw failure(label) }
        return value
    }

    private static func wait(_ label: String, _ check: @MainActor () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(8)
        while !check(), ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(15))
        }
        guard check() else { throw failure("timeout: " + label) }
    }

    private static func button(_ title: String, in view: NSView) -> NSButton? {
        if let button = view as? NSButton, button.title == title { return button }
        for child in view.subviews {
            if let found = button(title, in: child) { return found }
        }
        return nil
    }

    private static func labels(in view: NSView) -> [String] {
        let text = (view as? NSTextField).map { [$0.stringValue] } ?? []
        return text + view.subviews.flatMap { labels(in: $0) }
    }

    private static func hashes(_ root: URL) throws -> [String: String] {
        let entries = try require(FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isDirectoryKey]), "vault listing")
        var result: [String: String] = [:]
        for case let url as URL in entries {
            let path = String(url.path.dropFirst(root.path.count + 1))
            result[path] = try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true ? "directory"
                : SHA256.hash(data: try Data(contentsOf: url)).map { String(format: "%02x", $0) }.joined()
        }
        return result
    }

    private static func editor(_ workspace: Workspace, path: String, reading: Bool) async throws -> DiagramTextView {
        try await wait("mounted \(reading ? "reading" : "editing") view: " + path) {
            workspace.selected == path && workspace.textView?.isReading == reading
                && workspace.textView?.diagramPending == false
                && workspace.textView?.string.isEmpty == false
        }
        return try require(workspace.textView, "note view")
    }

    private static func clickText(_ text: String, in view: DiagramTextView, command: Bool = false) async throws {
        let range = (view.string as NSString).range(of: text)
        guard range.location != NSNotFound, range.length > 2, let window = view.window else {
            throw failure("text not displayed: " + text)
        }
        let glyph = NSRange(location: range.location + 2, length: 1)
        view.scrollRangeToVisible(glyph)
        try await Task.sleep(for: .milliseconds(80))
        let rect = view.firstRect(forCharacterRange: glyph, actualRange: nil)
        guard rect.height > 0 else { throw failure("text has no visible rect: " + text) }
        let primaryHeight = CGDisplayBounds(CGMainDisplayID()).height
        let point = CGPoint(x: rect.midX, y: primaryHeight - rect.midY)
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        for type in [CGEventType.leftMouseDown, .leftMouseUp] {
            let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)
            if command { event?.flags = .maskCommand }
            event?.post(tap: .cghidEventTap)
            try await Task.sleep(for: .milliseconds(60))
        }
    }

    private static func key(_ code: UInt16, in window: NSWindow, modifiers: NSEvent.ModifierFlags = []) throws {
        let event = try require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers,
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil,
            characters: code == 6 ? "z" : "", charactersIgnoringModifiers: code == 6 ? "z" : "",
            isARepeat: false, keyCode: code), "key event")
        window.sendEvent(event)
    }

    private static func clickProbe(_ name: String, in window: NSWindow) async throws {
        try await wait("visible control " + name) { ToolDragInspection.find(name, in: window) != nil }
        guard try await ToolDragInspection.click(name, window: window) else { throw failure("click " + name) }
    }

    static func run(workspace: Workspace, layout: Layout, arguments: [String]) async throws -> [String: Any] {
        guard CGPreflightPostEventAccess() else { throw failure("HID event posting permission required") }
        let context = try require(workspace.context, "vault context")
        let app = try require(workspace.application, "application workspace")
        let window = try require(app.records.first { $0.workspace === workspace }?.window, "app window")
        let mediaPrefix = "--knowledge-media="
        let mediaPath = try require(arguments.first { $0.hasPrefix(mediaPrefix) }, "media argument")
        let media = URL(fileURLWithPath: String(mediaPath.dropFirst(mediaPrefix.count)))
        let root = URL(fileURLWithPath: context.root)
        let handle = context.handle
        let modePrefix = "--knowledge-shot="
        let mode = arguments.first { $0.hasPrefix(modePrefix) }.map { String($0.dropFirst(modePrefix.count)) } ?? "en"
        func shot(_ name: String) async throws {
            try await Task.sleep(for: .milliseconds(180))
            try await AppDelegate.snapshot(to: media.appendingPathComponent(name + ".png").path, window: window)
        }
        try await wait("shared index is ready") { context.searchIndexReadyMilliseconds != nil }
        let before = try hashes(root)
        var result: [String: Any] = [
            "left": layout.groups(in: .left).flatMap(\.tabs).map(\.token),
            "right": layout.groups(in: .right).flatMap(\.tabs).map(\.token),
            "bottom_visible": layout.isShown(.bottom),
            "backlinks_label": L10n.string("Backlinks"),
            "tags_label": L10n.string("Tags"),
        ]
        workspace.openNote("architecture.md", savePrevious: false)
        _ = try await editor(workspace, path: "architecture.md", reading: true)
        try await clickProbe("panel-tab-backlinks", in: window)
        try await wait("normal and Mermaid backlinks displayed") {
            ToolDragInspection.find("backlink-router.md-3", in: window) != nil
                && ToolDragInspection.find("backlink-pipeline.md-5", in: window) != nil
        }
        let incoming = await Task.detached { handle.backlinks(path: "architecture.md") }.value
        result["backlink_count"] = incoming.count
        result["diagram_count"] = incoming.filter(\.inDiagram).count
        result["embed_count"] = incoming.filter(\.embed).count
        if mode == "en" { try await shot("backlinks-en-light") }
        if mode == "zh-light" { try await shot("backlinks-and-wikilink-light-zh") }
        try await clickProbe("backlink-router.md-3", in: window)
        _ = try await editor(workspace, path: "router.md", reading: true)
        let source = try require(incoming.first { $0.source == "router.md" }, "router backlink")
        guard workspace.current?.document?.sourceAnchor == Int(source.startUtf16),
              workspace.current?.document?.sourceSelectionLength == Int(source.lengthUtf16) else { throw failure("backlink source range") }
        result["backlink_reveal"] = true

        workspace.openNote("architecture.md", savePrevious: false)
        let reading = try await editor(workspace, path: "architecture.md", reading: true)
        try await clickText("Recovery protocol", in: reading)
        let targetView = try await editor(workspace, path: "target.md", reading: true)
        let heading = try require(workspace.current?.document?.headings.first { $0.title == "Section Two" }, "target heading")
        try await wait("heading reveal") {
            workspace.current?.document?.sourceAnchor == Int(heading.start) && targetView.visibleRect.minY > 20
        }
        result["heading_start"] = heading.start

        workspace.openNote("architecture.md", savePrevious: false)
        let origin = try await editor(workspace, path: "architecture.md", reading: true)
        try await clickText("Unindexed topic", in: origin)
        try await wait("broken link notice") { workspace.knowledgeMessage?.contains("missing") == true }
        result["broken_stayed"] = workspace.selected == "architecture.md"
        try await clickText("system/storage", in: origin)
        try await wait("inline tag selects Tags") { workspace.selectedTag == "system/storage" }
        try await clickProbe("tag-system", in: window)
        try await wait("parent tag includes inherited matches") {
            workspace.selectedTag == "system"
                && ToolDragInspection.find("tag-note-pipeline.md", in: window) != nil
                && ToolDragInspection.find("tag-note-router.md", in: window) != nil
        }
        let tags = await Task.detached { TagPanelRow.rows(handle.tags()) }.value
        result["tag_counts"] = Dictionary(uniqueKeysWithValues: tags.map { ($0.id, $0.count) })
        if mode == "zh-dark" { try await shot("tags-panel-dark-zh") }
        try await clickProbe("tag-note-pipeline.md", in: window)
        _ = try await editor(workspace, path: "pipeline.md", reading: true)
        result["tag_note_opened"] = true

        workspace.openNote("architecture.md", savePrevious: false)
        _ = try await editor(workspace, path: "architecture.md", reading: true)
        await workspace.toggleCurrentView()
        let editing = try await editor(workspace, path: "architecture.md", reading: false)
        try await wait("editor link styling") {
            let range = (editing.string as NSString).range(of: "Recovery protocol")
            return range.location != NSNotFound && ((editing.sourceStyling?.mask(at: range.location) ?? 0) & (1 << 18) != 0)
        }
        try await clickText("Recovery protocol", in: editing, command: true)
        _ = try await editor(workspace, path: "target.md", reading: true)
        try await wait("editor clicked to heading") { workspace.current?.document?.sourceAnchor == Int(heading.start) }
        result["command_click"] = true

        workspace.openNote("architecture.md", savePrevious: false)
        let completionView = try await editor(workspace, path: "architecture.md", reading: false)
        let original = completionView.string
        window.makeFirstResponder(completionView)
        completionView.setSelectedRange(NSRange(location: completionView.sourceUTF16Length, length: 0))
        completionView.insertText("[[arc", replacementRange: completionView.selectedRange())
        try await wait("completion window") { completionView.linkCompletion.onScreen }
        let hits = completionView.linkCompletion.results.map(\.insertTarget)
        guard hits == ["architecture", "archive"], window.firstResponder === completionView else {
            throw failure("completion filtering or focus: \(hits)")
        }
        if mode == "en" { try await shot("link-completion-light") }
        try key(125, in: window)
        guard completionView.linkCompletion.selected == 1 else { throw failure("completion down arrow") }
        try key(126, in: window)
        guard completionView.linkCompletion.selected == 0 else { throw failure("completion up arrow") }
        try key(36, in: window)
        try await wait("completion replacement") { completionView.string == original + "[[architecture]]" }
        completionView.undoManager?.undo()
        try await wait("single completion undo") { completionView.string == original + "[[arc" }
        completionView.undoManager?.undo()
        try await wait("typed fragment undo") { completionView.string == original }
        // 撤销后关闭试验标签，避免应用既有的自动保存改写原字节相同的文件。
        workspace.discardTabForTrash("architecture.md")
        workspace.openNote("architecture.md", savePrevious: false)
        try await wait("reopened clean note") { workspace.current?.document != nil }
        if workspace.current?.document?.reading == false { await workspace.toggleCurrentView() }
        _ = try await editor(workspace, path: "architecture.md", reading: true)
        result["completion"] = ["matches": hits, "keyboard_selection": true, "single_undo": true]
        result["after_read_only"] = try hashes(root)
        result["before_read_only"] = before
        guard try hashes(root) == before else { throw failure("vault changed during read-only operations") }

        if mode == "en" {
            result["rename"] = try await inspectRename(workspace: workspace, app: app, window: window, context: context)
        }
        return result
    }

    private static func inspectRename(workspace: Workspace, app: ApplicationWorkspace,
                                      window: NSWindow, context: VaultContext) async throws -> [String: Any] {
        let second = WindowRecord(layout: app.makeLayout())
        app.configure(second)
        app.records.append(second)
        await second.workspace.attach(context, state: nil)
        app.showWindow?(second.id)
        second.workspace.openNote("dirty.md", savePrevious: false)
        try await wait("second window note") { second.window != nil && second.workspace.current?.document != nil }
        let dirty = try require(second.workspace.current?.document, "dirty note")
        dirty.edited(range: NSRange(location: 0, length: 0), replacement: "Pending desk edit\n")
        dirty.compositionChanged(true)
        defer { second.workspace.discardTabForTrash("dirty.md") }
        app.activate(try require(app.records.first { $0.workspace === workspace }, "first window"))
        let old = "architecture.md", new = "topology.md"
        let rename = Task { try await workspace.renameNoteWithPreview(path: old, newName: new, window: window) }
        let title = L10n.string("Rename and Update Links")
        try await wait("real rename preview sheet") {
            window.attachedSheet?.contentView.flatMap { button(title, in: $0) } != nil
        }
        let content = try require(window.attachedSheet?.contentView, "preview content")
        let preview = labels(in: content).joined(separator: "\n")
        guard preview.contains("router.md:3"), preview.contains("[[architecture"), preview.contains("[[topology") else {
            throw failure("rename preview missing reference excerpts: " + preview)
        }
        try require(button(title, in: content), "confirm button").performClick(nil)
        guard try await rename.value?.path == new else { throw failure("rename did not complete") }
        try await wait("updated backlinks in real panel") {
            workspace.selected == new && ToolDragInspection.find("backlink-pipeline.md-5", in: window) != nil
        }
        let handle = context.handle
        let backlinks = await Task.detached { handle.backlinks(path: new) }.value
        guard dirty.dirty, workspace.knowledgeMessage?.contains("dirty.md") == true else { throw failure("dirty note protection") }
        let text = try await dirty.io.perform { try $0.snapshot().text }
        guard text.hasPrefix("Pending desk edit\n") else { throw failure("dirty editor contents") }
        return ["preview": preview, "dirty_preserved": true, "backlink_count": backlinks.count,
                "targets": backlinks.map(\.target), "new_path": new,
                "updated_open_note": workspace.selected == new]
    }
}
#endif
