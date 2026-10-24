import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite(.serialized) @MainActor
struct StyledSourceTests {
    private func ready(_ fixture: EditingTests.Fixture) async throws {
        try await fixture.ready()
        for _ in 0..<500 {
            if !fixture.coordinator.styling.pending { return }
            try await Task.sleep(for: .milliseconds(5))
        }
        Issue.record("样式更新超时")
    }
    @Test func everyArrowStepKeepsAllLineFragmentsAndStylesOutOfUndo() async throws {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("fixtures/vault-sample/样式化源码.md")
        let source = try String(contentsOf: url, encoding: .utf8)
        let f = try EditingTests.Fixture(source)
        try await ready(f)
        let view = f.view
        #expect(!f.document.dirty)
        #expect(view.undoManager?.canUndo != true)
        let title = (source as NSString).range(of: "# 样式化源码")
        #expect((f.coordinator.styling.style(f.coordinator.styling.mask(at: title.location))[.font] as? NSFont)?.pointSize == 24)
        func positions() -> [CGRect] {
            guard let manager = view.textLayoutManager, let content = manager.textContentManager else { return [] }
            manager.ensureLayout(for: content.documentRange)
            var rects: [CGRect] = []
            manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout, .ensuresExtraLineFragment]) { fragment in
                for line in fragment.textLineFragments { rects.append(line.typographicBounds.offsetBy(dx: fragment.layoutFragmentFrame.minX, dy: fragment.layoutFragmentFrame.minY)) }
                return true
            }
            return rects
        }
        let original = positions(), applications = f.coordinator.styling.applications
        view.setSelectedRange(NSRange(location: 0, length: 0))
        var steps = 0
        while view.selectedRange().location < view.sourceUTF16Length && steps < 500 {
            let before = view.selectedRange()
            view.moveDown(nil); view.layoutSubtreeIfNeeded()
            #expect(positions() == original)
            steps += 1
            if view.selectedRange() == before { view.moveToEndOfDocument(nil) }
        }
        while view.selectedRange().location > 0 && steps < 1000 {
            let before = view.selectedRange()
            view.moveUp(nil); view.layoutSubtreeIfNeeded()
            #expect(positions() == original)
            steps += 1
            if view.selectedRange() == before { view.moveToBeginningOfDocument(nil) }
        }
        #expect(f.coordinator.styling.applications == applications)
        #expect(!f.document.dirty)
        #expect(view.undoManager?.canUndo != true)
        print("光标稳定：steps=\(steps) fragments=\(original.count)")
        await f.close()
    }
    @Test func listContinuationExitIndentAndTaskCommandsEachUndoOnce() async throws {
        for (source, expected) in [("- 内容", "- 内容\n- "), ("  9) 内容", "  9) 内容\n  10) "),
            ("- [x] 完成", "- [x] 完成\n- [ ] "), ("    - ", "")] {
            let f = try EditingTests.Fixture(source); try await ready(f)
            f.view.setSelectedRange(NSRange(location: (source as NSString).length, length: 0))
            f.view.insertNewline(nil)
            #expect(f.view.string == expected)
            f.view.undoManager?.undo(); #expect(f.view.string == source)
            #expect(f.view.undoManager?.canUndo != true)
            f.view.undoManager?.redo(); #expect(f.view.string == expected)
            await f.close()
        }
        let f = try EditingTests.Fixture("- [ ] 任务\n- 第二项\n"); try await ready(f)
        let source = f.view.string
        f.view.insertTab(nil); #expect(f.view.string.hasPrefix("    - "))
        f.view.undoManager?.undo(); #expect(f.view.string == source); #expect(f.view.undoManager?.canUndo != true)
        f.view.setSelectedRange(NSRange(location: 0, length: 0))
        f.view.insertTab(nil); f.view.undoManager?.removeAllActions()
        f.view.insertBacktab(nil); #expect(f.view.string == source)
        f.view.undoManager?.undo(); #expect(f.view.string.hasPrefix("    - ")); #expect(f.view.undoManager?.canUndo != true)
        f.view.undoManager?.redo(); f.view.undoManager?.removeAllActions()
        f.view.setSelectedRange(NSRange(location: 0, length: 0))
        let event = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command,
            timestamp: 0, windowNumber: f.window.windowNumber, context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
        #expect(f.view.performKeyEquivalent(with: event)); #expect(f.view.string.hasPrefix("- [x]")); #expect(f.document.dirty)
        f.view.undoManager?.undo(); #expect(f.view.string == source); #expect(f.view.undoManager?.canUndo != true)
        f.view.undoManager?.removeAllActions()
        let range = NSRange(location: 2, length: 3)
        let rect = f.view.firstRect(forCharacterRange: range, actualRange: nil)
        let point = f.window.convertPoint(fromScreen: CGPoint(x: rect.midX, y: rect.midY))
        let mouse = try #require(NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [], timestamp: 0,
            windowNumber: f.window.windowNumber, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        f.view.mouseDown(with: mouse); #expect(f.view.string.hasPrefix("- [x]"))
        for _ in 0..<300 {
            if !f.document.dirty { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        #expect(!f.document.dirty)
        #expect(try String(contentsOf: f.document.io.url, encoding: .utf8) == f.view.string)
        f.view.undoManager?.undo(); #expect(f.view.string == source); #expect(f.view.undoManager?.canUndo != true)
        #expect(try await f.text() == f.view.string)
        await f.close()
    }
    @Test func richPasteIsPlainAndCompositionAttributesRemainUntouched() async throws {
        let f = try EditingTests.Fixture("# 标题\n正文 **强调**\n"); try await ready(f)
        let pboard = NSPasteboard.withUniqueName()
        let rich = NSAttributedString(string: "粘贴", attributes: [.font: NSFont.systemFont(ofSize: 60), .foregroundColor: NSColor.red])
        pboard.setString("粘贴", forType: .string)
        pboard.setData(try rich.data(from: NSRange(location: 0, length: rich.length), documentAttributes: [.documentType: NSAttributedString.DocumentType.rtf]), forType: .rtf)
        f.view.setSelectedRange(NSRange(location: f.view.sourceUTF16Length, length: 0))
        #expect(f.view.readSelection(from: pboard, type: .rtf)); try await ready(f)
        #expect((f.coordinator.styling.style(f.coordinator.styling.mask(at: f.view.sourceUTF16Length - 1))[.font] as? NSFont)?.pointSize == 15)
        f.view.setMarkedText("zhong", selectedRange: NSRange(location: 5, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        let range = f.view.markedRange()
        let before = try #require(f.view.textStorage?.attributedSubstring(from: range))
        let applications = f.coordinator.styling.applications
        try await Task.sleep(for: .milliseconds(100))
        #expect(f.view.textStorage?.attributedSubstring(from: range).isEqual(to: before) == true)
        #expect(f.coordinator.styling.paragraph(before, range: range, in: f.view).isEqual(to: before))
        #expect(f.coordinator.styling.applications == applications)
        f.view.insertText("中文", replacementRange: NSRange(location: NSNotFound, length: 0)); try await ready(f)
        #expect(try await f.text() == f.view.string)
        await f.close(); pboard.releaseGlobally()
    }
    @Test func appearanceFontAndHeadingRotorPreserveTextAndUndo() async throws {
        let source = "# 一级\n正文\n## 二级\n> 引用\n\n```text\n代码\n```\n"
        let f = try EditingTests.Fixture(source); try await ready(f)
        f.view.appearance = NSAppearance(named: .darkAqua)
        f.view.contrastForInspection = true
        f.coordinator.styling.schedule(view: f.view, document: f.document, size: 18, force: true)
        try await ready(f)
        #expect(f.view.increasedContrast)
        #expect((f.coordinator.styling.style(1)[.foregroundColor] as? NSColor)?.isEqual(NSColor.secondaryLabelColor) == true)
        #expect(abs(((f.coordinator.styling.style(1 << 24)[.font] as? NSFont)?.pointSize ?? 0) - 28.8) < 0.001)
        #expect(f.view.string == source); #expect(!f.document.dirty)
        #expect(f.view.undoManager?.canUndo != true)
        let rotor = try #require(f.view.accessibilityCustomRotors().first)
        let parameters = NSAccessibilityCustomRotor.SearchParameters()
        parameters.searchDirection = .next; parameters.filterString = ""
        let first = try #require(f.view.rotor(rotor, resultFor: parameters))
        #expect(first.targetRange.location == 0)
        #expect(first.customLabel == L10n.string("Heading level %1$@, %2$@", "1", "# 一级"))
        parameters.currentItem = first
        let second = try #require(f.view.rotor(rotor, resultFor: parameters))
        #expect(second.targetRange.location == (source as NSString).range(of: "##").location)
        #expect(second.customLabel == L10n.string("Heading level %1$@, %2$@", "2", "## 二级"))
        parameters.currentItem = nil; parameters.searchDirection = .previous
        #expect(f.view.rotor(rotor, resultFor: parameters)?.targetRange == second.targetRange)
        let manager = try #require(f.view.textLayoutManager)
        let content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        var decorated = 0
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            if let fragment = fragment as? NoteLayoutFragment, fragment.sourceMask & (32 | 64) != 0 {
                #expect(fragment.renderingSurfaceBounds.minX <= -12)
                #expect(fragment.renderingSurfaceBounds.maxX >= fragment.noticeWidth + 5)
                decorated += 1
            }
            return true
        }
        #expect(decorated >= 4)
        await f.close()
    }
    @Test func systemFindBarExpandsDiagramAndReplaceAllUndoesOnce() async throws {
        let source = "Needle\n\n```mermaid\nflowchart TB\nA[Needle] --> B[Leaf]\n```\n\nNeedle\n"
        let f = try EditingTests.Fixture(source); try await ready(f)
        func action(_ action: NSTextFinder.Action) {
            let item = NSMenuItem(); item.tag = action.rawValue; f.view.performFindPanelAction(item)
        }
        f.view.setSelectedRange(NSRange(location: 0, length: 6)); action(.setSearchString)
        action(.showReplaceInterface)
        #expect(f.view.enclosingScrollView?.isFindBarVisible == true)
        action(.nextMatch)
        #expect(f.view.selectedRange() == (source as NSString).range(of: "Needle", options: [], range: NSRange(location: 7, length: (source as NSString).length - 7)))
        #expect(f.view.expanded == [1])
        #expect(f.view.diagramStorage?.diagrams.isEmpty == true)
        func fields(_ view: NSView) -> [NSTextField] { (view as? NSTextField).map { [$0] } ?? view.subviews.flatMap { fields($0) } }
        let bar = try #require(f.view.enclosingScrollView?.findBarView)
        let inputs = fields(bar).filter { $0.isEditable }
        #expect(inputs.count == 2)
        let replacement = try #require(inputs.last)
        replacement.stringValue = "Changed"
        if let action = replacement.action { NSApp.sendAction(action, to: replacement.target, from: replacement) }
        f.view.undoManager?.removeAllActions()
        action(.replaceAll)
        #expect(f.view.string == source.replacingOccurrences(of: "Needle", with: "Changed"))
        f.view.undoManager?.undo()
        #expect(f.view.string == source)
        #expect(f.view.undoManager?.canUndo != true)
        #expect(try await f.text() == f.view.string)
        await f.close()
    }
    @Test func deletingToEmptyClearsStylesAndUndoRestoresHeading() async throws {
        let source = "# 标题\n正文\n"
        let f = try EditingTests.Fixture(source); try await ready(f)
        f.view.insertText("", replacementRange: NSRange(location: 0, length: f.view.sourceUTF16Length))
        try await ready(f)
        #expect(f.coordinator.styling.runs.isEmpty)
        #expect((f.view.typingAttributes[.font] as? NSFont)?.pointSize == 15)
        f.view.undoManager?.undo(); try await ready(f)
        #expect(f.view.string == source)
        #expect(f.coordinator.styling.mask(at: 2) >> 24 == 1)
        #expect(f.view.undoManager?.canUndo != true)
        await f.close()
    }
}
