import AppKit
import CoreText
import HortusCore
import Testing
@testable import Hortus

@Suite(.serialized) @MainActor
struct MathTests {
    static let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    static func service() -> MathService {
        MathService(resourceDirectory: root.appendingPathComponent("apps/macos/Resources/Fonts/KaTeX"))
    }

    @Test func dashedArrayBitmapUsesThemeAndExplicitStrokeColors() async throws {
        let service = Self.service()
        let array = "\\begin{array}{c:c}a&b\\\\\\hdashline c&d\\end{array}"
        for (source, foreground, expected) in [
            (array, [0.92, 0.94, 0.96, 1.0], [0.92, 0.94, 0.96]),
            ("\\color{red}" + array, [0.92, 0.94, 0.96, 1.0], [1.0, 0.0, 0.0]),
            ("\\color{black}" + array, [0.92, 0.94, 0.96, 1.0], [0.0, 0.0, 0.0])
        ] {
            let math = try await service.render(source, style: .display, bodySize: 30)
            var sampled = 0
            for drawing in math.drawing {
                guard case let .line(point, width, thickness, true, _) = drawing else { continue }
                let context = try #require(CGContext(data: nil, width: 256, height: 64, bitsPerComponent: 8,
                    bytesPerRow: 256 * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
                context.setAllowsAntialiasing(false)
                // 让描边状态残留一种无关颜色，验证每条虚线都设置自己的颜色。
                context.setStrokeColor(CGColor(red: 0, green: 1, blue: 0, alpha: 1))
                MathRenderer.draw(math, in: context, origin: CGPoint(x: 8 - point.x, y: 32 - point.y),
                    foreground: CGColor(colorSpace: CGColorSpace(name: CGColorSpace.sRGB)!, components: foreground.map { CGFloat($0) })!, deviceScale: 2)
                let pixels = try #require(context.data).assumingMemoryBound(to: UInt8.self)
                let x = Int(8 + min(width / 2, max(thickness, 0.5))), y = 32
                let offset = y * context.bytesPerRow + x * 4
                #expect(pixels[offset + 3] == 255)
                for channel in 0..<3 { #expect(abs(Double(pixels[offset + channel]) / 255 - expected[channel]) <= 0.01) }
                sampled += 1
            }
            #expect(sampled > 0)
        }
    }

    @Test func containerErrorDisplayHidesPrefixesAndPreservesMarkdownCopy() async throws {
        for source in ["> $$\n> \\frac{1}\n> $$\n", "100. $$\n     \\frac{1}\n     $$\n"] {
            let fixture = try ReadingTests.Fixture(source)
            try await fixture.start(mathService: Self.service())
            let view = try #require(fixture.view.mathViews.first)
            #expect(view.item.prepared.block.displaySource == "$$\n\\frac{1}\n$$")
            let lines = view.item.prepared.fallbackLines
            let displayed = lines.compactMap { CTLineGetGlyphRuns($0) as? [CTRun] }.flatMap { $0 }
                .compactMap { (CTRunGetAttributes($0) as NSDictionary)[kCTFontAttributeName] }
            #expect(!displayed.isEmpty)
            let expected = "$$\n\\frac{1}\n$$"
            #expect(lines.reduce(0) { $0 + CTLineGetStringRange($1).length } == (expected as NSString).length)
            let event = try #require(NSEvent.mouseEvent(with: .rightMouseDown, location: .zero, modifierFlags: [],
                timestamp: 0, windowNumber: fixture.editing.window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
            let menu = try #require(view.menu(for: event)), copy = try #require(menu.items.first)
            #expect(NSApp.sendAction(try #require(copy.action), to: copy.target, from: copy))
            let raw = (source as NSString).substring(with: NSRange(location: Int(view.item.prepared.block.sourceStart), length: Int(view.item.prepared.block.sourceLength)))
            #expect(NSPasteboard.general.string(forType: .string) == raw)
            await fixture.close()
        }
    }

    @Test func numberedListErrorLinesFitInsideTheirActualCanvas() async throws {
        let source = "10. $$\\" + String(repeating: "unknowncommand", count: 300) + "$$\n"
        let fixture = try ReadingTests.Fixture(source)
        fixture.editing.window.setContentSize(NSSize(width: 472, height: 540))
        try await fixture.start(mathService: Self.service())
        let view = try #require(fixture.view.mathViews.first)
        #expect(view.reservedPrefixWidth > 0)
        let width = try #require(view.scroll.documentView).bounds.width
        for line in view.item.prepared.fallbackLines + view.item.prepared.reasonLines {
            #expect(8 + CTLineGetTypographicBounds(line, nil, nil, nil) <= width - 8 + 0.5)
        }
        await fixture.close()
    }

    @Test func corpusGlyphAuditAndFallbackMetricsRunInBackground() async throws {
        let service = Self.service()
        let samples = try String(contentsOf: Self.root.appendingPathComponent("fixtures/math/samples.txt"), encoding: .utf8)
            .split(separator: "\n", omittingEmptySubsequences: false).filter { !$0.hasPrefix("# ") }.dropLast()
        var successes = 0, glyphs = 0, maximumError = 0.0
        for source in samples {
            for style in [MathStyle.display, .text] {
                do {
                    let result = try await service.render(String(source), style: style, bodySize: 15)
                    successes += 1; glyphs += result.glyphCount
                    maximumError = max(maximumError, result.maximumMetricError)
                } catch is MathFailure { } // 公开样本里的有意错误由 Rust 逐项核对。
            }
        }
        #expect(successes == 374)
        #expect(glyphs > 1000)
        #expect(maximumError * 2 <= 0.5)
        #expect(await service.mainThreadPreparations == 0)
        #expect(await service.registrationCount == 1)
        let measurements = await service.measurementCount
        #expect(measurements > 0)
        _ = try await service.render("\\text{中文😀}", style: .display, bodySize: 15)
        let first = await service.measurementCount
        _ = try await service.render("\\text{中文😀}", style: .text, bodySize: 20)
        #expect(await service.measurementCount == first)
    }

    @Test func unicodeFallbackGlyphsAreMeasuredAndDrawnIndividually() async throws {
        let source = try String(contentsOf: Self.root.appendingPathComponent("fixtures/math/missing-glyphs.md"), encoding: .utf8)
        let service = Self.service()
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: service)
        #expect(fixture.view.mathViews.count == 6)
        #expect(fixture.view.mathViews.allSatisfy { $0.item.prepared.math != nil })
        #expect(fixture.view.mathViews.allSatisfy { $0.item.prepared.math?.unresolvedGlyphCount == 0 })
        #expect(fixture.view.mathViews.allSatisfy { $0.item.prepared.math?.unresolvedMetricCount == 0 })
        let measured = Set(await service.measuredFallbackCodepoints)
        #expect(Set([0x041F, 0x03B1, 0x20BB7, 0x2764, 0x1F468]).isSubset(of: measured))
        let accent = try #require(fixture.view.mathViews.last?.item.prepared.math)
        #expect(accent.maximumMetricError * 2 <= 0.5)
        await fixture.close()
    }

