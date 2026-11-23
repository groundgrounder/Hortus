import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite @MainActor
struct FileOpsTests {
    private func entry(_ path: String, parent: String, directory: Bool) -> TreeEntry {
        TreeEntry(path: path, parent: parent, name: (path as NSString).lastPathComponent, directory: directory)
    }

    @Test func newEntryParentFollowsTreeSelection() {
        let workspace = Workspace()
        workspace.entries = [
            "docs": entry("docs", parent: "", directory: true),
            "docs/note.md": entry("docs/note.md", parent: "docs", directory: false),
        ]
        #expect(workspace.parentForNewEntry() == "")
        workspace.selected = "docs/note.md"
        #expect(workspace.parentForNewEntry() == "docs")
        workspace.folderSelection = "docs"
        #expect(workspace.parentForNewEntry() == "docs")
        workspace.folderSelection = nil
        workspace.selected = "docs"
        #expect(workspace.parentForNewEntry() == "docs")
        workspace.selected = nil
        #expect(workspace.parentForNewEntry() == "")
    }

    @Test func suggestedNamesNumberAroundCollisionsCaseInsensitively() {
        let workspace = Workspace()
        workspace.entries = [
            "new note.md": entry("new note.md", parent: "", directory: false),
            "New Folder": entry("New Folder", parent: "", directory: false),
            "other/New Note.md": entry("other/New Note.md", parent: "other", directory: false),
        ]
        #expect(workspace.suggestedName(base: "New Note", ext: "md", parent: "") == "New Note 2.md")
        #expect(workspace.suggestedName(base: "New Folder", ext: "", parent: "") == "New Folder 2")
        // 冲突只在同一父目录内计数。
        #expect(workspace.suggestedName(base: "New Note", ext: "md", parent: "other") == "New Note 2.md")
        #expect(workspace.suggestedName(base: "Fresh", ext: "md", parent: "") == "Fresh.md")
    }

    @Test func localNameValidationMirrorsVaultRules() {
        for name in ["", "  ", ".", "..", ".hidden", "node_modules", "a/b", "back\\slash", "nul\0", "ctrl\u{1}x"] {
            #expect(FileNaming.localError(name) != nil)
        }
        for name in ["idea", "Draft.markdown", "备忘 录", "  trimmed  "] {
            #expect(FileNaming.localError(name) == nil)
        }
    }

    @Test func applyRenameRemapsTabsSelectionAndViews() {
        let workspace = Workspace()
        workspace.entries = [
            "docs": entry("docs", parent: "", directory: true),
            "docs/inside.md": entry("docs/inside.md", parent: "docs", directory: false),
            "docs/deep": entry("docs/deep", parent: "docs", directory: true),
            "docs/deep/leaf.md": entry("docs/deep/leaf.md", parent: "docs/deep", directory: false),
        ]
        workspace.tabs = [NoteTab(path: "docs/inside.md", loading: false), NoteTab(path: "docs/deep/leaf.md", loading: false)]
        workspace.selected = "docs/inside.md"
        workspace.restoredViews = ["docs/inside.md": "editing", "docs/deep/leaf.md": "reading",
                                   "docs/closed.md": "editing"]
        workspace.closingTabs = ["docs/closed.md"]
        workspace.folderSelection = "docs/deep"
        let moved = TreeEntry(path: "papers", parent: "", name: "papers", directory: true)
        workspace.applyRename(oldPath: "docs", entry: moved)
        #expect(workspace.tabs.map(\.path) == ["papers/inside.md", "papers/deep/leaf.md"])
        #expect(workspace.selected == "papers/inside.md")
        #expect(workspace.restoredViews["papers/inside.md"] == "editing")
        #expect(workspace.restoredViews["papers/deep/leaf.md"] == "reading")
        #expect(workspace.restoredViews["papers/closed.md"] == "editing")
        #expect(workspace.closingTabs == ["papers/closed.md"])
        #expect(workspace.folderSelection == "papers/deep")
        // 单文件重命名不影响同目录其他标签。
        let single = TreeEntry(path: "papers/inbox.md", parent: "papers", name: "inbox.md", directory: false)
        workspace.applyRename(oldPath: "papers/inside.md", entry: single)
        #expect(workspace.tabs.map(\.path) == ["papers/inbox.md", "papers/deep/leaf.md"])
        #expect(workspace.selected == "papers/inbox.md")
    }

