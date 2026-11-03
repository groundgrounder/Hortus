import AppKit
import Testing
@testable import Hortus

@Suite @MainActor
struct WindowStateTests {
    @Test func closingBeforeTransferredEditorMountsReleasesItsCallbacksAndView() async throws {
        _ = NSApplication.shared
        let fixture = try CloseTests.Fixture()
        defer { fixture.cleanup() }
        let host = NoteText.Host()
        let container = NSView()
        host.container = container
        let input = NoteText(text: "", path: "note.md", vault: fixture.vault.path, fontSize: 15,
                             service: DiagramService(), core: fixture.core, document: fixture.document) { _, _ in }
        host.update(input)
        let surface = try #require(host.take())
        weak let view = surface.scroll.documentView
        fixture.document.transferredEditor = surface
        fixture.document.stopForShutdown()
        #expect(fixture.document.transferredEditor == nil)
        #expect(fixture.document.viewOwner == nil && fixture.document.onReload == nil)
        #expect(surface.editor.document == nil && surface.scroll.documentView == nil)
        for _ in 0..<100 where view != nil { try await Task.sleep(for: .milliseconds(10)) }
        #expect(view == nil)
    }

    @Test func loadingCloseKeepsUniqueReservationUntilOpenCompletes() async throws {
        _ = NSApplication.shared
        let fixture = try CloseTests.Fixture()
        defer { fixture.cleanup() }
        let application = ApplicationWorkspace()
        application.core = fixture.core
        let first = application.initial
        let second = WindowRecord(layout: Layout(arrangement: application.arrangement))
        application.records.append(second)
        let context = VaultContext(handle: try fixture.core.openVault(path: fixture.vault.path))
        application.contexts[context.root] = context
        for record in [first, second] {
            application.configure(record)
            record.workspace.context = context; context.attach(record.workspace)
        }
        first.workspace.openNote("note.md")
        #expect(first.workspace.tabs.first?.loading == true)
        first.workspace.closeTab("note.md")
        second.workspace.openSearchResult(path: "note.md", start: 1, length: 1)
        #expect(second.workspace.tabs.isEmpty)
        #expect(first.workspace.tabs.count == 1)
        #expect(first.workspace.closingTabs.contains("note.md"))
        for _ in 0..<500 where !first.workspace.tabs.isEmpty { try await Task.sleep(for: .milliseconds(10)) }
        #expect(first.workspace.tabs.isEmpty)
        second.workspace.openNote("note.md")
        // 关闭库也等待占位完成，随后才能释放最后一个共享上下文使用者。
        await application.detach(second.workspace)
        #expect(second.workspace.tabs.isEmpty)
        #expect(context.windowCount == 1)
        await application.detach(first.workspace)
        #expect(context.listenerCount == 0 && application.contexts.isEmpty)
    }

