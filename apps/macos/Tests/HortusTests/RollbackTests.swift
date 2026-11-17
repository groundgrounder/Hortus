import AppKit
import HortusCore
import Testing
@testable import Hortus

/// 回滚只刷新干净标签；未保存编辑、组合输入和活动冲突必须被面板阻止。
@Suite(.serialized) @MainActor
struct RollbackTests {
    @MainActor
    final class Fixture {
        let directory: URL
        let vault: URL
        let url: URL
        let core: WorkspaceCore
        let document: NoteDocument

        init(_ source: String) throws {
            directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            vault = directory.appendingPathComponent("vault")
            try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
            url = vault.appendingPathComponent("note.md")
            try Data(source.utf8).write(to: url)
            core = try WorkspaceCore.inDirectories(
                config: directory.appendingPathComponent("config").path,
                cache: directory.appendingPathComponent("cache").path
            )
            let session = try core.openDocument(vault: vault.path, path: "note.md")
            document = NoteDocument(
                io: DocumentIO(session: session, url: url),
                initial: try session.initial()
            )
        }

        func workspace() -> Workspace {
            let workspace = Workspace()
            workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
            workspace.selected = "note.md"
            return workspace
        }

        func cleanup() {
            try? FileManager.default.removeItem(at: directory)
        }
    }

    @Test func cleanTabRefreshesButDirtyAndMarkedTabsAreProtected() async throws {
        let fixture = try Fixture("one\n")
        defer { fixture.cleanup() }
        let workspace = fixture.workspace()

        #expect(!workspace.hasUnsavedEdits(in: ["note.md"]))
        try Data("two\n".utf8).write(to: fixture.url)
        workspace.refreshAfterRollback(["note.md"])
        for _ in 0..<300 where fixture.document.text != "two\n" {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.document.text == "two\n")
        #expect(!fixture.document.dirty)

        fixture.document.edited(range: NSRange(location: 0, length: 0), replacement: "unsaved ")
        #expect(fixture.document.dirty)
        #expect(workspace.hasUnsavedEdits(in: ["note.md"]))
        let dirtyText = fixture.document.text
        try Data("external\n".utf8).write(to: fixture.url)
        workspace.refreshAfterRollback(["note.md"])
        try await Task.sleep(for: .milliseconds(250))
        #expect(fixture.document.text == dirtyText)
        #expect(fixture.document.dirty)

        fixture.document.dirty = false
        fixture.document.marked = true
        #expect(workspace.hasUnsavedEdits(in: ["note.md"]))
        fixture.document.marked = false

        fixture.document.issue = .conflict
        #expect(workspace.hasUnsavedEdits(in: ["note.md"]))
        workspace.refreshAfterRollback(["note.md"])
        try await Task.sleep(for: .milliseconds(100))
        #expect(fixture.document.issue == .conflict)
    }

    @Test func restoredDeletedFileExitsMissingAndMatchesDisk() async throws {
        let fixture = try Fixture("original\n")
        defer { fixture.cleanup() }
        let workspace = fixture.workspace()
        try FileManager.default.removeItem(at: fixture.url)
        fixture.document.refresh()
        for _ in 0..<300 where fixture.document.issue != .missing {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.document.issue == .missing)

        try Data("original\n".utf8).write(to: fixture.url)
        workspace.refreshAfterRollback(["note.md"])
        for _ in 0..<300 where fixture.document.issue != .none {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.document.issue == .none)
        #expect(fixture.document.text == "original\n")
        #expect(!fixture.document.dirty)
    }
}
