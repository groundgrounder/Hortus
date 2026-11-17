import AppKit
import Testing
@testable import Hortus

@Suite(.serialized) @MainActor
struct CleanupTests {
    @Test func clearingAttachedContentDetachesDiagramsIdempotently() async throws {
        let fixture = try EditingTests.Fixture(EditingTests.source)
        try await fixture.ready()
        #expect(!fixture.view.blocks.isEmpty)
        #expect(!fixture.view.diagramViews.isEmpty)
        fixture.view.clearAttachedContent()
        fixture.view.clearAttachedContent()
        #expect(fixture.view.blocks.isEmpty)
        #expect(fixture.view.diagramViews.isEmpty)
        #expect(fixture.view.mediaViews.isEmpty && fixture.view.mediaBlocks.isEmpty && fixture.view.mediaProjection.isEmpty)
        #expect(fixture.view.mathViews.isEmpty && fixture.view.inlineMathViews.isEmpty && fixture.view.inlineMath.isEmpty)
        #expect(fixture.view.formulaAccessibilityBuildCount == 0)
        await fixture.close()
    }

    @Test func tearingDownForRemovalIsIdempotent() {
        _ = NSApplication.shared
        let view = DiagramTextView(diagramLayout: true)
        view.clearAttachedContent()
        view.tearDownForRemoval()
        view.tearDownForRemoval()
        #expect(view.diagramStorage == nil)
        #expect(view.string.isEmpty)
        #expect(view.editingCoordinator == nil && view.readingCoordinator == nil)
    }

    @Test func documentResolvesItsMountedTextViewWithoutWindowScan() async throws {
        let fixture = try EditingTests.Fixture(EditingTests.source)
        #expect(fixture.document.mountedTextView === fixture.view)
        fixture.coordinator.detach()
        #expect(fixture.document.mountedTextView == nil)
        await fixture.close()
    }

    @Test func readingDiagramClickForwardsTheSourceRangeWithoutMutatingDocument() async throws {
        let source = "前言\n\n```mermaid\nflowchart TB\nA[甲] --> B[乙]\n```\n"
        let f = try ReadingTests.Fixture(source)
        var requested: (start: Int, length: Int)?
        f.openSource = { start, length in requested = (start, length) }
        try await f.start()
        let block = try #require(f.view.blocks.first)
        let range = try #require(block.prepared?.sourceRanges.values.first)
        #expect(block.revision == f.document.revision)
        f.view.onSourceRange?(block.ordinal, range)
        #expect(requested?.start == range.location)
        #expect(requested?.length == range.length)
        #expect(f.document.reading)
        #expect(f.document.sourceAnchor == 0)
        await f.close()
    }

    @Test func readingDiagramClickSwitchesWorkspaceToEditingAtTheSourceRange() async throws {
        let source = "前言\n\n```mermaid\nflowchart TB\nA[甲] --> B[乙]\n```\n"
        let f = try ReadingTests.Fixture(source)
        let workspace = Workspace()
        workspace.tabs = [NoteTab(path: "note.md", document: f.document, loading: false)]
        workspace.selected = "note.md"
        f.openSource = { start, length in
            workspace.switchToEditing(document: f.document, anchor: start, selectionLength: length)
        }
        try await f.start()
        let block = try #require(f.view.blocks.first)
        let range = try #require(block.prepared?.sourceRanges.values.first)
        f.view.onSourceRange?(block.ordinal, range)
        for _ in 0..<500 where f.document.reading { try await Task.sleep(for: .milliseconds(10)) }
        #expect(!f.document.reading)
        #expect(f.document.sourceAnchor == range.location)
        #expect(f.document.sourceSelectionLength == range.length)
        await f.close()
    }

    @Test func readingSourceSwitchSharesTheSerialQueueAndKeepsTheLastAnchor() async throws {
        _ = NSApplication.shared
        let fixture = try CloseTests.Fixture()
        defer { fixture.cleanup() }
        let workspace = Workspace()
        workspace.tabs = [NoteTab(path: "note.md", document: fixture.document, loading: false)]
        workspace.selected = "note.md"
        #expect(fixture.document.reading)
        workspace.switchToEditing(document: fixture.document, anchor: 2, selectionLength: 1)
        workspace.switchToEditing(document: fixture.document, anchor: 3, selectionLength: 0)
        for _ in 0..<500 where fixture.document.reading { try await Task.sleep(for: .milliseconds(10)) }
        #expect(!fixture.document.reading)
        #expect(fixture.document.sourceAnchor == 3)
        #expect(fixture.document.sourceSelectionLength == 0)
    }
}