    @Test func treeSelectionSyncsFolderSelectionAndClearsOnDeselect() throws {
        let workspace = Workspace()
        workspace.entries = [
            "docs": entry("docs", parent: "", directory: true),
            "docs/note.md": entry("docs/note.md", parent: "docs", directory: false),
        ]
        let coordinator = FileTree.Coordinator(workspace: workspace)
        let outline = NSOutlineView()
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.delegate = coordinator
        outline.dataSource = coordinator
        coordinator.update(outline, version: 0, selected: nil, root: "/fixture")
        let folder = try #require(coordinator.nodes["docs"])
        let note = try #require(coordinator.nodes["docs/note.md"])
        outline.selectRowIndexes(IndexSet(integer: outline.row(forItem: folder)), byExtendingSelection: false)
        #expect(workspace.folderSelection == "docs")
        #expect(workspace.parentForNewEntry() == "docs")
        outline.deselectAll(nil)
        #expect(workspace.folderSelection == nil)
        outline.selectRowIndexes(IndexSet(integer: outline.row(forItem: folder)), byExtendingSelection: false)
        #expect(workspace.folderSelection == "docs")
        outline.expandItem(folder)
        let noteRow = outline.row(forItem: note)
        #expect(noteRow >= 0)
        outline.selectRowIndexes(IndexSet(integer: noteRow), byExtendingSelection: false)
        #expect(workspace.folderSelection == nil)
    }

    @Test func blankAreaMenuOffersRootCreationOnly() {
        let workspace = Workspace()
        let coordinator = FileTree.Coordinator(workspace: workspace)
        let event = NSEvent.mouseEvent(with: .rightMouseDown, location: .zero, modifierFlags: [],
                                       timestamp: 0, windowNumber: 0, context: nil,
                                       eventNumber: 0, clickCount: 1, pressure: 0)!
        let outline = NSOutlineView()
        let menu = coordinator.menu(for: event, outline: outline)
        #expect(menu?.items.count == 2)
        #expect(menu?.items[0].title == L10n.string("New Note"))
        #expect(menu?.items[1].title == L10n.string("New Folder"))
        #expect(coordinator.trashSelection(outline: outline) == false)
    }
}

/// 真实内核下的文件管理闭环：新建、跨窗口重命名迁移与废纸篓保护。
@Suite(.serialized) @MainActor
struct FileOpsIntegrationTests {
    @MainActor
    final class Fixture {
        let directory: URL
        let vault: URL
        let cache: URL
        let core: WorkspaceCore
        let handle: VaultHandle
        let workspace: Workspace
        var trashed: [URL] = []
        let application: ApplicationWorkspace

        init() throws {
            directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            vault = directory.appendingPathComponent("vault")
            cache = directory.appendingPathComponent("cache")
            try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
            try "原文\n".write(to: vault.appendingPathComponent("note.md"), atomically: true, encoding: .utf8)
            core = try WorkspaceCore.inDirectories(config: directory.appendingPathComponent("config").path,
                                                   cache: cache.path)
            handle = try core.openVault(path: vault.path)
            // 使用应用初始记录的工作区，让跨窗口协调路径（records 扫描）真实生效。
            application = ApplicationWorkspace()
            application.core = core
            workspace = application.initial.workspace
            application.configure(application.initial)
            let context = VaultContext(handle: handle)
            workspace.context = context
            // 与真实打开路径一致：应用级注册表持有库上下文，重命名迁移才能立即写回缓存。
            application.contexts[context.root] = context
            // 扫描批次没有消费者；条目由测试直接写入库级缓存。
            context.entries = [
                "note.md": TreeEntry(path: "note.md", parent: "", name: "note.md", directory: false),
            ]
            // 废纸篓路径的恢复稿目录由检查预先建立，便于断言“无残留”。
            try FileManager.default.createDirectory(atPath: cache.appendingPathComponent("recovery").path,
                                                    withIntermediateDirectories: true)
        }

