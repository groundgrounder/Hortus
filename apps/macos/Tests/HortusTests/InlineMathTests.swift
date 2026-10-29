import AppKit
import CoreText
import HortusCore
import Testing
@testable import Hortus

@MainActor private func eventually(_ predicate: () -> Bool) async throws {
    for _ in 0..<500 {
        if predicate() { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    Issue.record("等待公式刷新超时")
}

@Suite(.serialized) @MainActor
struct InlineMathTests {
    @Test func accessibilityTextFramesAndUniqueObjects() async throws {
        let f = try ReadingTests.Fixture("Before $x$ after $\\unknowncommand$.\n\n" + String(repeating: "Paragraph.\n\n", count: 90))
        try await f.start(mathService: MathTests.service())
        let range = NSRange(location: 0, length: f.view.sourceUTF16Length)
        let text = try #require(f.view.accessibilityString(for: range))
        let error = try #require(f.view.inlineMath.values.first { $0.item.prepared.reason != nil })
        #expect(text == "Before " + L10n.string("Formula: %@", "x") + " after " + L10n.string("Formula: %@", "\\unknowncommand") + " — " + error.item.prepared.reason! + ".\n\n" + String(repeating: "Paragraph.\n\n", count: 90))
        #expect(f.view.accessibilityAttributedString(for: range)?.string == text)
        let attributed = try #require(f.view.accessibilityAttributedString(for: range))
        attributed.enumerateAttribute(.attachment, in: NSRange(location: 0, length: attributed.length)) { value, _, _ in
            #expect(value == nil)
        }
        let elements = (f.view.accessibilityChildren() ?? []).compactMap { $0 as? NSAccessibilityElement }
        #expect(elements.count == 2)
        #expect(f.view.inlineMathViews.allSatisfy { !$0.isAccessibilityElement() })
        let first = try #require(elements.first)
        let before = first.accessibilityFrame()
        f.view.enclosingScrollView?.contentView.scroll(to: CGPoint(x: 0, y: 45))
        #expect(abs(first.accessibilityFrame().minY - before.minY - 45) < 0.5)
        let window = f.editing.window, origin = window.frame.origin
        window.setFrameOrigin(CGPoint(x: origin.x + 20, y: origin.y + 30))
        #expect(abs(first.accessibilityFrame().minX - before.minX - 20) < 0.5)
        window.setFrameOrigin(origin)
        await f.close()
    }

    @Test func singleClipboardWritePreservesOtherTypesAndDetachClearsTooltips() async throws {
        let f = try ReadingTests.Fixture("A $\\unknowncommand$.\n")
        try await f.start(mathService: MathTests.service())
        f.view.setSelectedRange(NSRange(location: 0, length: f.view.sourceUTF16Length))
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setData(Data([1, 2, 3]), forType: .png)
        #expect(f.view.writeSelection(to: board, type: .string))
        #expect(board.data(forType: .png) == Data([1, 2, 3]))
        #expect(!f.view.formulaTooltips.isEmpty)
        f.coordinator.detach()
        #expect(f.view.formulaTooltips.isEmpty)
        await f.close()
    }

    @Test func pageOverflowUsesDocumentWidthWhenScrollerIsDisabled() {
        let scroll = NSScrollView(frame: CGRect(x: 0, y: 0, width: 400, height: 200))
        let document = NSView(frame: CGRect(x: 0, y: 0, width: 600, height: 200))
        scroll.hasHorizontalScroller = false; scroll.documentView = document
        #expect(MathInspection.pageHasHorizontalOverflow(document))
        document.setFrameSize(CGSize(width: scroll.contentView.bounds.width, height: 200))
        #expect(!MathInspection.pageHasHorizontalOverflow(document))
    }
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
        #expect(NSPasteboard.general.string(forType: .string) == source.replacingOccurrences(of: "Before $$y=2$$ after.", with: "Before\n$$y=2$$\nafter."))
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

    @Test func readingClipboardTypesAndContainerSources() async throws {
        for source in ["Plain **text**.\n", "> $$\n> \\frac{1}{2}\n> $$\n\n> [!note]\n> $a\n> +b$\n"] {
            let f = try ReadingTests.Fixture(source)
            try await f.start(mathService: MathTests.service())
            f.view.setSelectedRange(NSRange(location: 0, length: f.view.sourceUTF16Length))
            f.view.copy(nil)
            #expect(Set(NSPasteboard.general.types ?? []) == Set([.string, NSPasteboard.PasteboardType("NSStringPboardType")]))
            let copied = try #require(NSPasteboard.general.string(forType: .string))
            #expect(copied == (source.contains("frac") ? "$$\n\\frac{1}{2}\n$$\n\nnote ·\n$a\n+b$\n" : "Plain text.\n"))
            #expect(f.view.writablePasteboardTypes == [.string])
            let board = NSPasteboard.withUniqueName()
            defer { board.releaseGlobally() }
            #expect(f.view.writeSelection(to: board, types: [.rtf, .string]))
            #expect(Set(board.types ?? []) == Set([.string, NSPasteboard.PasteboardType("NSStringPboardType")]))
            #expect(board.string(forType: .string) == copied)
            #expect(!f.view.writeSelection(to: board, type: .rtf))
            #expect(f.view.writeSelection(to: board, type: .string))
            #expect(Set(board.types ?? []) == Set([.string, NSPasteboard.PasteboardType("NSStringPboardType")]))
            #expect(board.string(forType: .string) == copied)
            await f.close()
        }
    }

    @Test func shiftedRepeatedAttachmentsAndDeletionBeforeMedia() async throws {
        let source = "First $a$.\n\nRepeated $x$ and $x$.\n\nRepeated $x$ and $x$.\n\n![missing](missing.png)\n\n```mermaid\nflowchart LR\nA --> B\n```\n"
        let f = try ReadingTests.Fixture(source)
        try await f.start(mathService: MathTests.service())
        let original = f.view.inlineMath.values.sorted { $0.item.range.location < $1.item.range.location }
        let shifted = "Inserted text " + source
        try Data(shifted.utf8).write(to: f.document.io.url); f.document.refresh()
        try await eventually { f.document.text == shifted && !f.coordinator.pending && f.coordinator.model?.revision == f.document.revision }
        let moved = f.view.inlineMath.values.sorted { $0.item.range.location < $1.item.range.location }
        #expect(moved.count == original.count)
        for (a, b) in zip(original.dropFirst(), moved.dropFirst()) { #expect(a === b) }
        #expect(NSMaxRange(f.view.inlineMathInvalidation ?? NSRange(location: 0, length: 0)) <= 25)
        #expect(moved.allSatisfy { f.view.inlineMath[$0.item.range.location] === $0 })
        let deleted = shifted.replacingOccurrences(of: "First $a$.", with: "First.")
        try Data(deleted.utf8).write(to: f.document.io.url); f.document.refresh()
        try await eventually { f.document.text == deleted && !f.coordinator.pending && f.coordinator.model?.revision == f.document.revision }
        let manager = try #require(f.view.textLayoutManager), content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            let offset = content.offset(from: content.documentRange.location, to: fragment.rangeInElement.location)
            if let paragraph = fragment.textElement as? NSTextParagraph {
                paragraph.attributedString.enumerateAttribute(.attachment, in: NSRange(location: 0, length: paragraph.attributedString.length)) { value, range, _ in
                    if let attachment = value as? InlineMathAttachment {
                        #expect(f.view.inlineMath[offset + range.location] === attachment)
                        #expect(attachment.item.range.location == offset + range.location)
                    }
                }
            }
            return true
        }
        #expect(f.view.inlineMath.count == 4 && f.view.mediaViews.count == 1 && f.view.diagramViews.count == 1)
        await f.close()
    }

    @Test func accessibilityIncludesOffscreenFormulasInDocumentOrder() async throws {
        let source = "First $a$.\n\n![missing](missing.png)\n\n$$b$$\n\n```mermaid\nflowchart LR\nA --> B\n```\n\n" + (0..<80).map { "Paragraph \($0) $x_\($0)$.\n\n" }.joined() + "$\\unknowncommand$\n"
        let f = try ReadingTests.Fixture(source)
        try await f.start(mathService: MathTests.service())
        let labels = (f.view.accessibilityChildren() ?? []).compactMap {
            ($0 as? NSAccessibilityElement)?.accessibilityLabel() ?? ($0 as? NSView)?.accessibilityLabel()
        }.filter { $0.hasPrefix(L10n.string("Formula: %@", "")) }
        let expected = ["a", "b"] + (0..<80).map { "x_\($0)" } + ["\\unknowncommand"]
        #expect(labels == expected.map { L10n.string("Formula: %@", $0) })
        let ordered = (f.view.accessibilityChildren() ?? []).compactMap { child -> String? in
            if let element = child as? NSAccessibilityElement { return element.accessibilityLabel() }
            if child is MediaView { return "media" }
            if child is DiagramView { return "diagram" }
            return nil
        }
        #expect(Array(ordered.prefix(4)) == [L10n.string("Formula: %@", "a"), "media", L10n.string("Formula: %@", "b"), "diagram"])
        let errors = (f.view.accessibilityChildren() ?? []).compactMap {
            ($0 as? NSAccessibilityElement)?.accessibilityHelp() ?? ($0 as? NSView)?.accessibilityHelp()
        }
        #expect(errors.contains { $0.contains("unknowncommand") })
        let elements = (f.view.accessibilityChildren() ?? []).compactMap { $0 as? NSAccessibilityElement }
        let shifted = "Inserted text " + source
        let manager = try #require(f.view.textLayoutManager), content = try #require(manager.textContentManager)
        let lastItem = try #require(f.view.inlineMath.values.max { $0.item.range.location < $1.item.range.location })
        let start = try #require(content.location(content.documentRange.location, offsetBy: lastItem.item.range.location))
        let end = try #require(content.location(start, offsetBy: 1))
        manager.invalidateLayout(for: try #require(NSTextRange(location: start, end: end)))
        #expect(elements.last?.accessibilityFrame().isEmpty == true)
        let repeated = (f.view.accessibilityChildren() ?? []).compactMap { $0 as? NSAccessibilityElement }
        #expect(zip(elements, repeated).allSatisfy { $0 === $1 })
        try Data(shifted.utf8).write(to: f.document.io.url); f.document.refresh()
        try await eventually { f.document.text == shifted && !f.coordinator.pending && f.coordinator.model?.revision == f.document.revision }
        let refreshed = (f.view.accessibilityChildren() ?? []).compactMap { $0 as? NSAccessibilityElement }
        #expect(refreshed.count == elements.count)
        for (before, after) in zip(elements.dropFirst(), refreshed.dropFirst()) { #expect(before === after) }
        await f.close()
    }

    @Test func tableFormulaSizeFallbackWidthAndErrorAccessibility() async throws {
        let source = "Body $x$.\n\n| Formula | Other |\n|---|---|\n| # $x$ | $a\\|b$ |\n| $\\unknowncommand{abcdefghijklmnop}$ | end |\n"
        let f = try ReadingTests.Fixture(source)
        try await f.start(mathService: MathTests.service())
        let body = try #require(f.view.inlineMath.values.first?.item.prepared.math)
        let table = try #require(f.view.mediaBlocks.first)
        let formulas = table.prepared.math.flatMap { $0 }
        #expect(formulas.contains { $0.block.latex == "a|b" })
        let heading = try #require(formulas.first { $0.block.latex == "x" }?.math)
        #expect(abs(body.size.width - heading.size.width) < 0.001)
        let error = try #require(formulas.first { $0.reason != nil })
        let line = try #require(error.fallbackLines.first)
        let width = CTLineGetTypographicBounds(line, nil, nil, nil)
        let runs = CTLineGetGlyphRuns(line) as! [CTRun]
        let font = (CTRunGetAttributes(try #require(runs.first)) as NSDictionary)[kCTFontAttributeName] as! CTFont
        #expect(CTFontGetSymbolicTraits(font).contains(.traitMonoSpace))
        #expect(width + 20 <= table.prepared.block.columns[0] + 0.01)
        let help = f.view.mediaViews.flatMap { $0.subviews }.compactMap { ($0 as? NSScrollView)?.documentView?.accessibilityHelp() }
        #expect(help.contains { $0.contains("unknowncommand") })
        await f.close()
    }


    @Test func renderedInkBaselineAndExactSizesAcrossBodySizes() async throws {
        for size in [12.0, 15, 18, 24] {
            let f = try ReadingTests.Fixture("x $x$\nNormal abc.\nSame $x^2$.\nTall $\\frac{a}{b}$.\nNormal abc.\n")
            f.view.appearance = NSAppearance(named: .aqua)
            try await f.start(mathService: MathTests.service(), size: size)
            let formula = try #require(f.view.inlineMathViews.min { $0.attachment.item.range.location < $1.attachment.item.range.location })
            let screen = f.view.firstRect(forCharacterRange: NSRange(location: 0, length: 1), actualRange: nil)
            let textRect = f.view.convert(f.editing.window.convertFromScreen(screen), from: nil)
            func inkBottom(_ rect: CGRect) throws -> Double {
                let bitmap = try #require(f.view.bitmapImageRepForCachingDisplay(in: rect))
                f.view.cacheDisplay(in: rect, to: bitmap)
                var last: Int?
                for y in 0..<bitmap.pixelsHigh {
                    for x in 0..<bitmap.pixelsWide {
                        if let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB),
                           color.alphaComponent > 0.5, max(color.redComponent, color.greenComponent, color.blueComponent) < 0.5 {
                            last = y
                        }
                    }
                }
                return rect.minY + Double(try #require(last) + 1) * rect.height / Double(bitmap.pixelsHigh)
            }
            let textBottom = try inkBottom(textRect), formulaBottom = try inkBottom(formula.frame)
            #expect(abs(textBottom - formulaBottom) <= 0.5, "size=\(size), ink text=\(textBottom), math=\(formulaBottom)")
            for view in f.view.inlineMathViews {
                let next = f.view.firstRect(forCharacterRange: NSRange(location: NSMaxRange(view.attachment.item.range), length: 1), actualRange: nil)
                let attachment = f.view.firstRect(forCharacterRange: view.attachment.item.range, actualRange: nil)
                #expect(abs(next.minX - attachment.minX - view.attachment.naturalSize.width) < 0.001)
                #expect(abs(view.frame.height - view.attachment.bounds.height) < 0.001)
            }
            let manager = try #require(f.view.textLayoutManager), content = try #require(manager.textContentManager)
            manager.ensureLayout(for: content.documentRange)
            var lines: [NSTextLineFragment] = []
            manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
                lines.append(contentsOf: fragment.textLineFragments.filter { $0.characterRange.length > 1 }); return true
            }
            #expect(lines.count >= 5)
            if lines.count >= 5 {
                #expect(abs(lines[1].typographicBounds.height - lines[2].typographicBounds.height) <= 0.5)
                #expect(lines[3].typographicBounds.height > lines[2].typographicBounds.height)
                #expect(abs(lines[1].typographicBounds.height - lines[4].typographicBounds.height) <= 0.5)
            }
            await f.close()
        }
    }

    @Test func existingInlineFormulaFollowsThemeAndLegacyScrollbar() async throws {
        let wide = Array(repeating: "x", count: 100).joined(separator: "+")
        let f = try ReadingTests.Fixture("$x$ and $\(wide)$\n\n$$\(wide)$$\n\n| $\(wide)$ |\n|---|\n")
        f.view.enclosingScrollView?.scrollerStyle = .overlay
        f.view.appearance = NSAppearance(named: .aqua)
        try await f.start(mathService: MathTests.service())
        let before = try #require(f.view.inlineMath.values.min { $0.item.range.location < $1.item.range.location })
        func pixels() throws -> [Double] {
            let mounted = try #require(f.view.inlineMathViews.first { $0.attachment.item.identity == before.item.identity })
            let bitmap = try #require(mounted.bitmapImageRepForCachingDisplay(in: mounted.bounds))
            mounted.cacheDisplay(in: mounted.bounds, to: bitmap)
            return (0..<bitmap.pixelsHigh).flatMap { y in (0..<bitmap.pixelsWide).compactMap { x -> Double? in
                let c = bitmap.colorAt(x: x, y: y)!.usingColorSpace(.sRGB)!
                guard c.alphaComponent > 0.8 else { return nil }
                return Double(c.redComponent + c.greenComponent + c.blueComponent) / 3
            } }
        }
        let light = try pixels()
        f.view.appearance = NSAppearance(named: .darkAqua); f.coordinator.refresh(force: true)
        try await f.ready()
        let dark = try pixels()
        #expect(!light.isEmpty && !dark.isEmpty)
        #expect(light.reduce(0, +) / Double(light.count) < 0.5)
        #expect(dark.reduce(0, +) / Double(dark.count) > 0.5)
        let attachment = try #require(f.view.inlineMath.values.first { $0.wide })
        let oldBlock = try #require(f.view.mathViews.first)
        let width = f.view.bounds.width
        f.view.enclosingScrollView?.scrollerStyle = .legacy
        f.view.onWidthOrThemeChange?()
        try await f.ready()
        #expect(f.view.bounds.width == width)
        let thickness = NSScroller.scrollerWidth(for: .regular, scrollerStyle: .legacy)
        let legacy = try #require(f.view.inlineMath.values.first { $0.wide })
        #expect(legacy !== attachment)
        #expect(legacy.bounds.height >= legacy.naturalSize.height + thickness)
        let block = try #require(f.view.mathViews.first)
        #expect(block !== oldBlock)
        #expect(block.preferredSize.height >= block.item.prepared.math!.size.height + 2 * MathTypography.padding + thickness)
        let table = try #require(f.view.mediaBlocks.first)
        #expect(abs(legacy.naturalSize.width - (table.prepared.block.columns[0] - 20)) < 0.001)
        await f.close()
    }


    @Test func selectionBackgroundFollowsNativeTextAndHostOwnsErrorTooltip() async throws {
        let f = try ReadingTests.Fixture("A $\\quad x$ Z $\\unknowncommand$.\n")
        try await f.start(mathService: MathTests.service())
        let formula = try #require(f.view.inlineMathViews.min { $0.attachment.item.range.location < $1.attachment.item.range.location })
        let field = NSTextField(frame: CGRect(x: 0, y: 0, width: 20, height: 20))
        f.editing.window.contentView?.addSubview(field)
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            f.view.appearance = NSAppearance(named: appearance)
            for focusBody in [true, false] {
                f.editing.window.makeKeyAndOrderFront(nil)
                f.view.setSelectedRange(NSRange(location: 0, length: f.view.sourceUTF16Length))
                f.editing.window.makeFirstResponder(focusBody ? f.view : field)
                #expect((f.editing.window.firstResponder === f.view) == focusBody)
                f.view.needsDisplay = true; f.view.layoutSubtreeIfNeeded(); f.view.displayIfNeeded()
                let area = CGRect(x: formula.frame.minX - 3, y: formula.frame.minY - 2, width: 8, height: formula.frame.height + 6)
                let bitmap = try #require(f.view.bitmapImageRepForCachingDisplay(in: area))
                f.view.cacheDisplay(in: area, to: bitmap)
                let scale = Double(bitmap.pixelsWide) / area.width
                var difference = 0.0
                for y in 0..<bitmap.pixelsHigh {
                    let a = try #require(bitmap.colorAt(x: Int(scale), y: y)?.usingColorSpace(.sRGB))
                    let b = try #require(bitmap.colorAt(x: Int(5 * scale), y: y)?.usingColorSpace(.sRGB))
                    difference = max(difference, abs(a.redComponent - b.redComponent), abs(a.greenComponent - b.greenComponent), abs(a.blueComponent - b.blueComponent))
                }
                #expect(difference <= 0.03, "theme=\(appearance), body=\(focusBody), background delta=\(difference)")
            }
        }
        let error = try #require(f.view.inlineMathViews.first { $0.attachment.item.prepared.reason != nil })
        #expect(error.hitTest(.zero) == nil)
        let tag = try #require(f.view.formulaTooltips.keys.first)
        #expect(f.view.view(f.view, stringForToolTip: tag, point: error.frame.origin, userData: nil).contains("unknowncommand"))
        await f.close()
    }

    @Test func duplicateMountedAttachmentDoesNotCrashLayout() async throws {
        let f = try ReadingTests.Fixture("First $x$.\n\nSecond $x$.\n")
        try await f.start(mathService: MathTests.service())
        let first = try #require(f.view.inlineMathViews.first)
        f.view.inlineMathViews.append(first)
        f.view.needsLayout = true; f.view.layoutSubtreeIfNeeded()
        #expect(Set(f.view.inlineMathViews.map { ObjectIdentifier($0.attachment) }).count == f.view.inlineMathViews.count)
        await f.close()
    }

}
