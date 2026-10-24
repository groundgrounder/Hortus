import AppKit
import HortusCore
import Testing
@testable import Hortus

@MainActor private func eventually(_ predicate: () -> Bool) async throws {
    for _ in 0..<500 {
        if predicate() { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    Issue.record("等待阅读视图超时")
}

@Suite(.serialized) @MainActor
struct ReadingTests {
    @MainActor final class Fixture {
        let editing: EditingTests.Fixture
        let coordinator = ReadingText.Coordinator()
        var view: DiagramTextView { editing.view }
        var document: NoteDocument { editing.document }
        init(_ source: String) throws { editing = try EditingTests.Fixture(source) }
        func start() async throws {
            try await editing.ready()
            editing.coordinator.task?.cancel(); editing.coordinator.styling.reset()
            view.delegate = nil; view.textStorage?.delegate = nil
            view.onCommit = nil; view.onCompositionState = nil
            view.isReading = true; view.isEditable = false; view.allowsUndo = false
            view.readingCoordinator = coordinator; coordinator.view = view
            coordinator.update(ReadingText(document: document, path: "note.md", vault: editing.vault.path,
                fontSize: 15, service: editing.service, core: editing.core) { _, _ in })
            try await ready()
        }
        func ready() async throws {
            try await eventually { !self.coordinator.pending && self.coordinator.model?.revision == self.document.revision }
            view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
        }
        func close() async {
            coordinator.detach(); view.readingCoordinator = nil
            await editing.close()
        }
    }

    @Test func displayCopySourceCopyFindAndHeadingRotor() async throws {
        let source = "# Heading😀\n\nA **bold** word and [[Target|Alias]].\n\n- [ ] Task\n"
        let f = try Fixture(source); try await f.start()
        #expect(!f.view.isEditable && f.view.isSelectable)
        for marker in ["#", "**", "[[", "- [ ]"] { #expect(!f.view.string.contains(marker)) }
        f.view.setSelectedRange(NSRange(location: 0, length: f.view.sourceUTF16Length))
        f.view.copy(nil)
        #expect(NSPasteboard.general.string(forType: .string) == f.view.string)
        f.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == source)
        let rotor = try #require(f.view.accessibilityCustomRotors().first)
        let search = NSAccessibilityCustomRotor.SearchParameters()
        search.searchDirection = .next; search.filterString = ""
        #expect(f.view.rotor(rotor, resultFor: search)?.customLabel == L10n.string("Heading level %1$@, %2$@", "1", "Heading😀"))
        f.view.setSelectedRange((f.view.string as NSString).range(of: "Alias"))
        let item = NSMenuItem(); item.tag = NSTextFinder.Action.setSearchString.rawValue
        f.view.performFindPanelAction(item)
        item.tag = NSTextFinder.Action.showFindInterface.rawValue; f.view.performFindPanelAction(item)
        #expect(f.view.enclosingScrollView?.isFindBarVisible == true)
        #expect(!f.document.dirty)
        await f.close()
    }

    @Test func twoHundredLocationsStayInTheSameBlockAndAreVisible() async throws {
        let source = (0..<220).map { "## Heading \($0)😀\n\nText **bold** and [[Target|Alias]] \($0).\n\n" }.joined()
        let f = try Fixture(source); try await f.start()
        let blocks = try #require(f.coordinator.model?.blocks)
        var seed: UInt64 = 0x106
        for _ in 0..<200 {
            seed = seed &* 6364136223846793005 &+ 1
            let index = Int(seed >> 32) % blocks.count
            let block = blocks[index]
            let source = Int(block.sourceStart + block.sourceLength / 2)
            let display = f.coordinator.displayPosition(source: source)
            let back = f.coordinator.sourcePosition(at: display)
            #expect(back >= Int(block.sourceStart) && back < Int(block.sourceStart + block.sourceLength))
            f.coordinator.reveal(source: source)
            f.view.layoutSubtreeIfNeeded()
            #expect(f.view.visibleRect.intersects(f.coordinator.rect(at: display)))
        }
        await f.close()
    }

    @Test func mouseTaskSavesExactlyOneSourceCharacter() async throws {
        let source = "前言😀\n\n- [ ] First\n- [x] Second\n\nEnd\n"
        let f = try Fixture(source); try await f.start()
        let check = (f.view.string as NSString).range(of: "☐")
        let rect = f.view.firstRect(forCharacterRange: check, actualRange: nil)
        let point = f.editing.window.convertPoint(fromScreen: CGPoint(x: rect.minX + 2, y: rect.midY))
        let event = try #require(NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [], timestamp: 0,
            windowNumber: f.editing.window.windowNumber, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        #expect(f.coordinator.clickTask(event))
        try await eventually { !f.document.dirty && !f.coordinator.pending && f.view.string.contains("☑") }
        let saved = try String(contentsOf: f.document.io.url, encoding: .utf8)
        #expect(saved == source.replacingOccurrences(of: "[ ]", with: "[x]"))
        #expect(zip(source.utf16, saved.utf16).filter { $0 != $1 }.count == 1)
        await f.close()
    }

    @Test func diagramHeightFallbackAndContinuousQuoteBars() async throws {
        let source = "> Line one\n> Line two\n> Line three\n\n> [!note] Title\n> Line two\n> Line three\n\n" + EditingTests.source + "\n\n~~~mermaid\nsequenceDiagram\nA->>B: Hello\n~~~\n"
        let f = try Fixture(source); try await f.start()
        #expect(f.view.diagramStorage?.diagrams.isEmpty == true)
        #expect(f.view.diagramViews.count == 1)
        let manager = try #require(f.view.textLayoutManager), content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        var quotes: [NoteLayoutFragment] = [], diagrams = 0, failures = 0
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            if let fragment = fragment as? NoteLayoutFragment {
                if fragment.sourceMask & 64 != 0 { quotes.append(fragment) }
                if let size = fragment.replacement { #expect(size.height > 20); diagrams += 1 }
                if fragment.reason != nil { failures += 1 }
            }
            return true
        }
        #expect(quotes.count == 6)
        #expect(quotes.map(\.quoteHead) == [true, false, false, true, false, false])
        #expect(quotes.map(\.quoteTail) == [false, false, true, false, false, true])
        #expect(diagrams == 1 && failures == 1)
        #expect(f.view.string.contains("sequenceDiagram"))
        await f.close()
    }

    @Test func externalMiddleChangeRebuildsOneBlockAndKeepsAnchor() async throws {
        let source = (0..<600).map { "Paragraph \($0) **bold** 中文😀.\n\n" }.joined()
        let f = try Fixture(source); try await f.start()
        f.coordinator.reveal(source: source.utf16.count / 2)
        f.view.layoutSubtreeIfNeeded()
        let before = try #require(f.coordinator.visibleAnchor())
        let changed = source.replacingOccurrences(of: "Paragraph 300", with: "Modified paragraph 300")
        try Data(changed.utf8).write(to: f.document.io.url)
        f.document.refresh()
        try await eventually { f.document.text == changed && !f.coordinator.pending && f.coordinator.model?.revision == f.document.revision }
        let after = try #require(f.coordinator.visibleAnchor())
        #expect(f.coordinator.model?.rebuilt == 1)
        #expect(before.id == after.id)
        #expect(abs(before.offset - after.offset) <= 1)
        await f.close()
    }

    @Test func refreshRequestsAreCoalescedUntilTheActiveRequestFinishes() async throws {
        let f = try Fixture("# Heading\n\nBody\n")
        try await f.start()

        var count = f.coordinator.submittedRequests
        f.coordinator.refresh()
        f.coordinator.refresh()
        #expect(f.coordinator.submittedRequests == count + 1)
        try await f.ready()

        count = f.coordinator.submittedRequests
        f.coordinator.refresh()
        f.view.setFrameSize(NSSize(width: f.view.bounds.width + 20, height: f.view.bounds.height))
        f.coordinator.refresh()
        f.coordinator.refresh()
        #expect(f.coordinator.submittedRequests == count + 1)
        try await f.ready()
        #expect(f.coordinator.submittedRequests == count + 2)
        await f.close()
    }
}