    @Test func packetRejectsTruncationUnknownVersionAndInvalidGeometry() throws {
        let bytes = try MathEngine().render(source: "x", style: .display)
        #expect(throws: (any Error).self) { try MathScene(bytes.dropLast()) }
        var version = bytes; version[0] = 2
        #expect(throws: (any Error).self) { try MathScene(version) }
        var value = try #require(JSONSerialization.jsonObject(with: bytes.dropFirst(8)) as? [String: Any])
        var items = try #require(value["items"] as? [[String: Any]])
        items[0].removeValue(forKey: "scale"); value["items"] = items
        let payload = try JSONSerialization.data(withJSONObject: value)
        var packet = Data([1, 0, 0, 0]); var length = UInt32(payload.count).littleEndian
        withUnsafeBytes(of: &length) { packet.append(contentsOf: $0) }; packet.append(payload)
        #expect(throws: (any Error).self) { try MathScene(packet) }
    }

    @Test func nativeBlockGeometrySourceCopyAccessibilityErrorAndReuse() async throws {
        let source = "前😀\n\n$$\\frac{a}{b}$$\n\n$$\\unknowncommand$$\n\n$$$$\n\n$a_1 b_1$\n\n$$" + Array(repeating: "x", count: 120).joined(separator: "+") + "$$\n\n后文\n"
        let service = Self.service()
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: service)
        #expect(fixture.view.mathViews.count == 3)
        let math = try #require(fixture.view.mathViews.first)
        #expect(math.item.prepared.math != nil)
        #expect(math.accessibilityRole() == .image)
        #expect(math.accessibilityLabel() == L10n.string("Formula: %@", "\\frac{a}{b}"))
        #expect(math.accessibilityChildren()?.isEmpty == true)
        #expect(fixture.view.mathViews[1].item.prepared.reason?.contains("unknowncommand") == true)
        #expect(fixture.view.string.contains("$$$$") && fixture.view.string.contains("$a_1 b_1$"))
        let wide = try #require(fixture.view.mathViews.last)
        #expect(wide.scroll.hasHorizontalScroller)
        #expect(!wide.scroll.hasVerticalScroller)
        #expect(wide.scroll.documentView!.bounds.width > wide.bounds.width)
        #expect(!fixture.view.enclosingScrollView!.hasHorizontalScroller)
        fixture.view.setSelectedRange(math.item.range); fixture.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == "$$\\frac{a}{b}$$")
        let event = try #require(NSEvent.mouseEvent(with: .rightMouseDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: fixture.editing.window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
        let menu = try #require(math.scroll.documentView?.menu(for: event))
        #expect(menu.items.first?.title == L10n.string("Copy Markdown Source"))
        let manager = try #require(fixture.view.textLayoutManager), content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        var replacements = 0
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            if let size = (fragment as? NoteLayoutFragment)?.replacement { #expect(size.height > 20); replacements += 1 }
            return true
        }
        #expect(replacements == 3)
        fixture.coordinator.refresh(force: true); try await fixture.ready()
        #expect(fixture.view.mathViews.first === math)
        await fixture.close()
    }

    @Test func missingFontsProduceIndependentFallbackWithoutRegistrationRetries() async throws {
        let service = MathService(resourceDirectory: URL(fileURLWithPath: "/nonexistent-hortus-fonts"))
        let engine = MathEngine()
        let scene = try MathScene(engine.render(source: "x", style: .display))
        #expect(scene.width > 0)
        for _ in 0..<2 {
            do { _ = try await service.render("x", style: .display, bodySize: 15); Issue.record("缺失字体应返回错误") }
            catch { }
        }
        #expect(await service.registrationCount == 0)
        let source = "$$x$$"
        let block = MathBlock(blockId: 1, sourceStart: 0, sourceLength: 5, displayStart: 0,
                              displayLength: 1, displaySource: source, latex: "x", style: .display, block: true)
        let prepared = try #require(await service.prepare([block], source: source, size: 15, width: 100, displayText: "\u{fffc}").first)
        #expect(prepared.reason?.hasPrefix(L10n.string("Formula fonts could not be loaded: %@; showing source", "").components(separatedBy: CharacterSet(charactersIn: ":："))[0]) == true)
    }

    @Test func longFormulaErrorsWrapSourceAndReasonByCharacterClusters() async throws {
        let latex = "\\" + String(repeating: "unknowncommand", count: 300)
        let source = "$$\(latex)$$"
        let block = MathBlock(blockId: 1, sourceStart: 0, sourceLength: UInt64((source as NSString).length),
                              displayStart: 0, displayLength: 1, displaySource: source, latex: latex, style: .display, block: true)
        let prepared = try #require(await Self.service().prepare([block], source: source, size: 15, width: 100, displayText: "\u{fffc}").first)
        #expect(prepared.math == nil)
        #expect(prepared.reason?.hasPrefix(L10n.string("Formula error: %@; showing source", "").components(separatedBy: CharacterSet(charactersIn: ":："))[0]) == true)
        #expect(prepared.fallbackLines.count > 1)
        #expect(prepared.reasonLines.count > 1)
        func assertWrapped(_ lines: [CTLine], text: String) {
            let ranges = lines.map { CTLineGetStringRange($0) }
            #expect(ranges.first?.location == 0)
            #expect(ranges.last.map { $0.location + $0.length } == (text as NSString).length)
            #expect(zip(ranges, ranges.dropFirst()).allSatisfy { $0.location + $0.length == $1.location })
            #expect(lines.allSatisfy { CTLineGetTypographicBounds($0, nil, nil, nil) <= 84.5 })
        }
        assertWrapped(prepared.fallbackLines, text: source)
        assertWrapped(prepared.reasonLines, text: prepared.reason!)
    }

    @Test func containerMarkersAndQuoteSurfaceSurviveReplacement() async throws {
        let fixture = try ReadingTests.Fixture("> [!note] Formula\n> $$x^2$$\n\n- $$y^2$$\n\n1. $$z^2$$\n")
        try await fixture.start(mathService: Self.service())
        let manager = try #require(fixture.view.textLayoutManager), content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        var formulas: [NoteLayoutFragment] = []
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            if let fragment = fragment as? NoteLayoutFragment, fragment.replacement != nil { formulas.append(fragment) }
            return true
        }
        #expect(formulas.count == 3)
        #expect((formulas.first?.sourceMask ?? 0) & (64 | 4096) != 0)
        #expect(formulas.first?.quoteHead == false && formulas.first?.quoteTail == true)
        #expect(formulas.dropFirst().compactMap { $0.replacementPrefix?.string.trimmingCharacters(in: .whitespaces) } == ["•", "1."])
        #expect(fixture.view.mathViews.dropFirst().first?.reservedPrefixWidth ?? 0 > 0)
        await fixture.close()
    }

    @Test func listFormulasStartAfterTheirPrefixAndUseOnlyBodyWidthForCentering() async throws {
        let wide = Array(repeating: "x", count: 120).joined(separator: "+")
        for marker in ["- ", "100. "] {
            for latex in ["x", wide] {
                let fixture = try ReadingTests.Fixture("\(marker)$$\(latex)$$\n")
                try await fixture.start(mathService: Self.service())
                let view = try #require(fixture.view.mathViews.first)
                let canvas = try #require(view.scroll.documentView)
                let math = try #require(view.item.prepared.math)
                #expect(view.reservedPrefixWidth > 0)
                #expect(view.scroll.frame.minX >= 12 + view.reservedPrefixWidth)
                let origin = max(0, (canvas.bounds.width - math.size.width) / 2)
                #expect(view.scroll.frame.minX + origin >= 12 + view.reservedPrefixWidth)
                if latex == "x" {
                    #expect(!view.scroll.hasHorizontalScroller)
                    #expect(abs(origin + math.size.width / 2 - view.scroll.bounds.width / 2) <= 0.5)
                } else {
                    #expect(view.scroll.hasHorizontalScroller && origin == 0)
                }
                await fixture.close()
            }
        }
    }

    @Test func listContinuationFormulasStartAfterTheBodyIndentation() async throws {
        for (marker, indent) in [("- ", "  "), ("100. ", "     ")] {
            let fixture = try ReadingTests.Fixture("\(marker)Solve\n\n\(indent)$$x^2=1$$\n")
            try await fixture.start(mathService: Self.service())
            let view = try #require(fixture.view.mathViews.first)
            let font = NSFont.systemFont(ofSize: 15)
            let expected = (indent as NSString).size(withAttributes: [.font: font]).width
            #expect(fixture.view.string.contains("\n\(indent)\u{fffc}"))
            #expect(view.reservedPrefixWidth >= expected - 0.5)
            #expect(view.scroll.frame.minX >= 12 + expected - 0.5)
            await fixture.close()
        }
    }

    @Test func duplicateFormulasKeepDistinctViewsAcrossThemeAndExternalRefresh() async throws {
        let source = "> intro\n>\n> $$x^2$$\n>\n> $$x^2$$\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: Self.service())
        #expect(fixture.view.mathViews.count == 2)
        #expect(Set(fixture.view.mathViews.map(ObjectIdentifier.init)).count == 2)
        func assertUniqueAccessibility() {
            let elements = (fixture.view.accessibilityChildren() ?? []).compactMap { $0 as? MathBlockView }
            #expect(elements.count == 2)
            #expect(Set(elements.map(ObjectIdentifier.init)).count == 2)
        }
        assertUniqueAccessibility()
        fixture.view.appearance = NSAppearance(named: .darkAqua)
        fixture.coordinator.refresh(force: true)
        try await fixture.ready()
        #expect(Set(fixture.view.mathViews.map(ObjectIdentifier.init)).count == 2)
        assertUniqueAccessibility()
        let changed = source.replacingOccurrences(of: "intro", with: "introduction")
        try Data(changed.utf8).write(to: fixture.document.io.url)
        fixture.document.refresh()
        for _ in 0..<500 {
            if fixture.document.text == changed && !fixture.coordinator.pending { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.document.text == changed && !fixture.coordinator.pending)
        #expect(fixture.view.mathViews.count == 2)
        #expect(Set(fixture.view.mathViews.map(ObjectIdentifier.init)).count == 2)
        assertUniqueAccessibility()
        await fixture.close()
    }

    @Test func externalTextBeforeFormulaRepositionsReusedViewAndKeepsDocumentHeight() async throws {
        let source = "intro\n\n$$x^2$$\n\nAfter\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: Self.service())
        let math = try #require(fixture.view.mathViews.first)
        let changed = source.replacingOccurrences(of: "intro", with: "introduction")
        try Data(changed.utf8).write(to: fixture.document.io.url)
        fixture.document.refresh()
        for _ in 0..<500 {
            if fixture.document.text == changed && !fixture.coordinator.pending { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(fixture.document.text == changed && !fixture.coordinator.pending)
        #expect(fixture.view.mathViews.first === math)
        let manager = try #require(fixture.view.textLayoutManager)
        let content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        var formulaFragment: NSTextLayoutFragment?
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            if (fragment as? NoteLayoutFragment)?.replacement != nil { formulaFragment = fragment }
            return true
        }
        let fragment = try #require(formulaFragment)
        #expect(abs(math.frame.minY - fragment.layoutFragmentFrame.minY - fixture.view.textContainerInset.height) < 0.5)
        #expect(abs(math.frame.height - math.preferredSize.height) < 0.5)
        #expect(fixture.view.frame.height >= manager.usageBoundsForTextContainer.maxY + 2 * fixture.view.textContainerInset.height - 0.5)
        await fixture.close()
    }

    @Test func externalFormulaChangeReusesOtherViewsAndPreservesAnchor() async throws {
        let source = (0..<60).map { "Paragraph \($0)\n\n$$x_{\($0)}^2$$\n\n" }.joined()
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: Self.service())
        let old = fixture.view.mathViews
        fixture.coordinator.reveal(source: Int(old[30].item.prepared.block.sourceStart))
        try await Task.sleep(for: .milliseconds(50))
        let before = try #require(fixture.coordinator.visibleAnchor())
        let changed = source.replacingOccurrences(of: "$$x_{30}^2$$", with: "$$x_{30}^3+y^3$$")
        try Data(changed.utf8).write(to: fixture.document.io.url)
        fixture.document.refresh()
        for _ in 0..<1000 {
            if fixture.document.text == changed { break }
            try await Task.sleep(for: .milliseconds(1))
        }
        #expect(fixture.document.text == changed)
        try await fixture.ready()
        let after = try #require(fixture.coordinator.visibleAnchor())
        #expect(fixture.coordinator.model?.rebuilt == 1)
        #expect(fixture.view.mathViews.filter { current in old.contains { $0 === current } }.count == 59)
        #expect(before.id == after.id && abs(before.offset - after.offset) <= 1)
        await fixture.close()
    }
}
