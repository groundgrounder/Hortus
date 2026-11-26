import AppKit
import CoreGraphics
import HortusCore
import HortusApple
import ImageIO
import Testing
@testable import Hortus

@Suite(.serialized) @MainActor
struct PolishTests {
    static let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    static func mathService() -> MathService {
        MathService(resourceDirectory: root.appendingPathComponent("apps/macos/Resources/Fonts/KaTeX"))
    }

    // MARK: 3.3 居中舒适最大列宽

    @Test func readingAndEditingShareCenteredColumnWidthAndMmdFillsWideWindow() async throws {
        #expect(ReadingColumnLayout.targetWidth(fontSize: 12) == 760)
        #expect(ReadingColumnLayout.targetWidth(fontSize: 15) == 840)
        #expect(ReadingColumnLayout.targetWidth(fontSize: 20) == 960)
        #expect(ReadingColumnLayout.horizontalInset(availableWidth: 1400, fontSize: 15, isMmd: false, isEmbedded: false) == 280)
        #expect(ReadingColumnLayout.horizontalInset(availableWidth: 680, fontSize: 15, isMmd: false, isEmbedded: false) == 30)
        #expect(ReadingColumnLayout.horizontalInset(availableWidth: 1400, fontSize: 15, isMmd: true, isEmbedded: false) == 30)

        let source = "# Title\n\nBody paragraph.\n"
        let reading = try ReadingTests.Fixture(source)
        let editing = try EditingTests.Fixture(source)
        reading.editing.window.setContentSize(NSSize(width: 1400, height: 600))
        editing.window.setContentSize(NSSize(width: 1400, height: 600))
        try await reading.start()
        try await editing.ready()
        reading.view.layoutSubtreeIfNeeded()
        editing.view.layoutSubtreeIfNeeded()
        #expect(abs(reading.view.textContainerInset.width - editing.view.textContainerInset.width) < 0.5)
        #expect(abs(reading.view.layoutContentWidth - 840) <= 1)
        #expect(abs(editing.view.layoutContentWidth - 840) <= 1)

        reading.editing.window.setContentSize(NSSize(width: 680, height: 600))
        editing.window.setContentSize(NSSize(width: 680, height: 600))
        reading.view.layoutSubtreeIfNeeded()
        editing.view.layoutSubtreeIfNeeded()
        #expect(reading.view.textContainerInset.width == 30)
        #expect(editing.view.textContainerInset.width == 30)
        await reading.close()
        await editing.close()

        let mmd = try EditingTests.Fixture("flowchart LR\n  A --> B\n", path: "arch.mmd")
        mmd.window.setContentSize(NSSize(width: 1400, height: 600))
        try await mmd.ready()
        mmd.view.layoutSubtreeIfNeeded()
        #expect(mmd.view.textContainerInset.width == 30)
        #expect(mmd.view.layoutContentWidth >= 1300)
        await mmd.close()
    }

    // MARK: 3.4 块级公式与表格垂直滚轮透传

    @Test func formulaScrollViewPassesVerticalWheelToOuterResponder() throws {
        final class OuterScrollProbe: NSScrollView {
            var forwardedEvents = 0
            override func scrollWheel(with event: NSEvent) {
                forwardedEvents += 1
            }
        }
        let outer = OuterScrollProbe(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
        let inner = FormulaScrollView(frame: NSRect(x: 0, y: 0, width: 300, height: 80))
        let wideDoc = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 80))
        inner.documentView = wideDoc
        outer.addSubview(inner)