    @Test func tabMoveConsumesTokenAndKeepsDocumentQueueAndLastVaultUser() async throws {
        _ = NSApplication.shared
        let fixture = try CloseTests.Fixture()
        defer { fixture.cleanup() }
        let application = ApplicationWorkspace()
        application.core = fixture.core
        let first = application.initial
        application.configure(first)
        let context = VaultContext(handle: try fixture.core.openVault(path: fixture.vault.path))
        application.contexts[context.root] = context
        first.workspace.context = context; context.attach(first.workspace)
        first.workspace.tabs = [NoteTab(path: "note.md", document: fixture.document, loading: false)]
        first.workspace.selected = "note.md"
        let document = fixture.document
        let point = NSPoint(x: 100000, y: 100000)
        document.marked = true
        #expect(application.beginTabDrag(workspace: first.workspace, path: "note.md") == nil)
        document.marked = false
        first.workspace.tabs[0].loading = true
        #expect(application.beginTabDrag(workspace: first.workspace, path: "note.md") == nil)
        first.workspace.tabs[0].loading = false
        let cancelled = try #require(application.beginTabDrag(workspace: first.workspace, path: "note.md"))
        application.cancelTabDrag(cancelled)
        #expect(application.finishTabDrag(cancelled, at: point) == nil)
        let closing = try #require(application.beginTabDrag(workspace: first.workspace, path: "note.md"))
        first.closing = true
        #expect(application.finishTabDrag(closing, at: point) == nil)
        first.closing = false
        let composition = try #require(application.beginTabDrag(workspace: first.workspace, path: "note.md"))
        document.marked = true
        #expect(application.finishTabDrag(composition, at: point) == nil)
        document.marked = false
        document.edited(range: NSRange(location: 0, length: 0), replacement: "unsaved")
        await document.checkpoint()
        let recovery = fixture.cache.appendingPathComponent("recovery")
        let drafts = try FileManager.default.contentsOfDirectory(at: recovery, includingPropertiesForKeys: nil)
        let draftBytes = try drafts.map { try Data(contentsOf: $0) }
        #expect(!drafts.isEmpty)
        document.sourceAnchor = 3; document.sourceSelectionLength = 2; document.reading = false
        let revision = document.revision, persistence = document.persistence
        let payload = try #require(application.beginTabDrag(workspace: first.workspace, path: "note.md"))
        #expect(first.workspace.tabs.count == 1)
        #expect(application.beginTabDrag(workspace: first.workspace, path: "note.md") == nil)
        let target = try #require(application.finishTabDrag(payload, at: point))
        #expect(application.finishTabDrag(payload, at: point) == nil)
        #expect(first.workspace.tabs.isEmpty && first.workspace.selected == nil)
        #expect(target.workspace.current?.document === document)
        #expect(target.workspace.current?.document?.io.session === fixture.document.io.session)
        #expect(document.dirty && document.revision == revision && document.persistence == persistence)
        #expect(!document.reading && document.sourceAnchor == 3 && document.sourceSelectionLength == 2)
        #expect(try drafts.map { try Data(contentsOf: $0) } == draftBytes)
        #expect(context.windowCount == 2 && context.listenerCount == 1)
        let closingMove = try #require(application.beginTabDrag(workspace: target.workspace, path: "note.md"))
        application.requestClose(target)
        #expect(application.tabDrags.isEmpty)
        #expect(application.finishTabDrag(closingMove, at: point) == nil)
        for _ in 0..<500 where application.records.count > 1 { try await Task.sleep(for: .milliseconds(10)) }
        #expect(application.records.count == 1)
        #expect(context.windowCount == 1 && context.listenerCount == 1 && application.contexts.count == 1)
        await application.detach(first.workspace)
        #expect(context.windowCount == 0 && context.listenerCount == 0 && application.contexts.isEmpty)
    }

    @Test func sameVaultOpenRoutesToExistingDocumentAndSnapshotKeepsLastVisibleWrite() async throws {
        _ = NSApplication.shared
        let fixture = try CloseTests.Fixture()
        defer { fixture.cleanup() }
        let application = ApplicationWorkspace()
        let first = application.initial
        let second = WindowRecord(layout: Layout(arrangement: application.arrangement))
        application.records.append(second)
        let context = VaultContext(handle: try fixture.core.openVault(path: fixture.vault.path))
        for record in [first, second] {
            record.workspace.configure(application: application, core: fixture.core, layout: record.layout)
            record.workspace.context = context
        }
        first.workspace.tabs = [NoteTab(path: "note.md", document: fixture.document, loading: false)]
        first.workspace.selected = "note.md"
        second.workspace.openSearchResult(path: "note.md", start: 1, length: 1)
        #expect(second.workspace.tabs.isEmpty)
        #expect(application.activeID == first.id)
        #expect(fixture.document.sourceAnchor == 1)
        #expect(fixture.document.sourceSelectionLength == 1)

        // 后写者在 records 的前面：遍历快照不能把旧值写回来。
        second.layout.visible[.left] = true
        application.layoutChanged(second.workspace)
        first.layout.visible[.left] = false
        application.layoutChanged(first.workspace)
        for _ in 0..<3 {
            #expect(application.snapshot().recent.first?.visible?["left"] == false)
        }
        await context.close()
    }

    @Test func explicitLaunchProtectsRealSessionAndIsolatedLaunchCanRoundTrip() {
        for arguments in [["--empty"], ["--vault=/public", "--note=same.md"]] {
            #expect(!ApplicationWorkspace.shouldPersistSession(arguments: arguments, cacheOverride: false))
            #expect(ApplicationWorkspace.shouldPersistSession(arguments: arguments, cacheOverride: true))
        }
        #expect(ApplicationWorkspace.shouldPersistSession(arguments: [], cacheOverride: false))
    }

