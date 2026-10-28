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
    }

    @Test func longFormulaErrorsWrapSourceAndReasonByCharacterClusters() async throws {
        let latex = "\\" + String(repeating: "unknowncommand", count: 300)
        let source = "$$\(latex)$$"
        let block = MathBlock(blockId: 1, sourceStart: 0, sourceLength: UInt64((source as NSString).length),
                              displayStart: 0, displayLength: 1, latex: latex, style: .display, block: true)
        let prepared = try #require(await Self.service().prepare([block], source: source, size: 15, width: 100).first)
        #expect(prepared.math == nil)
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

    @Test func duplicateFormulasKeepDistinctViewsAcrossThemeAndExternalRefresh() async throws {
        let source = "> intro\n>\n> $$x^2$$\n>\n> $$x^2$$\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: Self.service())
        #expect(fixture.view.mathViews.count == 2)
        #expect(Set(fixture.view.mathViews.map(ObjectIdentifier.init)).count == 2)
        fixture.view.appearance = NSAppearance(named: .darkAqua)
        fixture.coordinator.refresh(force: true)
        try await fixture.ready()
        #expect(Set(fixture.view.mathViews.map(ObjectIdentifier.init)).count == 2)
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
