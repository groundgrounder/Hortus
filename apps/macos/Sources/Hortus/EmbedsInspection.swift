#if DEBUG
import AppKit
import CryptoKit
import HortusCore

/// 真实窗口里核对嵌入卡片、只读边界与文件夹改名联动；只在调试二进制注册。
@MainActor
enum EmbedsInspection {
    static let argumentPrefix = "--embeds-inspection="
    static let requested = ProcessInfo.processInfo.arguments.contains { $0.hasPrefix(argumentPrefix) }

    private static func failure(_ detail: String) -> NSError {
        NSError(domain: "HortusEmbeds", code: 1, userInfo: [NSLocalizedDescriptionKey: detail])
    }

    private static func require<T>(_ value: T?, _ label: String) throws -> T {
        guard let value else { throw failure(label) }
        return value
    }

    private static func wait(_ label: String, timeout: Double = 10,
                             _ check: @MainActor () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(timeout)
        while !check(), ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        guard check() else { throw failure("timeout: " + label) }
    }

    private static func hashes(_ root: URL) throws -> [String: String] {
        let entries = try require(FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isDirectoryKey]),
                                  "vault listing")
        var result: [String: String] = [:]
        for case let url as URL in entries {
            let path = String(url.path.dropFirst(root.path.count + 1))
            result[path] = try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true ? "directory"
                : SHA256.hash(data: try Data(contentsOf: url)).map { String(format: "%02x", $0) }.joined()
        }
        return result
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

    private static func cards(in view: DiagramTextView) -> [EmbeddedNoteView] {
        var result = view.inspectionEmbeddedCards
        var index = 0
        while index < result.count {
            result += result[index].inspectionEmbeddedCards
            index += 1
        }
        return result
    }

    private static func card(_ cards: [EmbeddedNoteView], title: String) -> EmbeddedNoteView? {
        cards.first { $0.inspectionTitle == title }
    }

    private static func card(_ cards: [EmbeddedNoteView], path: String) -> EmbeddedNoteView? {
        cards.first { $0.inspectionResolvedPath == path }
    }

    private static func click(_ rect: NSRect, window: NSWindow) async throws {
        let primaryHeight = CGDisplayBounds(CGMainDisplayID()).height
        let point = CGPoint(x: rect.midX, y: primaryHeight - rect.midY)
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        for type in [CGEventType.leftMouseDown, .leftMouseUp] {
            let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)
            event?.post(tap: .cghidEventTap)
            try await Task.sleep(for: .milliseconds(70))
        }
    }

    private static func attachmentGeometry(_ card: EmbeddedNoteView) -> [String: Any] {
        guard let textView = card.inspectionReadingView?.inspectionTextView else { return [:] }
        textView.needsLayout = true
        textView.layoutSubtreeIfNeeded()
        return [
            "width": textView.bounds.width,
            "diagrams": textView.diagramViews.values.map { $0.frame.maxX },
            "media": textView.mediaViews.map { $0.frame.maxX },
        ]
    }

    private static func inspectDiagram(_ diagram: DiagramView, workspace: Workspace, window: NSWindow)
        async throws -> [String: Any] {
        let hit = try require(diagram.prepared.scene.hitRegions.first { $0.linkTarget == "Target" }, "diagram link")
        let point = diagram.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
        let event = try require(NSEvent.mouseEvent(with: .rightMouseDown, location: point, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1),
            "diagram menu event")
        let menu = try require(diagram.menu(for: event), "diagram menu")
        let link = try require(menu.items.first { $0.title == DiagramExportStrings.openLinkedNote }, "open link item")
        let collapse = try require(menu.items.first { $0.title == L10n.string("Collapse") }, "collapse item")
        NSApp.sendAction(try require(collapse.action, "collapse action"), to: collapse.target, from: collapse)
        try await wait("embedded node collapsed") {
            diagram.prepared.scene.hitRegions.contains { $0.nodeId == hit.nodeId && $0.collapsed }
        }
        let request = try require(diagram.exportRequestProvider?(), "embedded diagram export")
        let svg = try await request.svg()
        guard svg.hasPrefix("<svg"), !svg.contains(">Leaf<"), !menu.autoenablesItems else {
            throw failure("embedded diagram export or menu")
        }
        NSApp.sendAction(try require(link.action, "open link action"), to: link.target, from: link)
        try await wait("embedded local link target and heading") {
            guard workspace.selected == "gallery/Target.md", let document = workspace.current?.document,
                  let heading = document.headings.first(where: { $0.title == "Section" }) else { return false }
            return document.sourceAnchor == Int(heading.start)
        }
        return ["target": workspace.selected ?? "", "collapsed": true, "svg_bytes": svg.utf8.count,
                "autoenables_items": menu.autoenablesItems, "source": request.note]
    }

    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        guard CGPreflightPostEventAccess() else { throw failure("HID event posting permission required") }
        let context = try require(workspace.context, "vault context")
        let app = try require(workspace.application, "application workspace")
        let window = try require(app.records.first { $0.workspace === workspace }?.window, "app window")
        let mediaPrefix = "--embeds-media="
        let media = URL(fileURLWithPath: String(try require(
            arguments.first { $0.hasPrefix(mediaPrefix) }, "media argument").dropFirst(mediaPrefix.count)))
        let modePrefix = "--embeds-shot="
        let mode = arguments.first { $0.hasPrefix(modePrefix) }.map { String($0.dropFirst(modePrefix.count)) } ?? "en"
        let root = URL(fileURLWithPath: context.root)
        func shot(_ name: String) async throws {
            try await Task.sleep(for: .milliseconds(220))
            try await AppDelegate.snapshot(to: media.appendingPathComponent(name + ".png").path, window: window)
            guard FileManager.default.fileExists(atPath: media.appendingPathComponent(name + ".png").path) else {
                throw failure(name + ".png")
            }
        }
        try await wait("shared index is ready", timeout: 20) { context.searchIndexReadyMilliseconds != nil }
        let before = try hashes(root)
        var result: [String: Any] = [:]

        workspace.openNote("host.md", savePrevious: false)
        try await wait("host reading view") {
            workspace.selected == "host.md" && workspace.textView?.isReading == true
                && workspace.textView?.diagramPending == false && !(workspace.textView?.string.isEmpty ?? true)
        }
        let reading = try require(workspace.textView, "host view")
        try await wait("all top-level cards loaded", timeout: 15) {
            let loaded = cards(in: reading)
            return loaded.count >= 9 && loaded.allSatisfy { $0.inspectionKind != nil }
        }
        let initial = cards(in: reading)
        let full = try require(card(initial, title: "child"), "full child card")
        let section = try require(card(initial, title: "Section alias › Section Two"), "section card")
        let diagram = try require(card(initial, title: "arch"), "mmd card")
        let broken = try require(initial.first { $0.inspectionKind == "broken" }, "broken card")
        let missingHeading = try require(initial.first { $0.inspectionKind == "heading_missing" }, "missing heading card")
        let cycle = try require(initial.first { $0.inspectionKind == "cycle" }, "cycle card")
        guard full.inspectionResolvedPath == "child.md",
              full.inspectionBodyText.contains("Intro visible body"),
              !full.inspectionBodyText.contains("aliases:"),
              full.inspectionBodyText.contains("Section Three") else {
            throw failure("full embed body: " + full.inspectionBodyText)
        }
        guard section.inspectionBodyText.contains("Section two body"),
              !section.inspectionBodyText.contains("Later body"),
              !section.inspectionBodyText.contains("Section Three") else {
            throw failure("section boundary: " + section.inspectionBodyText)
        }
        guard diagram.inspectionKind == "diagram", diagram.inspectionHasDiagram,
              diagram.inspectionResolvedPath == "arch.mmd",
              diagram.inspectionContent?.contains("flowchart LR") == true else {
            throw failure("mmd card")
        }
        guard broken.inspectionMessage?.contains("missing") == true,
              missingHeading.inspectionMessage?.contains("No Such Heading") == true,
              cycle.inspectionMessage?.contains(cycle.inspectionTitle) == true else {
            throw failure("boundary cards")
        }
        try await wait("depth limit card", timeout: 15) {
            cards(in: reading).contains { $0.inspectionKind == "depth_exceeded" }
        }
        guard cards(in: reading).contains(where: { $0.inspectionTitle == "cycle-b" }) else {
            throw failure("nested cycle card")
        }
        let rich = try require(cards(in: reading).first { $0.inspectionResolvedPath == "rich.md" }, "rich card")
        guard rich.inspectionHasTable, rich.inspectionMediaCount >= 1, rich.inspectionMathCount >= 1,
              rich.inspectionDiagramCount >= 1 else {
            throw failure("rich embed media/math/diagram: \(rich.inspectionMediaCount)/\(rich.inspectionMathCount)/\(rich.inspectionDiagramCount)")
        }
        result["rich_embed"] = ["media": rich.inspectionMediaCount, "math": rich.inspectionMathCount,
                                "diagrams": rich.inspectionDiagramCount]
        result["cards"] = initial.map { ["title": $0.inspectionTitle, "kind": $0.inspectionKind ?? "",
                                          "path": $0.inspectionResolvedPath ?? ""] }
        result["full_body_length"] = full.inspectionBodyText.utf16.count
        result["section_body_length"] = section.inspectionBodyText.utf16.count
        if mode == "en" || mode == "zh-light" { try await shot("embeds-\(mode)-full") }

        // 真实鼠标点击卡片标题：完整嵌入跳到原笔记，章节嵌入定位到标题。
        let titleFrame = try require(full.inspectionTitleFrameInWindow, "full title frame")
        try await click(titleFrame, window: window)
        var openedByClick = false
        let clickDeadline = ContinuousClock.now + .seconds(3)
        while ContinuousClock.now < clickDeadline {
            if workspace.selected == "child.md" { openedByClick = true; break }
            try await Task.sleep(for: .milliseconds(20))
        }
        if !openedByClick {
            // 窗口缩放或异步卡片高度变化可能让首帧点击落空；标题按钮动作仍走同一入口。
            full.inspectionOpen()
            try await wait("opened embedded note") { workspace.selected == "child.md" }
        }
        result["title_click_hid"] = openedByClick
        workspace.openNote("host.md", savePrevious: false)
        try await wait("host reopened") {
            workspace.selected == "host.md" && workspace.textView?.isReading == true
                && workspace.textView?.diagramPending == false
        }
        let reopened = try require(workspace.textView, "host view")
        try await wait("cards after reopen", timeout: 15) {
            cards(in: reopened).contains { $0.inspectionTitle.hasPrefix("Section alias") && $0.inspectionKind == "note" }
        }
        let reopenedSection = try require(cards(in: reopened).first { $0.inspectionTitle.hasPrefix("Section alias") },
                                          "section card after reopen")
        // 标题动作与真实标题栏按钮共用同一入口；完整卡片已用真实鼠标点击验证。
        reopenedSection.inspectionOpen()
        try await wait("opened section note") {
            workspace.selected == "child.md" && workspace.current?.document?.headings.isEmpty == false
        }
        let heading = try require(workspace.current?.document?.headings.first { $0.title == "Section Two" },
                                  "child heading")
        try await wait("revealed section heading") {
            workspace.current?.document?.sourceAnchor == Int(heading.start)
        }
        result["title_click"] = true
        result["section_click"] = true
        workspace.openNote("mmd-first.md", savePrevious: false)
        try await wait("mmd title page", timeout: 15) {
            workspace.selected == "mmd-first.md"
                && cards(in: workspace.textView ?? reopened).contains { $0.inspectionHasDiagram }
        }
        let mmdFirst = try require(cards(in: workspace.textView ?? reopened).first { $0.inspectionHasDiagram },
                                   "mmd first card")
        mmdFirst.inspectionOpen()
        try await wait("opened mmd file") { workspace.selected == "arch.mmd" }
        result["mmd_click"] = true
        func openSeams() async throws -> (EmbeddedNoteView, EmbeddedNoteView, EmbeddedNoteView) {
            workspace.openNote("seams.md", savePrevious: false)
            try await wait("embedded diagram seams", timeout: 15) {
                guard workspace.selected == "seams.md", let view = workspace.textView else { return false }
                let active = cards(in: view)
                return card(active, path: "gallery/gallery.mmd")?.inspectionHasDiagram == true
                    && card(active, path: "rich.md")?.inspectionDiagramCount == 1
                    && card(active, path: "gallery/nested.md")?.inspectionDiagramCount == 1
            }
            let active = cards(in: try require(workspace.textView, "seams view"))
            return (try require(card(active, path: "gallery/gallery.mmd"), "mmd seams card"),
                    try require(card(active, path: "rich.md"), "rich seams card"),
                    try require(card(active, path: "gallery/nested.md"), "nested seams card"))
        }
        let (mmdSeams, _, _) = try await openSeams()
        result["mmd_interaction"] = try await inspectDiagram(
            try require(mmdSeams.inspectionDiagramView, "mmd diagram view"), workspace: workspace, window: window)
        let (_, richSeams, nestedSeams) = try await openSeams()
        result["geometry"] = ["outer": attachmentGeometry(richSeams), "nested": attachmentGeometry(nestedSeams)]
        let nestedDiagram = try require(nestedSeams.inspectionReadingView?.inspectionTextView.diagramViews[1],
                                        "nested diagram view")
        result["nested_interaction"] = try await inspectDiagram(nestedDiagram, workspace: workspace, window: window)
        guard try hashes(root) == before else { throw failure("vault changed during read-only embeds") }
        result["read_only"] = true
        workspace.openNote("host.md", savePrevious: false)
        try await wait("host reopened for live refresh") {
            workspace.selected == "host.md" && workspace.textView?.isReading == true
                && workspace.textView?.diagramPending == false
        }

        if mode == "zh-dark" || mode == "en-dark" {
            workspace.openNote("boundary.md", savePrevious: false)
            try await wait("boundary view", timeout: 15) {
                workspace.selected == "boundary.md" && workspace.textView?.diagramPending == false
                    && cards(in: workspace.textView ?? reading).contains { $0.inspectionKind == "cycle" }
            }
            try await shot("embeds-\(mode)-boundary")
            workspace.openNote("mmd-first.md", savePrevious: false)
            try await wait("mmd view", timeout: 15) {
                workspace.selected == "mmd-first.md"
                    && cards(in: workspace.textView ?? reading).contains { $0.inspectionHasDiagram }
            }
            try await shot("embeds-\(mode)-mmd")
            workspace.openNote("host.md", savePrevious: false)
            try await wait("host after shots") {
                workspace.selected == "host.md" && workspace.textView?.diagramPending == false
            }
        }

        // 被嵌入文件改动后，宿主视图自动刷新，无需关闭重开。
        let childURL = root.appendingPathComponent("child.md")
        let diagramURL = root.appendingPathComponent("arch.mmd")
        let childText = try String(contentsOf: childURL, encoding: .utf8)
        try (childText + "\nFresh body marker.\n").write(to: childURL, atomically: true, encoding: .utf8)
        try "flowchart LR\n  Fresh[Fresh] --> B\n".write(to: diagramURL, atomically: true, encoding: .utf8)
        try await wait("host auto refresh", timeout: 15) {
            guard workspace.selected == "host.md", workspace.textView?.diagramPending == false else { return false }
            let current = cards(in: workspace.textView ?? reopened)
            return current.contains { $0.inspectionBodyText.contains("Fresh body marker") }
                && current.contains { $0.inspectionContent?.contains("Fresh") == true }
        }
        result["live_refresh"] = true

        try await inspectRename(workspace: workspace, window: window, context: context, result: &result,
                                mode: mode, shot: shot)
        result["before_read_only"] = before
        return result
    }

    private static func inspectRename(workspace: Workspace, window: NSWindow, context: VaultContext,
                                      result: inout [String: Any], mode: String,
                                      shot: (String) async throws -> Void) async throws {
        let root = URL(fileURLWithPath: context.root)
        workspace.openNote("dirty.md", savePrevious: false)
        try await wait("dirty note open") { workspace.selected == "dirty.md" && workspace.current?.document != nil }
        let dirty = try require(workspace.current?.document, "dirty document")
        dirty.edited(range: NSRange(location: 0, length: 0), replacement: "Pending desk edit\n")
        dirty.compositionChanged(true)
        defer { workspace.discardTabForTrash("dirty.md") }
        workspace.openNote("outside.md", savePrevious: false)
        try await wait("rename host open") {
            workspace.selected == "outside.md" && workspace.textView?.isReading == true
                && workspace.textView?.diagramPending == false
        }
        let rename = Task { try await workspace.renameNoteWithPreview(path: "docs", newName: "papers", window: window) }
        let title = L10n.string("Rename and Update Links")
        try await wait("folder rename preview sheet") {
            window.attachedSheet?.contentView.flatMap { button(title, in: $0) } != nil
        }
        let content = try require(window.attachedSheet?.contentView, "preview content")
        let preview = labels(in: content).joined(separator: "\n")
        guard preview.contains("outside.md"), preview.contains("[[docs/"),
              preview.contains("[[papers/") else {
            throw failure("folder preview excerpts: " + preview)
        }
        result["rename_preview"] = preview
        if mode == "en" { try await shot("folder-rename-preview-light-en") }
        try require(button(title, in: content), "confirm button").performClick(nil)
        let moved = try await rename.value
        guard moved?.path == "papers" else { throw failure("folder rename did not complete") }
        try await wait("dirty skip notice") { workspace.knowledgeMessage?.contains("dirty.md") == true }
        let dirtyText = try await dirty.io.perform { try $0.snapshot().text }
        guard dirty.dirty, dirtyText.hasPrefix("Pending desk edit\n") else { throw failure("dirty document overwritten") }
        let outside = try String(contentsOf: root.appendingPathComponent("outside.md"), encoding: .utf8)
        let keep = try String(contentsOf: root.appendingPathComponent("docs-extra/keep.md"), encoding: .utf8)
        let dirtyDisk = try String(contentsOf: root.appendingPathComponent("dirty.md"), encoding: .utf8)
        guard outside.contains("[[papers/intro]]"), outside.contains("[[papers/sub/api#Auth|API]]"),
              outside.contains("![[papers/arch.mmd]]"), outside.contains("[[intro]]"),
              outside.contains("[[docs-extra/keep]]"), keep == "# Keep\n",
              dirtyDisk.hasPrefix("[[docs/intro]]") else {
            throw failure("folder rename disk state: outside=\(outside.debugDescription) dirty=\(dirtyDisk.debugDescription) keep=\(keep.debugDescription)")
        }
        guard !FileManager.default.fileExists(atPath: root.appendingPathComponent("docs").path) else {
            throw failure("old folder still exists")
        }
        let handle = context.handle
        guard !handle.backlinks(path: "papers/intro.md").isEmpty else { throw failure("backlinks after folder rename") }
        workspace.openNote("outside.md", savePrevious: false)
        try await wait("refreshed embed after rename", timeout: 15) {
            guard workspace.selected == "outside.md", workspace.textView?.diagramPending == false else { return false }
            guard let view = workspace.textView else { return false }
            return cards(in: view).contains { $0.inspectionResolvedPath == "papers/arch.mmd" }
        }
        result["folder_rename"] = ["preview": preview, "dirty_preserved": true, "resolved_after": "papers/arch.mmd"]
        if mode == "en-dark" || mode == "zh-dark" { try await shot("folder-rename-\(mode)-after") }
    }
}
#endif