    @Test func groupsAreSharedButSizesAndVisibilityBelongToEachWindow() throws {
        let arrangement = PanelArrangement()
        let a = Layout(arrangement: arrangement), b = Layout(arrangement: arrangement)
        a.split(.search, to: .right)
        #expect(a.state.regions.mapValues(\.groups) == b.state.regions.mapValues(\.groups))
        let group = try #require(a.groups(in: .right).last)
        b.select(.search, in: group.id)
        #expect(a.groups(in: .right).last?.selected == .search)
        a.visible[.left] = false
        a.size[.left] = 350
        #expect(b.visible[.left] == true)
        #expect(b.size[.left] == 260)
        let c = Layout(arrangement: arrangement)
        c.size = a.size
        #expect(c.size[.left] == 350)
        #expect(c.state.regions.mapValues(\.groups) == a.state.regions.mapValues(\.groups))
        let json = String(decoding: try JSONEncoder().encode(a.persistedState), as: UTF8.self)
        #expect(!json.contains("visible"))
    }

    @Test func sessionRoundTripRetainsOrderIdentityViewsAndVaultVisibility() throws {
        let input = Data(#"{"recent":[{"path":"/a","tabs":["same.md"],"selected":"same.md","views":{"same.md":"editing"},"visible":{"left":false}}],"windows":[{"id":"2","path":"/b","tabs":["same.md"],"selected":"same.md","views":{"same.md":"reading"},"frame":{"x":10,"y":30,"width":900,"height":560,"screen":"1"}},{"id":"1","path":"/a","tabs":["same.md"],"selected":"same.md","views":{"same.md":"editing"}}],"last_vault":"/b"}"#.utf8)
        let session = try JSONDecoder().decode(SessionState.self, from: input)
        let restored = try JSONDecoder().decode(SessionState.self, from: JSONEncoder().encode(session))
        #expect(restored.windows == session.windows)
        #expect(restored.windows?.map(\.id) == ["2", "1"])
        #expect(restored.recent.first?.visible?["left"] == false)
        let frame = try #require(restored.windows?.first?.frame)
        let screen = NSRect(x: 0, y: 0, width: 1920, height: 1080)
        #expect(frame.restored(in: [screen], cascade: 1) == NSRect(x: 10, y: 30, width: 900, height: 560))
        let small = NSRect(x: 0, y: 0, width: 800, height: 500)
        #expect(small.contains(frame.restored(in: [small], cascade: 1)))
        let legacy = try JSONDecoder().decode(SessionState.self, from: Data(#"{"last_vault":"/a","recent":[]}"#.utf8))
        #expect(legacy.windows == nil)
    }

    @Test func allWindowsParticipateInShutdownAndCancelLeavesDocumentsEditable() async throws {
        let a = try CloseTests.Fixture(), b = try CloseTests.Fixture()
        defer { a.cleanup(); b.cleanup() }
        let application = ApplicationWorkspace()
        let first = application.initial
        let second = WindowRecord(layout: Layout(arrangement: application.arrangement))
        application.records.append(second)
        first.workspace.tabs = [NoteTab(path: "note.md", document: a.document, loading: false)]
        second.workspace.tabs = [NoteTab(path: "note.md", document: b.document, loading: false)]
        a.document.edited(range: NSRange(location: 0, length: 0), replacement: "a")
        b.document.edited(range: NSRange(location: 0, length: 0), replacement: "b")
        a.makeUnwritable(); b.makeUnwritable()
        let gate = TerminationGate()
        var outcome: TerminationGate.Outcome?
        var cleaned = false
        gate.begin(documentTitles: ["a", "b"], documents: { application.prepareShutdown(completion: $0) },
                   cleanup: { done in cleaned = true; done() }) { outcome = $0 }
        for _ in 0..<500 where outcome == nil { try await Task.sleep(for: .milliseconds(10)) }
        if case .blocked(let unsaved) = outcome { #expect(unsaved.count == 2) } else { Issue.record("all windows must block") }
        #expect(!cleaned)
        a.makeWritable(); b.makeWritable()
        a.document.edited(range: NSRange(location: 0, length: 0), replacement: "after cancel")
        #expect(await a.document.prepareToClose() == .persisted)
        #expect(await b.document.prepareToClose() == .persisted)
    }
}
