import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite(.serialized) @MainActor
struct CloseTests {
    @Test func closeTabKeepsTabAndDocumentEditableWhenNothingPersists() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let document = fixture.document
        document.edited(range: NSRange(location: 0, length: 0), replacement: "未存")
        fixture.makeUnwritable()

        let workspace = Workspace()
        workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        workspace.selected = "note.md"
        var prompts = 0
        workspace.promptUnsaved = { notes, quitting in
            prompts += 1
            #expect(notes == ["note.md"])
            #expect(!quitting)
            return .cancel
        }

        #expect(await document.prepareToClose() == .unsaved)
        workspace.closeTab("note.md")
        for _ in 0..<500 where prompts == 0 { try await Task.sleep(for: .milliseconds(10)) }
        #expect(prompts == 1)
        #expect(workspace.tabs.map(\.path) == ["note.md"])
        #expect(workspace.selected == "note.md")

        // 取消后文档没有被置为 closed：修好权限还能正常保存和关闭。
        fixture.makeWritable()
        #expect(await document.prepareToClose() == .persisted)
        workspace.closeTab("note.md")
        for _ in 0..<500 where !workspace.tabs.isEmpty { try await Task.sleep(for: .milliseconds(10)) }
        #expect(workspace.tabs.isEmpty)
    }

    @Test func closeTabClosesWhenRecoveryDraftWasPersisted() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let document = fixture.document
        document.edited(range: NSRange(location: 0, length: 0), replacement: "恢复稿")
        // 笔记只读、恢复稿可写：最新正文已经安全落盘，不需要打扰用户。
        try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: fixture.url.path)

        let workspace = Workspace()
        workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        workspace.selected = "note.md"
        var prompts = 0
        workspace.promptUnsaved = { _, _ in
            prompts += 1
            return .cancel
        }

        #expect(await document.prepareToClose() == .recoveryDraft)
        workspace.closeTab("note.md")
        for _ in 0..<500 where !workspace.tabs.isEmpty { try await Task.sleep(for: .milliseconds(10)) }
        #expect(prompts == 0)
        #expect(workspace.tabs.isEmpty)
    }

    @MainActor
    final class Fixture {
        let directory: URL
        let vault: URL
        let url: URL
        let cache: URL
        let core: WorkspaceCore
        let document: NoteDocument
        private let recovery: URL

        init() throws {
            directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            vault = directory.appendingPathComponent("vault")
            cache = directory.appendingPathComponent("cache")
            try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
            url = vault.appendingPathComponent("note.md")
            try Data("原文\n".utf8).write(to: url)
            core = try WorkspaceCore.inDirectories(config: directory.appendingPathComponent("config").path,
                                                   cache: cache.path)
            recovery = cache.appendingPathComponent("recovery")
            try FileManager.default.createDirectory(at: recovery, withIntermediateDirectories: true)
            let session = try core.openDocument(vault: vault.path, path: "note.md")
            document = NoteDocument(io: DocumentIO(session: session, url: url), initial: try session.initial())
        }

        func makeUnwritable() {
            try? FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: recovery.path)
            try? FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: url.path)
            try? FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: vault.path)
        }

        func makeWritable() {
            try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: vault.path)
            try? FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)
            try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: recovery.path)
        }

        func cleanup() {
            makeWritable()
            try? FileManager.default.removeItem(at: directory)
        }
    }
}