        var recoveryDirectory: URL { cache.appendingPathComponent("recovery") }
        func cleanup() {
            for url in trashed { try? FileManager.default.removeItem(at: url) }
            handle.close()
            try? FileManager.default.removeItem(at: directory)
        }

        func openDocument(_ path: String) throws -> NoteDocument {
            let session = try core.openDocument(vault: vault.path, path: path)
            return NoteDocument(io: DocumentIO(session: session, url: vault.appendingPathComponent(path)),
                               initial: try session.initial())
        }
    }

    @Test func createNoteOpensEditingViewAndFocusesContent() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let created = try await fixture.workspace.createNote(parent: "", name: "Idea")
        #expect(created.path == "Idea.md")
        #expect((try String(contentsOfFile: fixture.vault.appendingPathComponent("Idea.md").path, encoding: .utf8)).isEmpty)
        #expect(fixture.workspace.entries["Idea.md"] != nil)
        // 新笔记以编辑视图打开，且进入未保存编辑队列。
        for _ in 0..<200 where fixture.workspace.current?.document == nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        let document = fixture.workspace.current?.document
        #expect(document != nil)
        #expect(document?.reading == false)
        #expect(fixture.workspace.pendingEditingPaths.isEmpty)
        document?.edited(range: NSRange(location: 0, length: 0), replacement: "草稿")
        await document?.saveNow()
        #expect(try String(contentsOfFile: fixture.vault.appendingPathComponent("Idea.md").path, encoding: .utf8) == "草稿")
    }

    @Test func renameMigratesSessionRecoveryAndBaseline() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let document = try fixture.openDocument("note.md")
        fixture.workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        fixture.workspace.selected = "note.md"
        document.edited(range: NSRange(location: 0, length: 0), replacement: "未存 ")
        await document.checkpoint()
        let moved = try await fixture.workspace.renameEntry(path: "note.md", newName: "renamed.md")
        #expect(moved.path == "renamed.md")
        // 树缓存立即换键并换显示名，不等监听批次。
        #expect(fixture.workspace.entries["renamed.md"]?.name == "renamed.md")
        #expect(fixture.workspace.entries["renamed.md"]?.path == "renamed.md")
        #expect(fixture.workspace.entries["note.md"] == nil)
        // 跨窗口协调由 ApplicationWorkspace.entryRenamed 完成：初始记录即本工作区。
        fixture.workspace.applyRename(oldPath: "note.md", entry: moved)
        #expect(fixture.workspace.tabs.map(\.path) == ["renamed.md"])
        #expect(fixture.workspace.selected == "renamed.md")
        // 等待文档队列上的路径迁移完成，保存必然排在其后。
        for _ in 0..<300 where document.io.url.lastPathComponent != "renamed.md" {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(document.io.url.lastPathComponent == "renamed.md")
        await document.saveNow()
        #expect(document.issue != .missing && document.issue != .conflict)
        #expect(try String(contentsOfFile: fixture.vault.appendingPathComponent("renamed.md").path, encoding: .utf8) == "未存 原文\n")
        // 恢复稿不残留在旧路径。
        #expect(try FileManager.default.contentsOfDirectory(atPath: fixture.recoveryDirectory.path).isEmpty)
    }

    @Test func renameAndTrashFinishDiagramStateChangesBeforeReturning() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let root = fixture.handle.root()
        try fixture.core.rememberDiagram(vault: root, note: "note.md", block: 1, collapsed: ["parent"])
        let moved = try await fixture.workspace.renameEntry(path: "note.md", newName: "renamed.md")
        #expect(try fixture.core.collapsedNodes(vault: root, note: "note.md", block: 1).isEmpty)
        #expect(try fixture.core.collapsedNodes(vault: root, note: moved.path, block: 1) == ["parent"])

        let trash = fixture.directory.appendingPathComponent("trashed.md")
        fixture.workspace.moveToTrash = { source in
            try FileManager.default.moveItem(at: source, to: trash)
            return trash
        }
        let removed = try await fixture.workspace.trashEntry(path: moved.path)
        #expect(removed == trash.path)
        #expect(try fixture.core.collapsedNodes(vault: root, note: moved.path, block: 1).isEmpty)
        _ = try await fixture.workspace.createNote(parent: "", name: "renamed")
        #expect(try fixture.core.collapsedNodes(vault: root, note: moved.path, block: 1).isEmpty)
    }

    @Test func trashProtectsUnsavedDraftsUntilConfirmed() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        FileOpsOutcome.lastTrashResult = nil
        let document = try fixture.openDocument("note.md")
        fixture.workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        fixture.workspace.selected = "note.md"
        document.edited(range: NSRange(location: 0, length: 0), replacement: "改动")
        var prompted: [String] = []
        fixture.workspace.promptTrash = { unsaved, target in
            prompted = unsaved
            #expect(target == "note.md")
            return false
        }
        fixture.workspace.requestTrash(path: "note.md")
        #expect(prompted == ["note.md"])
        // 取消：标签、未保存正文与磁盘都保持原样。
        #expect(fixture.workspace.tabs.map(\.path) == ["note.md"])
        let cancelled = try await document.io.perform { try $0.snapshot() }
        #expect(cancelled.text.hasPrefix("改动"))
        #expect(FileManager.default.fileExists(atPath: fixture.vault.appendingPathComponent("note.md").path))

        fixture.workspace.promptTrash = { _, _ in true }
        fixture.workspace.requestTrash(path: "note.md")
        // 磁盘移动成功后才关闭标签；记录 resultingItemURL 并在清理时删除。
        var resulting: String?
        for _ in 0..<300 {
            resulting = (FileOpsOutcome.lastTrashResult?["resulting"]) ?? nil
            if resulting != nil { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        for _ in 0..<300 where !fixture.workspace.tabs.isEmpty {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.workspace.tabs.isEmpty)
        #expect(resulting != nil)
        #expect(FileManager.default.fileExists(atPath: resulting!))
        fixture.trashed.append(URL(fileURLWithPath: resulting!))
        #expect(!FileManager.default.fileExists(atPath: fixture.vault.appendingPathComponent("note.md").path))
        #expect(fixture.workspace.entries["note.md"] == nil)
        #expect(try FileManager.default.contentsOfDirectory(atPath: fixture.recoveryDirectory.path).isEmpty)
    }

    @Test func focusExitsOnRenameTrashAndSameNoteNavigation() throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let document = try fixture.openDocument("note.md")
        fixture.workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        fixture.workspace.selected = "note.md"
        func session(note: String = "note.md") -> DiagramFocusSession {
            DiagramFocusSession(note: note, vault: fixture.vault.path, ordinal: 1, document: document,
                                reading: true, selection: NSRange(location: 0, length: 0),
                                scroll: .zero, initial: nil)
        }

        fixture.workspace.focusSession = session()
        fixture.workspace.openSearchResult(path: "note.md", start: 0, length: 0)
        #expect(fixture.workspace.focusSession == nil)

        fixture.workspace.focusSession = session()
        fixture.workspace.applyRename(oldPath: "note.md",
                                      entry: TreeEntry(path: "renamed.md", parent: "", name: "renamed.md",
                                                       directory: false))
        #expect(fixture.workspace.focusSession == nil)

        fixture.workspace.focusSession = session(note: "renamed.md")
        fixture.workspace.discardTabForTrash("renamed.md")
        #expect(fixture.workspace.focusSession == nil)
    }

    @Test func trashPromptIncludesRecoveryDrafts() throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let document = try fixture.openDocument("note.md")
        document.issue = .recovery
        fixture.workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        fixture.workspace.selected = "note.md"
        var prompted: [String] = []
        fixture.workspace.promptTrash = { unsaved, target in
            prompted = unsaved
            #expect(target == "note.md")
            return false
        }
        fixture.workspace.requestTrash(path: "note.md")
        #expect(prompted == ["note.md"])
        #expect(fixture.workspace.tabs.map(\.path) == ["note.md"])
    }

    @Test func trashFailureKeepsTabsLiveTextAndRecoveryDraft() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let document = try fixture.openDocument("note.md")
        fixture.workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        fixture.workspace.selected = "note.md"
        document.edited(range: NSRange(location: 0, length: 0), replacement: "草稿 ")
        await document.checkpoint()
        #expect(!(try FileManager.default.contentsOfDirectory(atPath: fixture.recoveryDirectory.path).isEmpty))
        fixture.workspace.promptTrash = { _, _ in true }
        fixture.workspace.moveToTrash = { _ in throw CocoaError(.fileWriteUnknown) }
        fixture.workspace.requestTrash(path: "note.md")
        for _ in 0..<300 where fixture.workspace.error == nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.workspace.error != nil)
        #expect(fixture.workspace.tabs.map(\.path) == ["note.md"])
        #expect(FileManager.default.fileExists(atPath: fixture.vault.appendingPathComponent("note.md").path))
        let live = try await document.io.perform { try $0.snapshot() }
        #expect(live.text.hasPrefix("草稿 "))
        #expect(!(try FileManager.default.contentsOfDirectory(atPath: fixture.recoveryDirectory.path).isEmpty))
    }

    @Test func dirtyBacklinkUsesLiveOffsets() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        try "prefix\n\n[[note|Alias]]\n".write(to: fixture.vault.appendingPathComponent("source.md"),
                                             atomically: true, encoding: .utf8)
        let document = try fixture.openDocument("source.md")
        fixture.workspace.tabs = [NoteTab(path: "source.md", document: document, loading: false)]
        fixture.workspace.selected = "source.md"
        document.edited(range: NSRange(location: 0, length: 0), replacement: "插入\n")
        #expect(document.dirty)
        let live = try await document.io.perform { try $0.snapshot().text }
        let linkRange = (live as NSString).range(of: "[[note|Alias]]")
        let stale = ("prefix\n\n" as NSString).length
        let entry = BacklinkInfo(source: "source.md", line: 3, target: "note", heading: nil,
                                 alias: "Alias", embed: false, inDiagram: false, context: "",
                                 startUtf16: UInt64(stale), lengthUtf16: UInt64(linkRange.length))
        fixture.workspace.openBacklink(entry)
        for _ in 0..<300 where document.sourceAnchor != linkRange.location {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(document.sourceAnchor == linkRange.location)
        #expect(document.sourceSelectionLength == linkRange.length)
        fixture.workspace.discardTabForTrash("source.md")
    }

    @Test func renameDuringAsyncCloseRemapsTheCloseRequest() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanup() }
        let document = try fixture.openDocument("note.md")
        fixture.workspace.tabs = [NoteTab(path: "note.md", document: document, loading: false)]
        fixture.workspace.selected = "note.md"
        fixture.workspace.closeTab()
        #expect(fixture.workspace.closingTabs == ["note.md"])

        try FileManager.default.moveItem(at: fixture.vault.appendingPathComponent("note.md"),
                                         to: fixture.vault.appendingPathComponent("renamed.md"))
        fixture.workspace.applyRename(oldPath: "note.md",
                                      entry: TreeEntry(path: "renamed.md", parent: "", name: "renamed.md",
                                                       directory: false))
        #expect(fixture.workspace.closingTabs == ["renamed.md"])
        for _ in 0..<300 where !fixture.workspace.tabs.isEmpty {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.workspace.tabs.isEmpty)
        #expect(fixture.workspace.closingTabs.isEmpty)
    }
}
