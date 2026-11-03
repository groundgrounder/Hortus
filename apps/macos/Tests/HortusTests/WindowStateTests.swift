import AppKit
import Testing
@testable import Hortus

@Suite @MainActor
struct WindowStateTests {
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
