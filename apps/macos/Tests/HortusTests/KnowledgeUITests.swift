import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite @MainActor
struct KnowledgeUITests {
    @Test func parentTagCountsDeduplicateDescendantNotes() {
        let rows = TagPanelRow.rows([
            TagSummaryInfo(tag: "service", raw: "Service", count: 1, notes: ["a.md"]),
            TagSummaryInfo(tag: "service/worker", raw: "service/worker", count: 2, notes: ["a.md", "b.md"]),
            TagSummaryInfo(tag: "transport/udp", raw: "transport/udp", count: 1, notes: ["c.md"]),
        ])
        #expect(rows.map(\.id) == ["service", "service/worker", "transport", "transport/udp"])
        #expect(rows.map(\.name) == ["Service", "service/worker", "transport", "transport/udp"])
        #expect(rows.map(\.count) == [2, 2, 1, 1])
        #expect(rows.map(\.depth) == [0, 1, 0, 1])
    }

    @Test func selectingTagRevealsItsExistingDockAndClearsTheFilter() throws {
        let workspace = Workspace()
        let application = ApplicationWorkspace()
        let layout = application.initial.layout
        layout.moveToRegion(.tool(.tags), .right)
        layout.visible[.right] = false
        workspace.tagFilter = "old filter"
        // 拼装生产窗口的布局，不依赖 AppKit 窗口。
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let core = try WorkspaceCore.inDirectories(config: temporary.appendingPathComponent("config").path,
                                                    cache: temporary.appendingPathComponent("cache").path)
        workspace.configure(application: application, core: core, layout: layout)
        workspace.showTag("service/worker")
        #expect(workspace.selectedTag == "service/worker" && workspace.tagFilter.isEmpty)
        #expect(layout.isShown(.right))
        #expect(layout.region(containing: .tool(.tags)) == .right)
        #expect(layout.group(containing: .tool(.tags))?.selected == .tool(.tags))
    }

    #if DEBUG
    @Test func renameSelfReferenceWaitsForRelocationAndSkipsLoadingTabs() async throws {
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let root = temporary.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try "[[journal]]\n\n正文\n".write(to: root.appendingPathComponent("journal.md"),
                                        atomically: true, encoding: .utf8)
        try "[[journal]]\n".write(to: root.appendingPathComponent("loader.md"),
                                  atomically: true, encoding: .utf8)
        let core = try WorkspaceCore.inDirectories(config: temporary.appendingPathComponent("config").path,
                                                    cache: temporary.appendingPathComponent("cache").path)
        let application = ApplicationWorkspace()
        application.core = core
        let first = application.initial
        application.configure(first)
        let context = VaultContext(handle: try core.openVault(path: root.path))
        application.contexts[context.root] = context
        await first.workspace.attach(context, state: nil)
        defer {
            first.workspace.discardTabForTrash("renamed.md")
            first.workspace.discardTabForTrash("loader.md")
            context.handle.close()
            try? FileManager.default.removeItem(at: temporary)
        }
        first.workspace.openNote("journal.md")
        let deadline = ContinuousClock.now + .seconds(5)
        while (first.workspace.current?.document == nil || context.searchIndexReadyMilliseconds == nil),
              ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        // 仍在加载的候选标签必须被安全跳过，而不是未加锁地并发改写。
        first.workspace.tabs.append(NoteTab(path: "loader.md"))
        first.workspace.inspectRenameLinks = { _ in .update }
        let renamed = try await first.workspace.renameNoteWithPreview(path: "journal.md",
                                                                     newName: "renamed.md", window: nil)
        #expect(renamed?.path == "renamed.md")
        let document = try #require(first.workspace.current?.document)
        let refreshDeadline = ContinuousClock.now + .seconds(3)
        while !document.text.contains("[[renamed]]"), ContinuousClock.now < refreshDeadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(document.text.contains("[[renamed]]"))
        #expect(document.issue != .missing)
        let content = try String(contentsOf: root.appendingPathComponent("renamed.md"), encoding: .utf8)
        #expect(content.hasPrefix("[[renamed]]"))
        #expect(first.workspace.knowledgeMessage?.contains("loader.md") == true)
        let untouched = try String(contentsOf: root.appendingPathComponent("loader.md"), encoding: .utf8)
        #expect(untouched == "[[journal]]\n")
    }

