import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite(.serialized) @MainActor
struct InlineMathTests {
    @Test func headingErrorWideSelectionAndResizeReuse() async throws {
        let wide = Array(repeating: "x", count: 100).joined(separator: "+")
        let source = "# Title $x^2$\n\nBefore $x^2$ after $\\unknowncommand$ and $   $.\n\nBefore $\(wide)$ after.\n"
        let fixture = try ReadingTests.Fixture(source)
        let service = MathTests.service()
        try await fixture.start(mathService: service)
        let formulas = fixture.view.inlineMath.values.sorted { $0.item.range.location < $1.item.range.location }
        #expect(formulas.count == 4)
        #expect(abs(formulas[0].naturalSize.width / formulas[1].naturalSize.width - 1.6) < 0.001)
        #expect(formulas[2].item.prepared.math == nil && formulas[2].item.prepared.reason != nil)
        #expect(formulas[2].item.prepared.source == "$\\unknowncommand$")
        #expect(fixture.view.string.contains("$   $"))
        let scrollable = try #require(formulas.last)
        #expect(scrollable.wide && scrollable.bounds.width <= fixture.view.bounds.width - 60)
        let manager = try #require(fixture.view.textLayoutManager)
        let content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        let location = try #require(content.location(content.documentRange.location, offsetBy: scrollable.item.range.location))
        let fragment = try #require(manager.textLayoutFragment(for: location))
        let local = content.offset(from: fragment.rangeInElement.location, to: location)
        let line = try #require(fragment.textLineFragments.first { NSLocationInRange(local, $0.characterRange) })
        #expect(line.characterRange.length <= 2)
        let plain = formulas[1]
        fixture.view.setSelectedRange(plain.item.range)
        fixture.view.copy(nil)
        #expect(NSPasteboard.general.string(forType: .string) == "$x^2$")
        fixture.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == "$x^2$")
        let layouts = await service.layoutCount
        fixture.view.setFrameSize(NSSize(width: fixture.view.frame.width - 100, height: fixture.view.frame.height))
        fixture.coordinator.refresh(force: true)
        try await fixture.ready()
        #expect(await service.layoutCount == layouts)
        await fixture.close()
    }

    @Test func copyChangesOnlyFormulaAttachmentsAndTableUsesPreparedScenes() async throws {
        let source = "A $x^2$ B\n\n$$y=2$$\n\n![missing](missing.png)\n\n| Formula |\n|---|\n| $\\frac{a}{b}$ |\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: MathTests.service())
        let table = try #require(fixture.view.mediaBlocks.first { !$0.prepared.block.columns.isEmpty })
        #expect(table.prepared.math.flatMap { $0 }.count == 1)
        #expect(table.prepared.math.flatMap { $0 }.first?.math != nil)
        fixture.view.setSelectedRange(NSRange(location: 0, length: fixture.view.sourceUTF16Length))
        fixture.view.copy(nil)
        let copied = try #require(NSPasteboard.general.string(forType: .string))
        #expect(copied.contains("A $x^2$ B") && copied.contains("$$y=2$$"))
        #expect(copied.filter { $0 == "\u{fffc}" }.count == 2)
        fixture.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == source)
        await fixture.close()
    }

    @Test func nativeBaselineLineHeightCopyAndAccessibility() async throws {
        let source = "Normal abc text.\nSame $x^2$ text.\nTall $\\frac{a}{b}$ text.\nNormal abc text.\n\nBefore $$y=2$$ after.\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: MathTests.service())
        let view = fixture.view
        #expect(view.inlineMath.count == 2)
        #expect(view.mathViews.count == 1)
        let manager = try #require(view.textLayoutManager)
        let content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        view.needsLayout = true
        view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
        var lines: [NSTextLineFragment] = []
        var attachments = 0
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            lines.append(contentsOf: fragment.textLineFragments.filter { $0.characterRange.length > 1 })
            for attachment in view.inlineMath.values {
                let position = content.location(content.documentRange.location, offsetBy: attachment.item.range.location)!
                guard fragment.rangeInElement.contains(position) else { continue }
                attachments += 1
                let formulaView = view.inlineMathViews.first { $0.attachment === attachment }
                #expect(formulaView?.accessibilityLabel() == L10n.string("Formula: %@", attachment.item.prepared.block.latex))
                let local = content.offset(from: fragment.rangeInElement.location, to: position)
                if let line = fragment.textLineFragments.first(where: { NSLocationInRange(local, $0.characterRange) }) {
                    let baseline = fragment.layoutFragmentFrame.minY + line.typographicBounds.minY + line.glyphOrigin.y + view.textContainerInset.height
                    let formulaBaseline = (formulaView?.frame.minY ?? -.infinity) + attachment.ascent
                    #expect(abs(formulaBaseline - baseline) <= 0.5, "baseline: \(formulaBaseline), text: \(baseline)")
                }
            }
            return true
        }
        #expect(attachments == 2)
        #expect(lines.count >= 4)
        if lines.count >= 4 {
            #expect(abs(lines[0].typographicBounds.height - lines[1].typographicBounds.height) <= 0.5)
            #expect(lines[2].typographicBounds.height > lines[1].typographicBounds.height)
            #expect(abs(lines[0].typographicBounds.height - lines[3].typographicBounds.height) <= 0.5)
        }
        view.setSelectedRange(NSRange(location: 0, length: view.sourceUTF16Length))
        view.copy(nil)
        #expect(NSPasteboard.general.string(forType: .string) == source.replacingOccurrences(of: "Before $$y=2$$ after.", with: "Before \n$$y=2$$\n after."))
        fixture.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == source)
        #expect(view.textLayoutManager === manager)
        await fixture.close()
    }

    @Test func scrollingMountsVisibleFormulaAndSelectionIsAtomic() async throws {
        let source = (0..<100).map { "Paragraph \($0) before $x^2$ after.\n\n" }.joined()
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: MathTests.service())
        let view = fixture.view
        let formulas = view.inlineMath.values.sorted { $0.item.range.location < $1.item.range.location }
        let target = try #require(formulas.dropFirst(70).first)
        fixture.coordinator.reveal(source: Int(target.item.prepared.block.sourceStart))
        view.needsLayout = true
        view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
        let mounted = try #require(view.inlineMathViews.first { $0.attachment === target })
        #expect(mounted.superview === view && mounted.frame.intersects(view.visibleRect))
        #expect(view.inlineMathViews.count < formulas.count)
        // 一个 U+FFFC 只能在前后落光标；原生扩选跨过它时包含整个公式。
        view.setSelectedRange(NSRange(location: target.item.range.location, length: 0))
        view.moveRightAndModifySelection(nil)
        #expect(view.selectedRange() == target.item.range)
        view.copy(nil)
        #expect(NSPasteboard.general.string(forType: .string) == "$x^2$")
        await fixture.close()
    }
}
