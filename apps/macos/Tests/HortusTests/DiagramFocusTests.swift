import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite @MainActor
struct DiagramFocusTests {
    @Test func canFocusDiagramFollowsPreparedDiagramState() throws {
        let workspace = Workspace()
        #expect(!workspace.canFocusDiagram)
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let vault = directory.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
        let url = vault.appendingPathComponent("note.md")
        try Data("plain\n".utf8).write(to: url)
        let core = try WorkspaceCore.inDirectories(config: directory.appendingPathComponent("config").path,
                                                   cache: directory.appendingPathComponent("cache").path)
        let session = try core.openDocument(vault: vault.path, path: "note.md")
        let document = NoteDocument(io: DocumentIO(session: session, url: url), initial: try session.initial())
        workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        workspace.selected = "note.md"
        #expect(!workspace.canFocusDiagram)
        document.hasPreparedDiagrams = true
        #expect(workspace.canFocusDiagram)
        document.hasPreparedDiagrams = false
        #expect(!workspace.canFocusDiagram)
    }

    private func keyEvent(_ characters: String, _ modifiers: NSEvent.ModifierFlags) -> NSEvent? {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers, timestamp: 0,
                         windowNumber: 0, context: nil, characters: characters,
                         charactersIgnoringModifiers: characters, isARepeat: false, keyCode: 0)
    }

    @Test func focusShortcutsRequireExactModifiers() throws {
        let plain = try #require(keyEvent("f", .command))
        #expect(FocusKeyCommand.resolve(plain) == .search)
        if let shifted = keyEvent("f", [.command, .shift]) { #expect(FocusKeyCommand.resolve(shifted) == nil) }
        if let option = keyEvent("f", [.command, .option]) { #expect(FocusKeyCommand.resolve(option) == nil) }
        if let noCommand = keyEvent("f", []) { #expect(FocusKeyCommand.resolve(noCommand) == nil) }
        if let exit = keyEvent("d", [.command, .shift]) { #expect(FocusKeyCommand.resolve(exit) == .exit) }
        if let plainD = keyEvent("d", .command) { #expect(FocusKeyCommand.resolve(plainD) == nil) }
        if let extra = keyEvent("d", [.command, .shift, .option]) { #expect(FocusKeyCommand.resolve(extra) == nil) }
        if let next = keyEvent("g", .command) { #expect(FocusKeyCommand.resolve(next) == .nextMatch) }
        if let previous = keyEvent("g", [.command, .shift]) {
            #expect(FocusKeyCommand.resolve(previous) == .previousMatch)
        }
        if let optionG = keyEvent("g", [.command, .option]) { #expect(FocusKeyCommand.resolve(optionG) == nil) }
        if let fit = keyEvent("0", .command) { #expect(FocusKeyCommand.resolve(fit) == .fit) }
        if let actual = keyEvent("1", .command) { #expect(FocusKeyCommand.resolve(actual) == .actualSize) }
        if let zoomIn = keyEvent("=", .command) { #expect(FocusKeyCommand.resolve(zoomIn) == .zoomIn) }
        if let zoomOut = keyEvent("-", .command) { #expect(FocusKeyCommand.resolve(zoomOut) == .zoomOut) }
    }

    @Test func zoomIsStrictlyClampedToQuarterAndFour() {
        #expect(DiagramFocusLimits.clamp(0.01) == 0.25)
        #expect(DiagramFocusLimits.clamp(0.25) == 0.25)
        #expect(DiagramFocusLimits.clamp(1.75) == 1.75)
        #expect(DiagramFocusLimits.clamp(4) == 4)
        #expect(DiagramFocusLimits.clamp(12) == 4)
    }

    private func preparedScene() throws -> PreparedScene {
        let source = "mindmap\n  root((Root))\n    first(First)\n      second(Second)\n        third(Third)\n          fourth(Fourth)\n    other(Other)\n"
        let scene = try DiagramEngine().render(source: source, mmd: true, block: 1, width: 420,
                                               dark: false, collapsed: [])
        let offsets = DiagramSourceOffsets(source)
        var ranges: [String: NSRange] = [:]
        for hit in scene.hitRegions {
            if let range = offsets.range(start: hit.sourceStart, end: hit.sourceEnd) { ranges[hit.nodeId] = range }
        }
        return PreparedScene(scene: try DiagramScene(scene), sourceRanges: ranges)
    }

    @Test func highlightOnlyInvalidatesDrawingAndKeepsTheScene() throws {
        let prepared = try preparedScene()
        let view = DiagramView(prepared: prepared)
        guard let hit = prepared.scene.hitRegions.first(where: \.canCollapse) else {
            Issue.record("没有可折叠节点")
            return
        }
        view.needsDisplay = false
        view.highlightedNodeID = hit.nodeId
        #expect(view.prepared === prepared)
        #expect(view.highlightedNodeID == hit.nodeId)
        view.highlightedNodeID = nil
        #expect(view.highlightedNodeID == nil)
    }

    @Test func accessibilityCustomActionSelectsNodeSource() throws {
        let prepared = try preparedScene()
        let view = DiagramView(prepared: prepared)
        guard let hit = prepared.scene.hitRegions.first(where: \.canCollapse),
              let expected = prepared.sourceRanges[hit.nodeId] else {
            Issue.record("没有可折叠节点")
            return
        }
        var jumped: NSRange?
        view.onSourceRange = { jumped = $0 }
        let children = view.accessibilityChildren() as? [NSAccessibilityElement] ?? []
        guard let action = children.prefix(view.accessibilityNodeCount)
            .compactMap({ $0.accessibilityCustomActions()?.first }).first else {
            Issue.record("节点没有跳到源码动作")
            return
        }
        #expect(action.name == L10n.string("Jump to Source"))
        #expect(action.handler?() == true)
        #expect(jumped == expected)
    }

    @Test func commandClickOnCollapsibleNodeJumpsInsteadOfToggling() throws {
        let prepared = try preparedScene()
        let view = DiagramView(prepared: prepared)
        view.frame = NSRect(x: 0, y: 0, width: prepared.scene.width, height: prepared.scene.height)
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = view
        defer { window.close() }
        guard let hit = prepared.scene.hitRegions.first(where: \.canCollapse),
              let expected = prepared.sourceRanges[hit.nodeId] else {
            Issue.record("没有可折叠节点")
            return
        }
        var toggled = false, jumped: NSRange?
        view.onToggle = { _ in toggled = true }
        view.onSourceRange = { jumped = $0 }
        let point = view.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
        guard let event = NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [.command],
                                             timestamp: 0, windowNumber: window.windowNumber, context: nil,
                                             eventNumber: 0, clickCount: 1, pressure: 1) else {
            Issue.record("无法构造鼠标事件")
            return
        }
        view.mouseDown(with: event)
        #expect(!toggled)
        #expect(jumped == expected)
        // 普通单击仍折叠可折叠节点，不回归。
        guard let plain = NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [],
                                             timestamp: 0, windowNumber: window.windowNumber, context: nil,
                                             eventNumber: 0, clickCount: 1, pressure: 1) else { return }
        toggled = false
        view.mouseDown(with: plain)
        #expect(toggled)
    }

    @Test func contextMenuOffersJumpCollapseAndFocus() throws {
        let prepared = try preparedScene()
        let view = DiagramView(prepared: prepared)
        view.frame = NSRect(x: 0, y: 0, width: prepared.scene.width, height: prepared.scene.height)
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = view
        defer { window.close() }
        var focused = false
        view.onFocus = { focused = true }
        guard let hit = prepared.scene.hitRegions.first(where: \.canCollapse) else {
            Issue.record("没有可折叠节点")
            return
        }
        let point = view.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
        guard let event = NSEvent.mouseEvent(with: .rightMouseDown, location: point, modifierFlags: [],
                                             timestamp: 0, windowNumber: window.windowNumber, context: nil,
                                             eventNumber: 0, clickCount: 1, pressure: 1),
              let menu = view.menu(for: event) else {
            Issue.record("没有右键菜单")
            return
        }
        let titles = menu.items.filter { !$0.isSeparatorItem }.map(\.title)
        #expect(titles.contains(L10n.string("Jump to Source")))
        #expect(titles.contains(L10n.string("Collapse")))
        #expect(titles.contains(L10n.string("Focus Mode")))
        if let item = menu.items.first(where: { $0.title == L10n.string("Focus Mode") }),
           let action = item.action, let target = item.target as? NSObject {
            _ = target.perform(action, with: item)
        }
        #expect(focused)
    }

    /// 专注覆盖层的真实窗口夹具：正文、会话和画布都走生产路径。
    @MainActor
    private final class FocusFixture {
        final class Flag { var value = false }
        let editing: EditingTests.Fixture
        let document: NoteDocument
        let session: DiagramFocusSession
        let focus: FocusModeView
        let window: NSWindow
        let flag: Flag

        init(_ source: String, ordinal: UInt32 = 1) throws {
            let exitFlag = Flag()
            flag = exitFlag
            editing = try EditingTests.Fixture(source, path: "note.mmd")
            document = editing.document
            session = DiagramFocusSession(
                note: editing.path, vault: editing.vault.path, ordinal: ordinal, document: document,
                reading: false, selection: NSRange(location: 0, length: 0), scroll: .zero, initial: nil)
            focus = FocusModeView(session: session, core: editing.core, fontSize: 15,
                                  onExit: { exitFlag.value = true },
                                  onJump: { _ in }, onCollapse: { _ in }, onPersist: { _, _, _ in })
            window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 700),
                              styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            let container = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 700))
            window.contentView = container
            container.addSubview(focus)
            focus.frame = container.bounds
            focus.autoresizingMask = [.width, .height]
            window.makeKeyAndOrderFront(nil)
            window.makeKey()
            focus.needsLayout = true
            focus.layoutSubtreeIfNeeded()
            window.displayIfNeeded()
        }

        func ready() async throws {
            for _ in 0..<800 {
                window.contentView?.layoutSubtreeIfNeeded()
                focus.layoutSubtreeIfNeeded()
                if focus.canvas.inspectionReady { return }
                try await Task.sleep(for: .milliseconds(10))
            }
            Issue.record("专注画布未就绪: focus=\(NSStringFromRect(focus.frame)) canvas=\(NSStringFromRect(focus.canvas.frame)) scene=\(focus.canvas.inspectionSceneSize)")
        }

        func close() async {
            window.close()
            await editing.close()
        }
    }

    private static let tallSequence: String = {
        let messages = (0..<40).map { "U->>M: 第\($0)步" }.joined(separator: "\n")
        return "sequenceDiagram\nparticipant U as 你\nparticipant M as Hortus\nparticipant C as 孤独者\n"
            + messages + "\n"
    }()

    @Test func focusSearchCountsMirroredParticipantsOnce() async throws {
        let fixture = try FocusFixture(Self.tallSequence)
        try await fixture.ready()
        // 该参与者不出现在任何消息前缀里，搜索只能命中顶部与底部两个镜像胶囊。
        fixture.focus.canvas.inspectionSearch("孤独者")
        #expect(fixture.focus.canvas.inspectionMatches["visible"] as? Int == 1)
        await fixture.close()
    }

    @Test func focusDocumentRevisionReloadsAndKeepsViewport() async throws {
        var source = Self.tallSequence
        let fixture = try FocusFixture(source)
        try await fixture.ready()
        let canvas = fixture.focus.canvas
        canvas.inspectionZoom(2)
        canvas.inspectionPan(30, -20)
        let center = try #require(canvas.inspectionViewportCenter)
        let before = canvas.inspectionRustLayoutCalls
        canvas.inspectionSearch("Hortus")
        #expect((canvas.inspectionMatches["visible"] as? Int ?? 0) > 0)

        source += "M-->>U: 外部修改\n"
        try Data(source.utf8).write(to: fixture.document.io.url)
        fixture.document.refresh()
        let deadline = ContinuousClock.now + .seconds(3)
        while fixture.document.text != source, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.document.text == source)
        fixture.focus.updateDocument(revision: fixture.document.revision, refreshToken: 0)
        for _ in 0..<300 where canvas.inspectionRustLayoutCalls <= before {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(canvas.inspectionRustLayoutCalls > before)
        try await Task.sleep(for: .milliseconds(150))
        #expect(abs(canvas.inspectionSceneScale() - 2) < 0.001)
        let after = try #require(canvas.inspectionViewportCenter)
        #expect(hypot(after.x - center.x, after.y - center.y) < 1)
        #expect((canvas.inspectionMatches["visible"] as? Int ?? 0) > 0)
        await fixture.close()
    }

    @Test func focusShortcutsOnlyApplyInsideTheOverlay() async throws {
        let fixture = try FocusFixture(Self.tallSequence)
        try await fixture.ready()
        let previousMenu = NSApp.mainMenu
        NSApp.mainMenu = NSMenu()
        defer { NSApp.mainMenu = previousMenu }

        final class ResponderView: NSView {
            override var acceptsFirstResponder: Bool { true }
        }
        let external = ResponderView(frame: NSRect(x: 0, y: 0, width: 80, height: 22))
        fixture.window.contentView?.addSubview(external)
        fixture.window.makeKey()
        #expect(fixture.window.makeFirstResponder(external))
        let search = try #require(keyEvent("f", .command))
        let outsideSearch = fixture.focus.performKeyEquivalent(with: search)
        #expect(!outsideSearch)
        #expect(fixture.window.firstResponder === external)
        let fit = try #require(keyEvent("0", .command))
        let outsideFit = fixture.focus.performKeyEquivalent(with: fit)
        #expect(!outsideFit)
        let exit = try #require(keyEvent("d", [.command, .shift]))
        let outsideExit = fixture.focus.performKeyEquivalent(with: exit)
        #expect(!outsideExit)
        #expect(!fixture.flag.value)

        fixture.window.makeFirstResponder(fixture.focus.canvas)
        let insideSearch = fixture.focus.performKeyEquivalent(with: search)
        #expect(insideSearch)
        #expect(fixture.focus.inspectionSearchVisible)
        fixture.window.makeFirstResponder(fixture.focus.inspectionSearchField)
        let actual = try #require(keyEvent("1", .command))
        let insideActual = fixture.focus.performKeyEquivalent(with: actual)
        #expect(insideActual)
        let insideExit = fixture.focus.performKeyEquivalent(with: exit)
        #expect(insideExit)
        #expect(fixture.flag.value)
        await fixture.close()
    }

    @Test func backgroundWindowCannotOverwriteTheFocusMenu() throws {
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let vault = temporary.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
        try "plain\n".write(to: vault.appendingPathComponent("note.md"), atomically: true, encoding: .utf8)
        let core = try WorkspaceCore.inDirectories(config: temporary.appendingPathComponent("config").path,
                                                   cache: temporary.appendingPathComponent("cache").path)
        let session = try core.openDocument(vault: vault.path, path: "note.md")
        let document = NoteDocument(io: DocumentIO(session: session, url: vault.appendingPathComponent("note.md")),
                                    initial: try session.initial())
        let application = ApplicationWorkspace()
        application.core = core
        let first = application.initial
        let second = WindowRecord(layout: Layout())
        application.records.append(second)
        application.configure(first)
        application.configure(second)
        first.workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        first.workspace.selected = "note.md"
        document.hasPreparedDiagrams = true

        let item = NSMenuItem(title: "Focus", action: nil, keyEquivalent: "d")
        item.keyEquivalentModifierMask = [.command, .shift]
        item.isEnabled = false
        FocusMenuState.inspectionItem = item
        defer { FocusMenuState.inspectionItem = nil }

        application.activeID = first.id
        first.workspace.syncFocusDiagramMenu()
        #expect(item.isEnabled)
        // 背景窗口排版完成不能把前台窗口的菜单关掉。
        second.workspace.syncFocusDiagramMenu()
        #expect(item.isEnabled)
        application.activeID = second.id
        second.workspace.syncFocusDiagramMenu()
        #expect(!item.isEnabled)
    }
}