    @Test func renamePreviewHonorsCancelAndSkipsUnsavedNotesAcrossWindows() async throws {
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let root = temporary.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let old = root.appendingPathComponent("journal.md")
        let clean = root.appendingPathComponent("reader.md")
        let dirty = root.appendingPathComponent("worker.md")
        let link = "![[journal#Queue|Retry log]]"
        try "---\naliases: [Ledger]\n---\n## Queue\n".write(to: old, atomically: true, encoding: .utf8)
        try (link + "\n[[Ledger]]\n").write(to: clean, atomically: true, encoding: .utf8)
        try link.write(to: dirty, atomically: true, encoding: .utf8)
        let core = try WorkspaceCore.inDirectories(config: temporary.appendingPathComponent("config").path,
                                                    cache: temporary.appendingPathComponent("cache").path)
        let application = ApplicationWorkspace()
        application.core = core
        let first = application.initial
        let second = WindowRecord(layout: Layout())
        application.records.append(second)
        application.configure(first)
        application.configure(second)
        let context = VaultContext(handle: try core.openVault(path: root.path))
        application.contexts[context.root] = context
        await first.workspace.attach(context, state: nil)
        await second.workspace.attach(context, state: nil)
        defer {
            first.workspace.discardTabForTrash("reader.md")
            second.workspace.discardTabForTrash("worker.md")
            context.handle.close()
        }
        first.workspace.openNote("reader.md")
        second.workspace.openNote("worker.md")
        let deadline = ContinuousClock.now + .seconds(5)
        while (first.workspace.current?.document == nil || second.workspace.current?.document == nil
               || context.searchIndexReadyMilliseconds == nil), ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        let doc = try #require(second.workspace.current?.document)
        doc.edited(range: NSRange(location: 0, length: 0), replacement: "Pending ")
        doc.compositionChanged(true)
        var decisions = 0
        first.workspace.inspectRenameLinks = { plan in
            decisions += 1
            #expect(plan.affectedNotes == 2 && plan.totalRewrites == 2)
            return .cancel
        }
        let cancelled = try await first.workspace.renameNoteWithPreview(path: "journal.md", newName: "queue.md", window: nil)
        #expect(cancelled == nil && FileManager.default.fileExists(atPath: old.path))
        first.workspace.inspectRenameLinks = { plan in
            decisions += 1
            #expect(plan.files.first?.edits.first?.after == "![[queue#Queue|Retry log]]")
            return .update
        }
        let renamed = try await first.workspace.renameNoteWithPreview(path: "journal.md", newName: "queue.md", window: nil)
        #expect(decisions == 2 && renamed?.path == "queue.md")
        #expect(try String(contentsOf: clean, encoding: .utf8) == "![[queue#Queue|Retry log]]\n[[Ledger]]\n")
        #expect(try String(contentsOf: dirty, encoding: .utf8) == link)
        #expect(doc.dirty && first.workspace.knowledgeMessage?.contains("worker.md") == true)
        #expect(try await doc.io.perform { try $0.snapshot().text } == "Pending " + link)
        #expect(context.handle.backlinks(path: "queue.md").map(\.target) == ["queue", "Ledger"])
        let refreshDeadline = ContinuousClock.now + .seconds(3)
        while first.workspace.current?.document?.text.contains("[[queue") == false, ContinuousClock.now < refreshDeadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(first.workspace.current?.document?.text.contains("[[queue") == true)
    }
    #endif
}