        let verticalCG = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: -24, wheel2: 0, wheel3: 0))
        let verticalEvent = try #require(NSEvent(cgEvent: verticalCG))
        inner.scrollWheel(with: verticalEvent)
        #expect(outer.forwardedEvents == 1)

        // 横向可滚且以横向为主时不透传。
        inner.contentView.scroll(to: CGPoint(x: 100, y: 0))
        let horizontalCG = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: 0, wheel2: -24, wheel3: 0))
        let horizontalEvent = try #require(NSEvent(cgEvent: horizontalCG))
        inner.scrollWheel(with: horizontalEvent)
        #expect(outer.forwardedEvents == 1)
    }

    // MARK: 3.5 嵌入缓存复用、身份防碰撞与滚动锚定

    @Test func embedRendererCachesReusesAndSeparatesHeadingIdentities() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let vault = dir.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
        let child = """
        # One
        ```mermaid
        flowchart LR
          A --> B
        ```

        # Two
        ```mermaid
        flowchart LR
          C --> D
        ```
        """
        try "# Host\n".write(to: vault.appendingPathComponent("host.md"), atomically: true, encoding: .utf8)
        try child.write(to: vault.appendingPathComponent("child.md"), atomically: true, encoding: .utf8)
        let core = try WorkspaceCore.inDirectories(config: dir.appendingPathComponent("config").path,
                                                   cache: dir.appendingPathComponent("cache").path)
        let handle = try core.openVault(path: vault.path)
        handle.updateSearchIndex(upserted: ["host.md", "child.md"], removed: [])
        defer { handle.close(); try? FileManager.default.removeItem(at: dir) }

        let renderer = EmbedRenderer()
        let service = DiagramService()
        let req1 = EmbedRequest(source: "host.md", target: "child", heading: "One", alias: nil,
                                ancestors: ["host.md"], depth: 0, handle: handle, vault: vault.path,
                                width: 500, size: 15, dark: false, service: service, mathService: Self.mathService(),
                                knowledgeVersion: 1)
        let req2 = EmbedRequest(source: "host.md", target: "child", heading: "Two", alias: nil,
                                ancestors: ["host.md"], depth: 0, handle: handle, vault: vault.path,
                                width: 500, size: 15, dark: false, service: service, mathService: Self.mathService(),
                                knowledgeVersion: 1)
        let p1 = try #require(await renderer.prepare(req1))
        let p1Again = try #require(await renderer.prepare(req1))
        let p2 = try #require(await renderer.prepare(req2))
        #expect(await renderer.cacheHitCount == 1)
        #expect(p1.diagrams.first?.documentIdentity == p1Again.diagrams.first?.documentIdentity)
        #expect(p1.diagrams.first?.documentIdentity != p2.diagrams.first?.documentIdentity)

        var req1Updated = req1
        req1Updated.knowledgeVersion = 2
        _ = await renderer.prepare(req1Updated)
        #expect(await renderer.cacheHitCount == 1)
    }

    // MARK: 3.6 大纲随滚动高亮当前小节

    @Test func outlineActiveHeadingTracksScrollAndEditingCaret() async throws {
        let source = (0..<30).map { "## Section \($0)\n\nParagraph text for section \($0).\n\n" }.joined()
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start()
        for _ in 0..<300 where fixture.document.outlineRevision != fixture.document.revision {
            try await Task.sleep(for: .milliseconds(10))
        }
        try #require(fixture.document.headings.count == 30)
        let target = fixture.document.headings[15]
        let displayPos = fixture.coordinator.displayPosition(source: Int(target.start))
        let targetRect = fixture.coordinator.rect(at: displayPos)
        fixture.editing.scroll.contentView.scroll(to: CGPoint(x: 0, y: max(0, targetRect.minY)))
        fixture.editing.scroll.reflectScrolledClipView(fixture.editing.scroll.contentView)
        fixture.view.layoutSubtreeIfNeeded()
        fixture.coordinator.syncActiveHeading()
        #expect(fixture.document.activeHeadingStart == Int(target.start))
        await fixture.close()

        let editing = try EditingTests.Fixture(source)
        try await editing.ready()
        for _ in 0..<300 where editing.document.outlineRevision != editing.document.revision {
            try await Task.sleep(for: .milliseconds(10))
        }
        let editTarget = editing.document.headings[10]
        editing.view.setSelectedRange(NSRange(location: Int(editTarget.start) + 4, length: 0))
        editing.coordinator.mediaSelectionChanged()
        #expect(editing.document.activeHeadingStart == Int(editTarget.start))
        await editing.close()
    }

    // MARK: 3.7 阅读视图 ⌘F 搜索公式内容

    @Test func readingViewFindMatchesInlineAndBlockFormulaLatex() async throws {
        let source = "Intro text with $\\alpha + \\beta$ and $E=mc^2$.\n\n$$\\sum_{i=1}^n i^2$$\n\nEnding text.\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: Self.mathService())
        let finder = fixture.view.ensureReadingFinder()
        let alphaMatches = finder.matches(for: "\\alpha")
        #expect(alphaMatches.count == 1 && alphaMatches[0].isFormula)
        let emcMatches = finder.matches(for: "E=mc^2")
        #expect(emcMatches.count == 1 && emcMatches[0].isFormula)
        let sumMatches = finder.matches(for: "\\sum_{i=1}^n")
        #expect(sumMatches.count == 1 && sumMatches[0].isFormula)
        let rects = finder.rects(forCharacterRange: sumMatches[0].virtualRange)
        #expect(rects?.isEmpty == false)
        finder.scrollRangeToVisible(sumMatches[0].virtualRange)
        await fixture.close()
    }

    // MARK: 3.8 小接缝验证（快捷键、topHit、表格双链、NFC/NFD、行内注释双空格、pendingSends）

    @Test func tableCellsSupportWikilinksBrokenLinksAndVoiceOverFormulaCells() async throws {
        let source = "| Note | Formula |\n| --- | --- |\n| [[Target]] and [[Missing]] #tag1 | $x^2 + y^2$ |\n"
        let fixture = try ReadingTests.Fixture(source)
        let targetURL = fixture.editing.vault.appendingPathComponent("Target.md")
        try "# Target\n".write(to: targetURL, atomically: true, encoding: .utf8)
        let handle = try fixture.editing.core.openVault(path: fixture.editing.vault.path)
        handle.updateSearchIndex(upserted: ["note.md", "Target.md"], removed: [])
        defer { handle.close() }

        try await fixture.start(mathService: Self.mathService(), knowledgeHandle: handle)
        let media = try #require(fixture.view.mediaViews.first)
        let cells = media.tableInspectionCells
        #expect(cells.count == 4)
        let links = try #require(cells[2]["links"] as? [[String: Any]])
        #expect(links.count == 2)
        #expect(links[0]["target"] as? String == "Target" && links[0]["broken"] as? Bool == false)
        #expect(links[1]["target"] as? String == "Missing" && links[1]["broken"] as? Bool == true)

        let canvas = try #require(media.tableCanvasView)
        let axCells = (canvas.accessibilityChildren() ?? []).compactMap { $0 as? NSAccessibilityElement }
        #expect(axCells.count == 4)
        let formulaLabel = axCells[3].accessibilityLabel() ?? ""
        #expect(formulaLabel.contains("x^2 + y^2"))
        await fixture.close()
    }

    @Test func localImageCacheSupportsNfcNfdEquivalenceInsideVaultOnly() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let vault = dir.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let nfdName = "cafe\u{0301}.png"
        let nfcName = "caf\u{00E9}.png"
        let nfdURL = vault.appendingPathComponent(nfdName)
        try MediaTests.writeImage(nfdURL, width: 64, height: 32)
        let cache = LocalImageCache()
        let nfcPath = vault.appendingPathComponent(nfcName).path
        let decoded = await cache.decode(nfcPath, vault: vault.path)
        #expect(decoded != nil)
        #expect(await cache.decode(nfcPath, vault: vault.path + "-other") == nil)
    }

    @Test func inlineCommentFoldsDoubleSpaceAndNoPrivateUseCharacterLeaks() async throws {
        let source = "---\ntitle: Demo\n---\n\nfoo %%hidden%% bar\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start()
        #expect(fixture.view.string.contains("foo bar"))
        #expect(!fixture.view.string.contains("foo  bar"))
        #expect(!fixture.view.string.contains("\u{e000}"))
        fixture.view.setSelectedRange(NSRange(location: 0, length: fixture.view.sourceUTF16Length))
        let plain = try #require(fixture.view.selectionPlainText())
        #expect(!plain.contains("\u{e000}"))
        await fixture.close()
    }

    @Test func inlineCommentFoldPreservesInlineMathOffsetsAndFinderMatches() async throws {
        let source = "foo %%hidden%% bar $x^2$\n\na %%c1%% b $y^2$ c <!-- c2 --> d $z^3$ end\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: Self.mathService())
        let display = fixture.view.string as NSString
        #expect(!fixture.view.string.contains("foo  bar"))
        #expect(!fixture.view.string.contains("a  b"))
        #expect(!fixture.view.string.contains("c  d"))
        #expect(fixture.view.inlineMath.count == 3)

        var matchedLatex: [String] = []
        for (location, attachment) in fixture.view.inlineMath.sorted(by: { $0.key < $1.key }) {
            let range = attachment.item.range
            #expect(range.location == location)
            #expect(range.length == 1)
            #expect(NSMaxRange(range) <= display.length)
            #expect(display.character(at: range.location) == 0xFFFC)
            matchedLatex.append(attachment.item.prepared.block.latex)
        }
        #expect(matchedLatex == ["x^2", "y^2", "z^3"])

        let finder = fixture.view.ensureReadingFinder()
        for query in ["x^2", "y^2", "z^3"] {
            let hits = finder.matches(for: query)
            #expect(hits.count == 1)
            let hit = try #require(hits.first)
            #expect(hit.isFormula)
            #expect(hit.realRange.length == 1)
            #expect(NSMaxRange(hit.realRange) <= display.length)
            #expect(display.character(at: hit.realRange.location) == 0xFFFC)
        }
        await fixture.close()
    }

    @Test func embedAsyncHeightGrowthPreservesScrollAnchorBelowEmbed() async throws {
        let body = (0..<24).map { "## Section \($0)\n\nParagraph text for section \($0) with enough length to occupy vertical space.\n\n" }.joined()
        let source = "![[child]]\n\n" + body
        let fixture = try ReadingTests.Fixture(source)
        try "# Child\n\nEmbedded body paragraph.\n".write(
            to: fixture.editing.vault.appendingPathComponent("child.md"),
            atomically: true, encoding: .utf8
        )
        let handle = try fixture.editing.core.openVault(path: fixture.editing.vault.path)
        handle.updateSearchIndex(upserted: ["note.md", "child.md"], removed: [])
        defer { handle.close() }

        try await fixture.start(mathService: Self.mathService(), knowledgeHandle: handle)
        let media = try #require(fixture.view.mediaViews.first)
        // 等待嵌入卡片完成初次异步加载，确保后续滚动测试不受初始装载竞态干扰。
        for _ in 0..<200 where media.inspectionEmbeddedCard == nil || media.preferredSize.height <= 48 {
            try await Task.sleep(for: .milliseconds(10))
        }
        try #require(media.inspectionEmbeddedCard != nil)
        for _ in 0..<200 where fixture.document.outlineRevision != fixture.document.revision {
            try await Task.sleep(for: .milliseconds(10))
        }
        try #require(fixture.document.headings.count == 24)

        let targetHeading = fixture.document.headings[12]
        let displayPos = fixture.coordinator.displayPosition(source: Int(targetHeading.start))
        if let manager = fixture.view.textLayoutManager,
           let content = manager.textContentManager,
           let endLoc = content.location(content.documentRange.location, offsetBy: displayPos),
           let prefixRange = NSTextRange(location: content.documentRange.location, end: endLoc) {
            manager.ensureLayout(for: prefixRange)
        }
        let targetRect = fixture.coordinator.rect(at: displayPos)
        fixture.editing.scroll.contentView.scroll(to: CGPoint(x: 0, y: max(0, targetRect.minY)))
        fixture.editing.scroll.reflectScrolledClipView(fixture.editing.scroll.contentView)
        fixture.view.layoutSubtreeIfNeeded()

        let beforeVisibleMinY = fixture.view.visibleRect.minY
        let beforeAnchor = try #require(fixture.coordinator.visibleAnchor())
        let beforeAnchorPos = fixture.coordinator.displayPosition(source: beforeAnchor.source)
        let beforeOffset = fixture.coordinator.rect(at: beforeAnchorPos).minY - beforeVisibleMinY

        let oldHeight = media.preferredSize.height
        let growth: CGFloat = 240
        media.simulateEmbedHeightGrowthForTesting(oldHeight + growth)
        fixture.view.layoutSubtreeIfNeeded()

        let afterVisibleMinY = fixture.view.visibleRect.minY
        let afterAnchor = try #require(fixture.coordinator.visibleAnchor())
        let afterOffset = fixture.coordinator.rect(at: beforeAnchorPos).minY - afterVisibleMinY
        #expect(abs(media.preferredSize.height - (oldHeight + growth)) <= 0.5)
        #expect(afterAnchor.id == beforeAnchor.id)
        #expect(abs(afterOffset - beforeOffset) <= 1.0)
        await fixture.close()
    }

    @Test func tableMediaViewRejectsReuseWhenAppearanceOrContrastChanges() async throws {
        let source = "| A | B |\n| --- | --- |\n| 1 | 2 |\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: Self.mathService())
        let media = try #require(fixture.view.mediaViews.first)
        let prepared = media.prepared
        let width = media.renderWidth
        let size = media.renderSize
        let vault = media.vault
        let version = media.knowledgeVersion
        let dark = media.renderDark
        let contrast = media.renderIncreasedContrast

        #expect(media.canReuse(for: prepared, width: width, size: size, vault: vault,
                               knowledgeVersion: version, dark: dark, increasedContrast: contrast))
        #expect(!media.canReuse(for: prepared, width: width, size: size, vault: vault,
                                knowledgeVersion: version, dark: !dark, increasedContrast: contrast))
        #expect(!media.canReuse(for: prepared, width: width, size: size, vault: vault,
                                knowledgeVersion: version, dark: dark, increasedContrast: !contrast))
        await fixture.close()
    }

    @Test func terminalSessionClearsPendingSendsOnVaultSwitchAndStop() async {
        let session = TerminalSession()
        session.setInitialRoot("/tmp/vault-a")
        #expect(session.sendText("old-vault-payload", bracketedPaste: false))
        #expect(session.inspectionPendingSendCount == 1)
        await session.setRootAndWait("/tmp/vault-b")
        #expect(session.inspectionPendingSendCount == 0)
        #expect(session.sendText("second-payload", bracketedPaste: false))
        #expect(session.inspectionPendingSendCount == 1)
        await session.stopAndWait()
        #expect(session.inspectionPendingSendCount == 0)
    }

    @Test func preparedSceneTopHitPrefersNonContainerNode() throws {
        let id = DiagramIdentity(document: "d", block: 1, revision: 1, dark: false, fontGeneration: 0, requestGeneration: 0)
        let containerRect = SceneRect(x: 0, y: 0, width: 200, height: 200)
        let nodeRect = SceneRect(x: 40, y: 40, width: 80, height: 40)
        let sceneData = SceneData(
            schemaVersion: 2, identity: id,
            previous: nil,
            graphMetrics: nil, width: 200, height: 200,
            primitives: [
                .graphShape(rect: containerRect, shape: "Container", outline: [], details: [],
                            fill: SceneColor(r: 0, g: 0, b: 0, a: 0), stroke: SceneColor(r: 0, g: 0, b: 0, a: 1), strokeWidth: 1)
            ],
            hitRegions: [
                SceneHit(nodeId: "sub", label: "Sub", prefix: nil, rect: containerRect,
                         canCollapse: true, collapsed: false, sourceStart: 0, sourceEnd: 5, linkTarget: nil, linkHeading: nil),
                SceneHit(nodeId: "n1", label: "Node", prefix: nil, rect: nodeRect,
                         canCollapse: false, collapsed: false, sourceStart: 6, sourceEnd: 10, linkTarget: nil, linkHeading: nil),
            ],
            wireBytes: 0, layoutMs: 0, semanticJson: "{}", diagramKind: "flowchart"
        )
        let prepared = PreparedScene(scene: try DiagramScene(sceneData))
        #expect(prepared.topHit(at: CGPoint(x: 60, y: 60))?.nodeId == "n1")
        #expect(prepared.topHit(at: CGPoint(x: 10, y: 10))?.nodeId == "sub")
        #expect(prepared.topHit(at: CGPoint(x: 10, y: 10), includeContainer: false) == nil)
    }

    // MARK: 3.9 块级公式指针命中

    @Test func blockFormulaAccessibilityHitTestReturnsConsistentElement() async throws {
        let source = "Before\n\n$$a^2 + b^2 = c^2$$\n\nAfter\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start(mathService: Self.mathService())
        let math = try #require(fixture.view.mathViews.first)
        fixture.view.layoutSubtreeIfNeeded()
        let screenRect = fixture.editing.window.convertToScreen(math.convert(math.bounds, to: nil))
        let center = NSPoint(x: screenRect.midX, y: screenRect.midY)
        let fromTextView = fixture.view.accessibilityHitTest(center) as? NSAccessibilityElement
        let fromBlock = math.accessibilityHitTest(center) as? NSAccessibilityElement
        let fromScroll = math.scroll.accessibilityHitTest(center) as? NSAccessibilityElement
        let fromCanvas = math.scroll.documentView?.accessibilityHitTest(center) as? NSAccessibilityElement
        let element = try #require(fromTextView)
        #expect(fromBlock === element)
        #expect(fromScroll === element)
        #expect(fromCanvas === element)
        #expect(element.accessibilityLabel() == L10n.string("Formula: %@", "a^2 + b^2 = c^2"))
        #expect(!element.accessibilityFrame().isEmpty)
        await fixture.close()
    }

    // MARK: 3.10 FIX-10B 联合盲审清零回归

    @Test func copyMarkdownAfterInlineCommentsPreservesExactSource() async throws {
        let source = "foo %%c1%% bar baz\n\nalpha %%c1%% beta <!-- c2 --> gamma delta\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start()
        let displayed = fixture.view.string as NSString

        let singleRange1 = displayed.range(of: "b", range: displayed.range(of: "bar"))
        try #require(singleRange1.location != NSNotFound)
        fixture.view.setSelectedRange(singleRange1)
        fixture.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == "b")

        let multiRange1 = displayed.range(of: "bar")
        try #require(multiRange1.location != NSNotFound)
        fixture.view.setSelectedRange(multiRange1)
        fixture.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == "bar")

        let gammaRange = displayed.range(of: "gamma")
        try #require(gammaRange.location != NSNotFound)
        fixture.view.setSelectedRange(NSRange(location: gammaRange.location, length: 1))
        fixture.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == "g")

        fixture.view.setSelectedRange(gammaRange)
        fixture.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == "gamma")
        await fixture.close()
    }

    @Test func readingFinderPublishesSameGenerationSnapshotAfterFormulaUpdate() async throws {
        let fixture = try ReadingTests.Fixture("Intro $\\alpha$ tail\n")
        try await fixture.start(mathService: Self.mathService())
        let finder = fixture.view.ensureReadingFinder()
        #expect(finder.matches(for: "\\alpha").count == 1)

        let updated = "Intro $\\gamma + \\delta$ and\n\n$$\\int_0^1 x\\,dx$$\n\ntail\n"
        let oldLength = (fixture.document.text as NSString).length
        let prevCount = fixture.view.layoutRenderCount
        fixture.document.edited(range: NSRange(location: 0, length: oldLength), replacement: updated)
        fixture.coordinator.refresh(force: true)
        for _ in 0..<200 where fixture.view.layoutRenderCount == prevCount {
            try await Task.sleep(for: .milliseconds(10))
        }

        // 不先在主线程调 matches(for:)，直接从后台线程读取 NSTextFinderClient 快照，验证已同步发布最新同代文本。
        let backgroundSnapshot = await Task.detached { () -> String in
            var effective = NSRange(location: 0, length: 0)
            var boundary: ObjCBool = false
            return finder.string(at: 0, effectiveRange: &effective, endsWithSearchBoundary: &boundary)
        }.value
        #expect(backgroundSnapshot.contains("\\gamma + \\delta"))
        #expect(backgroundSnapshot.contains("\\int_0^1 x\\,dx"))
        #expect(!backgroundSnapshot.contains("\\alpha"))

        let display = fixture.view.string as NSString
        let inlineHits = finder.matches(for: "\\gamma + \\delta")
        #expect(inlineHits.count == 1 && inlineHits[0].isFormula)
        #expect(inlineHits[0].realRange.length == 1)
        #expect(display.character(at: inlineHits[0].realRange.location) == 0xFFFC)

        let blockHits = finder.matches(for: "\\int_0^1")
        #expect(blockHits.count == 1 && blockHits[0].isFormula)
        #expect(NSMaxRange(blockHits[0].realRange) <= display.length)
        await fixture.close()
    }

    @Test func tableCellStyledWikilinkDoesNotShiftSubsequentLinkCursor() async throws {
        let source = "| Links |\n| --- |\n| [[NoteA\\|**bold** text]] and [[NoteB]] |\n"
        let fixture = try ReadingTests.Fixture(source)
        try "# NoteA\n".write(to: fixture.editing.vault.appendingPathComponent("NoteA.md"), atomically: true, encoding: .utf8)
        try "# NoteB\n".write(to: fixture.editing.vault.appendingPathComponent("NoteB.md"), atomically: true, encoding: .utf8)
        let handle = try fixture.editing.core.openVault(path: fixture.editing.vault.path)
        handle.updateSearchIndex(upserted: ["note.md", "NoteA.md", "NoteB.md"], removed: [])
        defer { handle.close() }

        try await fixture.start(mathService: Self.mathService(), knowledgeHandle: handle)
        let media = try #require(fixture.view.mediaViews.first)
        let allLinks = media.inspectionTableCellLinks
        try #require(allLinks.count == 2)
        let bodyLinks = allLinks[1]
        #expect(bodyLinks.count >= 2)
        let noteALinks = bodyLinks.filter { $0.target == "NoteA" }
        let noteBLinks = bodyLinks.filter { $0.target == "NoteB" }
        #expect(!noteALinks.isEmpty)
        #expect(noteBLinks.count == 1)
        #expect(noteALinks.allSatisfy { !$0.broken })
        #expect(noteBLinks.allSatisfy { !$0.broken })
        await fixture.close()
    }

    @Test func embedRendererInvalidateFailedMediaRecoversMissingImage() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-embed-img-\(UUID().uuidString)")
        let vault = dir.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
        try "# Host\n\n![[child]]\n".write(to: vault.appendingPathComponent("host.md"), atomically: true, encoding: .utf8)
        try "# Child\n\n![[recovered.png]]\n".write(to: vault.appendingPathComponent("child.md"), atomically: true, encoding: .utf8)
        let core = try WorkspaceCore.inDirectories(config: dir.appendingPathComponent("config").path,
                                                   cache: dir.appendingPathComponent("cache").path)
        let handle = try core.openVault(path: vault.path)
        handle.updateSearchIndex(upserted: ["host.md", "child.md"], removed: [])
        defer { handle.close(); try? FileManager.default.removeItem(at: dir) }

        let renderer = EmbedRenderer()
        let request = EmbedRequest(source: "host.md", target: "child", heading: nil, alias: nil,
                                   ancestors: ["host.md"], depth: 0, handle: handle, vault: vault.path,
                                   width: 500, size: 15, dark: false, service: DiagramService(),
                                   mathService: Self.mathService(), knowledgeVersion: 1)
        let first = try #require(await renderer.prepare(request))
        #expect(first.media.first?.reason == "image.missing")

        // 外部补齐图片（不改 .md，knowledgeVersion 保持不变），清理失败缓存后重准备应恢复。
        try MediaTests.writeImage(vault.appendingPathComponent("recovered.png"), width: 48, height: 32)
        await LocalImageCache.shared.invalidateFailed()
        await renderer.invalidateFailedMedia()
        let recovered = try #require(await renderer.prepare(request))
        #expect(recovered.media.first?.reason == "")
        #expect(recovered.media.first?.imageSize == CGSize(width: 48, height: 32))
    }

    @Test func selectionPlainTextPreservesBodyPrivateUseCharacter() async throws {
        let source = "---\ntitle: Demo\n---\n\nBody with \u{e000} glyph.\n"
        let fixture = try ReadingTests.Fixture(source)
        try await fixture.start()
        fixture.view.setSelectedRange(NSRange(location: 0, length: fixture.view.sourceUTF16Length))
        let plain = try #require(fixture.view.selectionPlainText())
        #expect(plain.hasPrefix(L10n.string("Properties")))
        #expect(plain.contains("Body with \u{e000} glyph."))
        await fixture.close()

        let noProps = try ReadingTests.Fixture("\u{e000} leading glyph\n")
        try await noProps.start()
        noProps.view.setSelectedRange(NSRange(location: 0, length: noProps.view.sourceUTF16Length))
        let noPropsPlain = try #require(noProps.view.selectionPlainText())
        #expect(noPropsPlain.hasPrefix("\u{e000} leading glyph"))
        await noProps.close()
    }

    @Test func localImageCacheResolvesMixedMultiLevelNfcNfdAndRejectsOutsideVault() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-nfc-nfd-\(UUID().uuidString)")
        let vault = dir.appendingPathComponent("vault")
        let outside = dir.appendingPathComponent("outside")
        let nestedDir = vault
            .appendingPathComponent("cafe\u{0301}", isDirectory: true)
            .appendingPathComponent("re\u{0301}sume\u{0301}", isDirectory: true)
        try FileManager.default.createDirectory(at: nestedDir, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let imageURL = nestedDir.appendingPathComponent("na\u{00EF}ve.png")
        try MediaTests.writeImage(imageURL, width: 40, height: 20)
        let outsideImg = outside.appendingPathComponent("secret.png")
        try MediaTests.writeImage(outsideImg, width: 24, height: 24)
        let escapeLink = vault.appendingPathComponent("escape.png")
        try FileManager.default.createSymbolicLink(atPath: escapeLink.path, withDestinationPath: outsideImg.path)

        let cache = LocalImageCache()
        let mixedQuery = vault
            .appendingPathComponent("caf\u{00E9}", isDirectory: true)
            .appendingPathComponent("r\u{00E9}sum\u{00E9}", isDirectory: true)
            .appendingPathComponent("nai\u{0308}ve.png").path
        let decoded = await cache.decode(mixedQuery, vault: vault.path)
        #expect(decoded != nil)
        #expect(decoded?.width == 40)
        #expect(decoded?.height == 20)

        // 库外符号链接逃逸必须拒绝。
        #expect(await cache.decode(escapeLink.path, vault: vault.path) == nil)
        #expect(await cache.decode(outsideImg.path, vault: vault.path) == nil)
    }
}
