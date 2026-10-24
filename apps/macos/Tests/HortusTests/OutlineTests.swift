import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite(.serialized) @MainActor
struct OutlineTests {
    private func ready(_ document: NoteDocument) async throws {
        for _ in 0..<500 {
            if document.outlineRevision == document.revision { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        Issue.record("等待大纲超时")
    }

    @Test func unsavedEditsAndExternalReloadUpdateOutline() async throws {
        let fixture = try EditingTests.Fixture("# First😀\n\nBody\n")
        try await fixture.ready(); try await ready(fixture.document)
        #expect(fixture.document.headings.map(\.title) == ["First😀"])
        let range = (fixture.view.string as NSString).range(of: "First😀")
        fixture.view.insertText("修改", replacementRange: range)
        try await ready(fixture.document)
        #expect(fixture.document.dirty)
        #expect(fixture.document.headings.map(\.title) == ["修改"])
        await fixture.document.saveNow()
        try Data("Setext\n===\n\n## External\n".utf8).write(to: fixture.document.io.url)
        fixture.document.refresh()
        for _ in 0..<500 {
            if fixture.document.headings.map(\.title) == ["Setext", "External"] { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.document.headings.map(\.level) == [1, 2])
        #expect(fixture.document.headings.map(\.title) == ["Setext", "External"])
        await fixture.close()
    }

    @Test func outlineActionsRevealMatchingHeadingInBothViewsAndTabs() async throws {
        let source = (0..<80).map { "## Heading \($0)😀\n\nBody **text**.\n\n" }.joined()
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(); try await ready(fixture.document)
        let workspace = Workspace()
        workspace.tabs = [NoteTab(path: "note.md", document: fixture.document, loading: false)]
        workspace.selected = "note.md"
        let heading = try #require(fixture.document.headings.last)
        workspace.revealHeading(heading)
        fixture.view.layoutSubtreeIfNeeded()
        let display = fixture.coordinator.displayPosition(source: Int(heading.start))
        #expect(fixture.view.visibleRect.intersects(fixture.coordinator.rect(at: display)))
        #expect(fixture.document.sourceAnchor == Int(heading.start))
        await fixture.close()
        let editing = try EditingTests.Fixture(source)
        try await editing.ready(); try await ready(editing.document)
        editing.document.reading = false
        workspace.tabs = [NoteTab(path: "note.md", document: editing.document, loading: false)]
        workspace.revealHeading(heading)
        #expect(editing.view.selectedRange() == NSRange(location: Int(heading.start), length: 0))
        let caret = editing.view.firstRect(forCharacterRange: editing.view.selectedRange(), actualRange: nil)
        let local = editing.view.convert(editing.window.convertFromScreen(caret), from: nil)
        #expect(editing.view.visibleRect.intersects(local))
        let second = try EditingTests.Fixture("# Other note\n")
        try await ready(second.document)
        workspace.tabs.append(NoteTab(path: "other.md", document: second.document, loading: false))
        workspace.selected = "other.md"
        #expect(workspace.current?.document?.headings.first?.title == "Other note")
        workspace.selected = "note.md"
        #expect(workspace.current?.document?.headings.count == 80)
        await second.close(); await editing.close()
    }
}
