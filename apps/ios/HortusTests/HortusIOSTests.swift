import CoreGraphics
import CoreText
import HortusApple
import HortusCore
import UIKit
import XCTest
@testable import Hortus

@MainActor
final class HortusIOSTests: XCTestCase {
    func testDiagramEngineRendersAllSixMermaidTypes() async throws {
        let samples: [(String, String)] = [
            ("mindmap", "mindmap\n  root((Hortus))\n    Notes\n    Mermaid"),
            ("flowchart", "flowchart TD\n  A[Start] --> B[End]"),
            ("sequence", "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi"),
            ("state", "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Active"),
            ("class", "classDiagram\n  class Note {\n    +String title\n  }"),
            ("er", "erDiagram\n  VAULT ||--o{ NOTE : contains"),
        ]
        let service = DiagramService()
        for (kind, body) in samples {
            let source = "```mermaid\n\(body)\n```\n"
            let blocks = try await service.blocks(source: source, mmd: false, vault: "/tmp/test", note: "sample.md", core: nil)
            XCTAssertEqual(blocks.count, 1, "Expected 1 block for \(kind)")
            let rendered = await service.render(source: source, mmd: false, blocks: blocks, width: 360, dark: false)
            let prepared = try XCTUnwrap(rendered.first?.prepared, "Missing prepared scene for \(kind)")
            XCTAssertGreaterThan(prepared.scene.width, 0)
            XCTAssertGreaterThan(prepared.scene.height, 0)
            XCTAssertFalse(prepared.lines.isEmpty, "Expected text lines for \(kind)")
            for (span, line) in prepared.lines {
                let ctWidth = CTLineGetTypographicBounds(line, nil, nil, nil)
                XCTAssertLessThanOrEqual(abs(ctWidth - span.width), 0.5, "Glyph width drift for \(kind)")
            }
            let png = DiagramExport.renderPNG(prepared: prepared)
            XCTAssertNotNil(png, "PNG export failed for \(kind)")
            let pdf = DiagramExport.renderPDF(prepared: prepared)
            XCTAssertNotNil(pdf, "PDF export failed for \(kind)")
            let svg = try await DiagramExport.renderSVG(source: source, mmd: false, block: 1, width: 360, dark: false, collapsed: [])
            XCTAssertTrue(svg.contains("<svg"), "SVG export failed for \(kind)")
        }
    }

    func testMathServiceAndLatexCopySourceRestoration() async throws {
        let source = "Before $\\alpha + \\beta$ middle.\n\n$$\na^2 + b^2 = c^2\n$$\n\nAfter.\n"
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-math-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("vault", isDirectory: true)
        try FileManager.default.createDirectory(at: vaultURL, withIntermediateDirectories: true)
        let noteURL = vaultURL.appendingPathComponent("math.md")
        try source.write(to: noteURL, atomically: true, encoding: .utf8)

        let core = try makeSandboxCore(base: tempRoot)
        _ = try core.openVault(path: vaultURL.path)
        let session = try core.openDocument(vault: vaultURL.path, path: "math.md")
        let wire = try session.readingModelSince(since: UInt64.max)
        let model = try ReadingViewModel(wire)
        let mathBlocks = try session.mathBlocks()
        XCTAssertEqual(mathBlocks.count, 2)

        let prepared = await MathService.shared.prepare(mathBlocks, source: source, size: 16, width: 340, displayText: model.text)
        XCTAssertEqual(prepared.count, 2)
        XCTAssertNotNil(prepared[0].math)
        XCTAssertNotNil(prepared[1].math)

        let textView = HortusReadingUITextView()
        textView.attributedText = NSAttributedString(string: model.text)
        textView.mathCopyItems = prepared.map {
            (range: NSRange(location: Int($0.block.displayStart), length: Int($0.block.displayLength)),
             source: $0.block.displaySource)
        }
        textView.selectedRange = NSRange(location: 0, length: (model.text as NSString).length)
        let copied = try XCTUnwrap(textView.selectedPlainTextWithLatex())
        XCTAssertTrue(copied.contains("$\\alpha + \\beta$"), "Copied text must restore LaTeX source: \(copied)")
        XCTAssertTrue(copied.contains("a^2 + b^2 = c^2"), "Copied text must restore block LaTeX source: \(copied)")
        XCTAssertFalse(copied.contains("\u{FFFC}"))
    }

    func testDemoVaultRemainsUntouchedAndEditingSavesSafely() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-vault-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("vault", isDirectory: true)
        try FileManager.default.createDirectory(at: vaultURL, withIntermediateDirectories: true)
        let sampleURL = vaultURL.appendingPathComponent("Hello.md")
        try "# Hello\n\nLink to [[Second]].\n".write(to: sampleURL, atomically: true, encoding: .utf8)
        let secondURL = vaultURL.appendingPathComponent("Second.md")
        try "# Second\n\n#tag-one\n".write(to: secondURL, atomically: true, encoding: .utf8)

        let core = try makeSandboxCore(base: tempRoot)
        let handle = try core.openVault(path: vaultURL.path)

        let created = try handle.createNote(parent: "", name: "Created.md")
        XCTAssertEqual(created.path, "Created.md")

        let session = try core.openDocument(vault: vaultURL.path, path: "Created.md")
        let initial = try session.initial()
        let doc = IOSNoteDocument(io: DocumentIO(session: session, url: vaultURL.appendingPathComponent("Created.md")), initial: initial)
        doc.edited(range: NSRange(location: 0, length: 0), replacement: "# Created Note\n\nBody text.\n")
        await doc.saveNow()
        XCTAssertFalse(doc.dirty)
        let saved = try String(contentsOf: vaultURL.appendingPathComponent("Created.md"), encoding: .utf8)
        XCTAssertEqual(saved, "# Created Note\n\nBody text.\n")

        // Ensure no hidden app state files were written into the vault folder.
        let vaultContents = try FileManager.default.contentsOfDirectory(atPath: vaultURL.path).sorted()
        XCTAssertEqual(vaultContents, ["Created.md", "Hello.md", "Second.md"])
    }

    func testDemoVaultKnowledgeSearchOutlineBacklinksTagsAndLinks() async throws {
        let bundled = try XCTUnwrap(Bundle.main.url(forResource: "demo-vault", withExtension: nil))
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-demo-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("demo-vault", isDirectory: true)
        try FileManager.default.copyItem(at: bundled, to: vaultURL)
        let beforeFiles = try FileManager.default.subpathsOfDirectory(atPath: vaultURL.path).sorted()

        let core = try makeSandboxCore(base: tempRoot)
        let handle = try core.openVault(path: vaultURL.path)
        let sub = VaultSubscription(handle: handle) { batches in
            let upserted = batches.flatMap { $0.upserted.filter { !$0.directory }.map(\.path) }
            let removed = batches.flatMap(\.removed)
            handle.updateSearchIndex(upserted: upserted, removed: removed)
        }
        defer { sub.close() }

        try await Task.sleep(for: .milliseconds(350))
        let quick = handle.quickOpen(query: "Markdown", limit: 10)
        XCTAssertTrue(quick.contains(where: { $0.path.contains("Markdown field guide") }))

        let search = handle.search(query: "Mermaid", maxMatchesPerNote: 5, cancellation: SearchCancellation())
        XCTAssertFalse(search.isEmpty)

        let resolved = handle.resolveLink(sourcePath: "Start here.md", rawTarget: "Markdown field guide")
        XCTAssertEqual(resolved.resolvedPath, "Markdown field guide.md")

        let backlinks = handle.backlinks(path: "Markdown field guide.md")
        XCTAssertFalse(backlinks.isEmpty)

        let tags = handle.tags()
        XCTAssertFalse(tags.isEmpty)

        let session = try core.openDocument(vault: vaultURL.path, path: "Markdown field guide.md")
        let outline = try session.outline()
        XCTAssertFalse(outline.headings.isEmpty)

        let afterFiles = try FileManager.default.subpathsOfDirectory(atPath: vaultURL.path).sorted()
        XCTAssertEqual(beforeFiles, afterFiles, "Vault folder must remain untouched by read/index operations")
    }

    func testMarkdownFieldGuideReadingPipelineProducesNonEmptyLayout() async throws {
        let bundled = try XCTUnwrap(Bundle.main.url(forResource: "demo-vault", withExtension: nil))
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-guide-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("demo-vault", isDirectory: true)
        try FileManager.default.copyItem(at: bundled, to: vaultURL)

        let core = try makeSandboxCore(base: tempRoot)
        let handle = try core.openVault(path: vaultURL.path)
        let session = try core.openDocument(vault: vaultURL.path, path: "Markdown field guide.md")
        let initial = try session.initial()
        let io = DocumentIO(session: session, url: vaultURL.appendingPathComponent("Markdown field guide.md"))
        let doc = IOSNoteDocument(io: io, initial: initial)

        let view = HortusReadingUITextView()
        view.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        let repr = IOSReadingTextView(
            document: doc,
            path: "Markdown field guide.md",
            vault: vaultURL.path,
            fontSize: 16,
            diagramService: DiagramService(),
            core: core,
            vaultHandle: handle,
            knowledgeVersion: 1,
            refreshToken: 0,
            onOpenLink: { _, _ in },
            onShowTag: { _ in },
            onFocus: { _ in },
            onToggleDiagram: { _, _ in }
        )
        let coord = repr.makeCoordinator()
        coord.view = view
        coord.layoutDelegate.view = view
        coord.layoutDelegate.baseDelegate = view.textLayoutManager?.delegate
        view.textLayoutManager?.delegate = coord.layoutDelegate
        coord.update(repr)
        for _ in 0..<40 {
            if (view.attributedText?.length ?? 0) > 0 { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertNotNil(view.textLayoutManager, "Reading view must remain in TextKit 2 mode")
        XCTAssertGreaterThan(view.attributedText?.length ?? 0, 100, "Expected rendered attributedText for Markdown field guide")
        let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
        let window = scene.map { UIWindow(windowScene: $0) } ?? UIWindow(frame: CGRect(x: 0, y: 0, width: 390, height: 844))
        window.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        let hostVC = UIViewController()
        hostVC.view.addSubview(view)
        window.rootViewController = hostVC
        window.makeKeyAndVisible()
        view.layoutIfNeeded()
        try await Task.sleep(for: .milliseconds(150))
        let mountedNames = view.subviews.map { String(describing: type(of: $0)) }
        XCTAssertTrue(mountedNames.contains(where: { $0.contains("MediaBlockHostUIView") }), "Expected mounted table/media host view: \(mountedNames)")
        XCTAssertTrue(mountedNames.contains(where: { $0.contains("InlineMathHostUIView") }), "Expected mounted inline math view: \(mountedNames)")
        XCTAssertTrue(mountedNames.contains(where: { $0.contains("BlockMathHostUIView") }), "Expected mounted block math view: \(mountedNames)")
        XCTAssertTrue(mountedNames.contains(where: { $0.contains("InlineDiagramUIView") }), "Expected mounted diagram view: \(mountedNames)")
    }

    func testFocusModeViewportWidthAndFitScale() async throws {
        let bundled = try XCTUnwrap(Bundle.main.url(forResource: "demo-vault", withExtension: nil))
        let guideURL = bundled.appendingPathComponent("Markdown field guide.md")
        let source = try String(contentsOf: guideURL, encoding: .utf8)

        let iPhoneViewport = CGSize(width: 393, height: 852)
        let renderWidth = IOSDiagramFocusView.renderWidth(for: iPhoneViewport)
        XCTAssertEqual(renderWidth, 361.0, accuracy: 0.5)

        let service = DiagramService()
        let blocks = try await service.blocks(source: source, mmd: false, vault: bundled.path, note: "Markdown field guide.md", core: nil)
        XCTAssertEqual(blocks.count, 1)
        let rendered = await service.render(source: source, mmd: false, blocks: blocks, width: renderWidth, dark: false)
        let prepared = try XCTUnwrap(rendered.first?.prepared)
        XCTAssertLessThanOrEqual(prepared.scene.width, 400.0)

        let fit = IOSDiagramFocusView.fitMetrics(
            sceneSize: CGSize(width: prepared.scene.width, height: prepared.scene.height),
            viewportSize: iPhoneViewport
        )
        XCTAssertEqual(fit.scale, 1.0, accuracy: 0.05)
        XCTAssertGreaterThanOrEqual(fit.offset.width, 0)
        XCTAssertGreaterThanOrEqual(fit.offset.height, 0)
    }

    func testFocusModeExportSVGMatchesPreparedSceneCollapsedStateAndTheme() async throws {
        let source = """
        mindmap
          root((Hortus))
            Branch
              LeafA
              LeafB
            Other
        
        """
        let service = DiagramService()
        let width = IOSDiagramFocusView.renderWidth(for: CGSize(width: 430, height: 932))

        let expandedBlock = NoteDiagram(
            ordinal: 1,
            sourceStart: 0,
            range: NSRange(location: 0, length: (source as NSString).length),
            collapsed: [],
            documentIdentity: "focus-svg-expanded",
            revision: 1
        )
        let expandedRendered = await service.render(source: source, mmd: true, blocks: [expandedBlock], width: width, dark: false)
        let expandedPrepared = try XCTUnwrap(expandedRendered.first?.prepared, "render reason: \(expandedRendered.first?.reason ?? "nil")")
        let collapsibleId = try XCTUnwrap(expandedPrepared.scene.hitRegions.first(where: { $0.canCollapse })?.nodeId)

        let collapsedBlock = NoteDiagram(
            ordinal: 1,
            sourceStart: 0,
            range: NSRange(location: 0, length: (source as NSString).length),
            collapsed: [collapsibleId],
            documentIdentity: "focus-svg-collapsed",
            revision: 2
        )
        let collapsedLightRendered = await service.render(source: source, mmd: true, blocks: [collapsedBlock], width: width, dark: false)
        let collapsedLight = try XCTUnwrap(collapsedLightRendered.first?.prepared)

        let collapsedDarkRendered = await service.render(source: source, mmd: true, blocks: [collapsedBlock], width: width, dark: true)
        let collapsedDark = try XCTUnwrap(collapsedDarkRendered.first?.prepared)

        let expandedRequest = IOSDiagramFocusView.makeExportRequest(prepared: expandedPrepared, note: "Flow.mmd", ordinal: 1)
        let collapsedLightRequest = IOSDiagramFocusView.makeExportRequest(prepared: collapsedLight, note: "Flow.mmd", ordinal: 1)
        let collapsedDarkRequest = IOSDiagramFocusView.makeExportRequest(prepared: collapsedDark, note: "Flow.mmd", ordinal: 1)

        let expandedSVG = try await expandedRequest.svg()
        let collapsedLightSVG = try await collapsedLightRequest.svg()
        let collapsedDarkSVG = try await collapsedDarkRequest.svg()

        XCTAssertEqual(expandedSVG, try DiagramExport.renderSVG(prepared: expandedPrepared))
        XCTAssertEqual(collapsedLightSVG, try DiagramExport.renderSVG(prepared: collapsedLight))
        XCTAssertEqual(collapsedDarkSVG, try DiagramExport.renderSVG(prepared: collapsedDark))
        XCTAssertNotEqual(expandedSVG, collapsedLightSVG)
        XCTAssertNotEqual(collapsedLightSVG, collapsedDarkSVG)
        XCTAssertTrue(collapsedLightSVG.contains("viewBox=\"0 0 "))
    }

    func testStudyLoopEmbedAndMmdEmbedRendering() async throws {
        let bundled = try XCTUnwrap(Bundle.main.url(forResource: "demo-vault", withExtension: nil))
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-embed-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("demo-vault", isDirectory: true)
        try FileManager.default.copyItem(at: bundled, to: vaultURL)
        let mmdURL = vaultURL.appendingPathComponent("Learning/Flow.mmd")
        try "flowchart LR\n  A[Read] --> B[Recall]\n".write(to: mmdURL, atomically: true, encoding: .utf8)
        let hostURL = vaultURL.appendingPathComponent("Embed Host.md")
        try "# Embed Host\n\n![[Flow.mmd]]\n\n![[Missing Note]]\n".write(to: hostURL, atomically: true, encoding: .utf8)

        let core = try makeSandboxCore(base: tempRoot)
        let handle = try core.openVault(path: vaultURL.path)
        let sub = VaultSubscription(handle: handle) { batches in
            let upserted = batches.flatMap { $0.upserted.filter { !$0.directory }.map(\.path) }
            let removed = batches.flatMap(\.removed)
            handle.updateSearchIndex(upserted: upserted, removed: removed)
        }
        defer { sub.close() }
        try await Task.sleep(for: .milliseconds(350))

        // 1. Study loop full note embed (with nested Mermaid diagram + callout)
        let studyInfo = try handle.resolveEmbed(
            sourcePath: "Markdown field guide.md", rawTarget: "Study loop",
            heading: nil, alias: nil, ancestors: ["Markdown field guide.md"],
            maxDepth: 3, width: 320, size: 16
        )
        XCTAssertEqual(studyInfo.kind, "note")
        XCTAssertEqual(studyInfo.resolvedPath, "Learning/Study loop.md")
        XCTAssertFalse(studyInfo.diagrams.isEmpty, "Study loop embed must include its Mermaid mindmap")
        XCTAssertTrue(studyInfo.content.contains("Make recall concrete"))

        // 2. .mmd diagram embed resolution & rendering
        let mmdInfo = try handle.resolveEmbed(
            sourcePath: "Embed Host.md", rawTarget: "Flow.mmd",
            heading: nil, alias: nil, ancestors: ["Embed Host.md"],
            maxDepth: 3, width: 320, size: 16
        )
        XCTAssertEqual(mmdInfo.kind, "diagram")
        XCTAssertTrue(mmdInfo.isMmd)
        XCTAssertEqual(mmdInfo.resolvedPath, "Learning/Flow.mmd")
        let service = DiagramService()
        let mmdBlock = NoteDiagram(ordinal: 1, sourceStart: 0, range: NSRange(location: 0, length: (mmdInfo.content as NSString).length), collapsed: [])
        let renderedMmd = await service.render(source: mmdInfo.content, mmd: true, blocks: [mmdBlock], width: 320, dark: false)
        let preparedMmd = try XCTUnwrap(renderedMmd.first?.prepared)
        XCTAssertLessThanOrEqual(preparedMmd.scene.width, 325)

        // 3. Boundary embed states
        let brokenInfo = try handle.resolveEmbed(
            sourcePath: "Embed Host.md", rawTarget: "Missing Note",
            heading: nil, alias: nil, ancestors: ["Embed Host.md"],
            maxDepth: 3, width: 320, size: 16
        )
        XCTAssertEqual(brokenInfo.kind, "broken")

        // 4. Live TextKit 2 host view renders full Study loop card (> 112pt) with inner Mermaid view
        let session = try core.openDocument(vault: vaultURL.path, path: "Markdown field guide.md")
        let initial = try session.initial()
        let io = DocumentIO(session: session, url: vaultURL.appendingPathComponent("Markdown field guide.md"))
        let doc = IOSNoteDocument(io: io, initial: initial)

        let view = HortusReadingUITextView()
        view.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        let repr = IOSReadingTextView(
            document: doc,
            path: "Markdown field guide.md",
            vault: vaultURL.path,
            fontSize: 16,
            diagramService: service,
            core: core,
            vaultHandle: handle,
            knowledgeVersion: 1,
            refreshToken: 0,
            onOpenLink: { _, _ in },
            onShowTag: { _ in },
            onFocus: { _ in },
            onToggleDiagram: { _, _ in }
        )
        let coord = repr.makeCoordinator()
        coord.view = view
        coord.layoutDelegate.view = view
        coord.layoutDelegate.baseDelegate = view.textLayoutManager?.delegate
        view.textLayoutManager?.delegate = coord.layoutDelegate
        coord.update(repr)

        let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
        let window = scene.map { UIWindow(windowScene: $0) } ?? UIWindow(frame: CGRect(x: 0, y: 0, width: 390, height: 844))
        window.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        let hostVC = UIViewController()
        hostVC.view.addSubview(view)
        window.rootViewController = hostVC
        window.makeKeyAndVisible()

        var studyCard: MediaBlockHostUIView?
        for _ in 0..<60 {
            view.layoutIfNeeded()
            if let card = view.subviews.compactMap({ $0 as? MediaBlockHostUIView }).first(where: { $0.embedKind == "note" }),
               card.frame.height > 150 {
                studyCard = card
                break
            }
            try await Task.sleep(for: .milliseconds(50))
        }
        let resolvedCard = try XCTUnwrap(studyCard, "Expected Study loop embed card to resolve and expand beyond 112pt")
        XCTAssertEqual(resolvedCard.resolvedEmbedPath, "Learning/Study loop.md")
        XCTAssertGreaterThan(resolvedCard.frame.height, 150)

        // Verify inner embedded reading text view mounted Study loop's Mermaid diagram and full callout content
        let innerReading = resolvedCard.subviews.flatMap { [$0] + $0.subviews }.compactMap { $0 as? HortusReadingUITextView }.first
        let innerTextView = try XCTUnwrap(innerReading, "Expected inner HortusReadingUITextView inside Study loop card")
        innerTextView.layoutIfNeeded()
        let innerText = innerTextView.attributedText?.string ?? ""
        XCTAssertTrue(innerText.contains("Make recall concrete"), "Expected Study loop callout inside embed card: \(innerText)")
        XCTAssertTrue(innerText.contains("Content: CC0"), "Expected complete Study loop note inside embed card: \(innerText)")
        let innerTypes = innerTextView.subviews.map { String(describing: type(of: $0)) }
        XCTAssertTrue(innerTypes.contains(where: { $0.contains("InlineDiagramUIView") }), "Expected Study loop inner Mermaid diagram view: \(innerTypes)")
    }

    func testSameNoteHeadingAnchorJump() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-anchor-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("vault", isDirectory: true)
        try FileManager.default.createDirectory(at: vaultURL, withIntermediateDirectories: true)
        let noteText = "# Top\n\nSee [[#Target Section]].\n\n## Target Section\n\nDetails here.\n"
        try noteText.write(to: vaultURL.appendingPathComponent("Anchor.md"), atomically: true, encoding: .utf8)

        let appModel = IOSAppModel()
        await appModel.openVault(url: vaultURL, securityScoped: false)
        appModel.openNote("Anchor.md")
        for _ in 0..<40 {
            if appModel.currentDocument != nil { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        let doc = try XCTUnwrap(appModel.currentDocument)
        doc.pendingReveal = nil
        let previousToken = appModel.refreshToken

        appModel.openLink(target: "", heading: "Target Section")
        XCTAssertNil(appModel.errorMessage)
        XCTAssertEqual(appModel.currentPath, "Anchor.md")
        XCTAssertEqual(appModel.refreshToken, previousToken + 1)
        let reveal = try XCTUnwrap(doc.pendingReveal)
        let ns = noteText as NSString
        let targetRange = ns.range(of: "## Target Section")
        XCTAssertGreaterThanOrEqual(reveal.source, targetRange.location)
    }

    func testM4_01_VaultSwitchAndBackgroundFlushSavesAndWritesRecovery() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-m401-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vault1 = tempRoot.appendingPathComponent("vault1", isDirectory: true)
        let vault2 = tempRoot.appendingPathComponent("vault2", isDirectory: true)
        try FileManager.default.createDirectory(at: vault1, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: vault2, withIntermediateDirectories: true)
        let note1URL = vault1.appendingPathComponent("Note1.md")
        let note2URL = vault2.appendingPathComponent("Note2.md")
        try "# Note 1\n".write(to: note1URL, atomically: true, encoding: .utf8)
        try "# Note 2\n".write(to: note2URL, atomically: true, encoding: .utf8)

        let appModel = IOSAppModel()
        await appModel.openVault(url: vault1, securityScoped: false)
        appModel.openNote("Note1.md", editing: true)
        for _ in 0..<40 {
            if appModel.currentDocument != nil { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        let doc1 = try XCTUnwrap(appModel.currentDocument)
        doc1.edited(range: NSRange(location: (doc1.text as NSString).length, length: 0), replacement: "\nUnsaved edit before vault switch.\n")
        XCTAssertTrue(doc1.dirty)

        // Switching vaults must flush dirty edits before closing vault1.
        await appModel.openVault(url: vault2, securityScoped: false)
        let saved1 = try String(contentsOf: note1URL, encoding: .utf8)
        XCTAssertTrue(saved1.contains("Unsaved edit before vault switch."), "Expected vault switch to save dirty note: \(saved1)")

        appModel.openNote("Note2.md", editing: true)
        for _ in 0..<40 {
            if appModel.currentPath == "Note2.md" { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        let doc2 = try XCTUnwrap(appModel.currentDocument)
        doc2.edited(range: NSRange(location: (doc2.text as NSString).length, length: 0), replacement: "Draft while marked")
        doc2.compositionChanged(true)
        let persistence = await appModel.flushDocumentForBackground(doc2).value
        XCTAssertEqual(persistence, .recoveryDraft)
        await appModel.closeCurrentVault()

        // Reopen Note2 via core and confirm recovery draft exists.
        let core = try XCTUnwrap(appModel.core)
        let session2 = try core.openDocument(vault: vault2.path, path: "Note2.md")
        let reopened = try session2.initial()
        XCTAssertEqual(reopened.issue, .recovery)
        let restored = try session2.recover(restore: true)
        XCTAssertTrue((restored.text ?? "").contains("Draft while marked"))
    }

    func testM4_02_ClearCachesPreservesRecoveryDirectory() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-m402-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let supportDir = tempRoot.appendingPathComponent("support", isDirectory: true)
        let cacheDir = tempRoot.appendingPathComponent("cache", isDirectory: true)
        let recoveryDir = cacheDir.appendingPathComponent("recovery", isDirectory: true)
        try FileManager.default.createDirectory(at: supportDir, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: recoveryDir, withIntermediateDirectories: true)

        let sessionFile = cacheDir.appendingPathComponent("session.toml")
        let recoveryFile = recoveryDir.appendingPathComponent("abcd1234.json")
        let tempCacheFile = cacheDir.appendingPathComponent("preview-cache.bin")
        try "recent = []\n".write(to: sessionFile, atomically: true, encoding: .utf8)
        try "{\"text\":\"unsaved\"}\n".write(to: recoveryFile, atomically: true, encoding: .utf8)
        try Data([1, 2, 3, 4]).write(to: tempCacheFile)

        let settings = IOSSettingsStore(supportDirectory: supportDir, cacheDirectory: cacheDir)
        await settings.clearCaches(diagramService: DiagramService())

        XCTAssertTrue(settings.cacheClearedNotice)
        XCTAssertTrue(FileManager.default.fileExists(atPath: sessionFile.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: recoveryFile.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: tempCacheFile.path))
    }

    func testM4_03_04_IMECommitReconciliationAndToggleReadingModeSave() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-m403-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("vault", isDirectory: true)
        try FileManager.default.createDirectory(at: vaultURL, withIntermediateDirectories: true)
        let noteURL = vaultURL.appendingPathComponent("IME.md")
        try "Hello ".write(to: noteURL, atomically: true, encoding: .utf8)

        let appModel = IOSAppModel()
        await appModel.openVault(url: vaultURL, securityScoped: false)
        appModel.openNote("IME.md", editing: true)
        for _ in 0..<40 {
            if appModel.currentDocument != nil { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        let doc = try XCTUnwrap(appModel.currentDocument)
        XCTAssertFalse(doc.reading)

        let editRepr = IOSEditingTextView(document: doc, fontSize: 16)
        let coord = editRepr.makeCoordinator()
        let textView = UITextView()
        textView.delegate = coord
        coord.view = textView
        coord.update(editRepr, view: textView)

        let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
        let window = scene.map { UIWindow(windowScene: $0) } ?? UIWindow(frame: CGRect(x: 0, y: 0, width: 390, height: 844))
        let hostVC = UIViewController()
        hostVC.view.addSubview(textView)
        window.rootViewController = hostVC
        window.makeKeyAndVisible()
        textView.becomeFirstResponder()

        // Simulate IME marked text insertion skipped by shouldChangeTextIn, then candidate commit.
        textView.selectedRange = NSRange(location: (textView.text as NSString).length, length: 0)
        textView.setMarkedText("nihao", selectedRange: NSRange(location: 5, length: 0))
        coord.textViewDidChange(textView)
        XCTAssertTrue(doc.marked)

        textView.unmarkText()
        let range = (textView.text as NSString).range(of: "nihao")
        textView.textStorage.replaceCharacters(in: range, with: "你好")
        coord.textViewDidChange(textView)
        XCTAssertFalse(doc.marked)
        XCTAssertEqual(doc.text, "Hello 你好")

        await doc.saveNow()
        let savedFirst = try String(contentsOf: noteURL, encoding: .utf8)
        XCTAssertEqual(savedFirst, "Hello 你好")

        // Simulate leaving editing mode while still composing marked text.
        textView.selectedRange = NSRange(location: (textView.text as NSString).length, length: 0)
        textView.setMarkedText("世界", selectedRange: NSRange(location: 2, length: 0))
        coord.textViewDidChange(textView)
        XCTAssertTrue(doc.marked)

        await appModel.toggleReadingMode()
        XCTAssertTrue(doc.reading)
        XCTAssertFalse(doc.marked, "toggleReadingMode must clear marked state")
        XCTAssertFalse(doc.dirty, "toggleReadingMode must wait for saveNow to finish")
        let savedSecond = try String(contentsOf: noteURL, encoding: .utf8)
        XCTAssertEqual(savedSecond, "Hello 你好世界")
    }

    func testM4_05_TableInlineMathDrawsNonEmptyPixelsInLightAndDark() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-m405-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("vault", isDirectory: true)
        try FileManager.default.createDirectory(at: vaultURL, withIntermediateDirectories: true)
        let source = "| Formula |\n| --- |\n| $\\alpha + \\beta$ |\n"
        try source.write(to: vaultURL.appendingPathComponent("TableMath.md"), atomically: true, encoding: .utf8)

        let core = try makeSandboxCore(base: tempRoot)
        _ = try core.openVault(path: vaultURL.path)
        let session = try core.openDocument(vault: vaultURL.path, path: "TableMath.md")
        let media = try session.mediaBlocks(width: 340, size: 16)
        let prepared = await LocalImageCache.shared.prepare(media, vault: vaultURL.path, size: 16, mathService: MathService.shared)
        let tableMedia = try XCTUnwrap(prepared.first(where: { !$0.block.columns.isEmpty }))
        XCTAssertFalse(tableMedia.math.flatMap { $0 }.isEmpty, "Expected prepared math in table cells")

        let measuredWithMath = TableLayoutMetrics.measure(block: tableMedia.block, size: 16, math: tableMedia.math)
        let measuredBlank = TableLayoutMetrics.measure(block: tableMedia.block, size: 16, math: [])
        let viewWithMath = TableCanvasUIView(measured: measuredWithMath)
        let viewBlank = TableCanvasUIView(measured: measuredBlank)

        func renderPNGData(view: UIView, style: UIUserInterfaceStyle) -> Data {
            let format = UIGraphicsImageRendererFormat()
            format.scale = 2
            format.opaque = false
            let renderer = UIGraphicsImageRenderer(size: view.bounds.size, format: format)
            return renderer.image { _ in
                UITraitCollection(userInterfaceStyle: style).performAsCurrent {
                    view.drawHierarchy(in: view.bounds, afterScreenUpdates: false)
                    // Draw directly into the current context so traitCollection applies cleanly.
                }
            }.pngData() ?? Data()
        }

        func drawPixels(view: UIView, style: UIUserInterfaceStyle) -> [UInt8] {
            let width = max(1, Int(ceil(view.bounds.width)))
            let height = max(1, Int(ceil(view.bounds.height)))
            var bytes = [UInt8](repeating: 0, count: width * height * 4)
            let space = CGColorSpaceCreateDeviceRGB()
            guard let ctx = CGContext(data: &bytes, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                                      space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
                return bytes
            }
            ctx.translateBy(x: 0, y: CGFloat(height))
            ctx.scaleBy(x: 1, y: -1)
            UIGraphicsPushContext(ctx)
            UITraitCollection(userInterfaceStyle: style).performAsCurrent {
                view.draw(view.bounds)
            }
            UIGraphicsPopContext()
            return bytes
        }

        _ = renderPNGData(view: viewWithMath, style: .light)
        let lightMathPixels = drawPixels(view: viewWithMath, style: .light)
        let lightBlankPixels = drawPixels(view: viewBlank, style: .light)
        let darkMathPixels = drawPixels(view: viewWithMath, style: .dark)

        let row0Height = Int(measuredWithMath.heights.first ?? 0)
        let rowBytes = max(1, Int(ceil(viewWithMath.bounds.width))) * 4
        let bodyStart = row0Height * rowBytes
        let mathAlphaCount = stride(from: bodyStart + 3, to: lightMathPixels.count, by: 4).filter { lightMathPixels[$0] > 0 }.count
        let blankAlphaCount = stride(from: bodyStart + 3, to: lightBlankPixels.count, by: 4).filter { lightBlankPixels[$0] > 0 }.count
        XCTAssertGreaterThan(mathAlphaCount, blankAlphaCount + 20, "Expected table inline math cell to draw non-empty formula pixels")
        XCTAssertNotEqual(lightMathPixels, darkMathPixels, "Expected table inline math colors to adapt between light and dark traits")
    }

    func testM4_06_ReadingViewDarkLightRefreshUpdatesDiagrams() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-m406-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("vault", isDirectory: true)
        try FileManager.default.createDirectory(at: vaultURL, withIntermediateDirectories: true)
        let source = "# Diagram\n\n```mermaid\nflowchart TD\n  A[Start] --> B[End]\n```\n"
        try source.write(to: vaultURL.appendingPathComponent("Theme.md"), atomically: true, encoding: .utf8)

        let core = try makeSandboxCore(base: tempRoot)
        let handle = try core.openVault(path: vaultURL.path)
        let session = try core.openDocument(vault: vaultURL.path, path: "Theme.md")
        let initial = try session.initial()
        let doc = IOSNoteDocument(io: DocumentIO(session: session, url: vaultURL.appendingPathComponent("Theme.md")), initial: initial)

        let view = HortusReadingUITextView()
        view.frame = CGRect(x: 0, y: 0, width: 390, height: 600)
        view.overrideUserInterfaceStyle = .light
        let repr = IOSReadingTextView(
            document: doc,
            path: "Theme.md",
            vault: vaultURL.path,
            fontSize: 16,
            diagramService: DiagramService(),
            core: core,
            vaultHandle: handle,
            knowledgeVersion: 1,
            refreshToken: 0,
            onOpenLink: { _, _ in },
            onShowTag: { _ in },
            onFocus: { _ in },
            onToggleDiagram: { _, _ in }
        )
        let coord = repr.makeCoordinator()
        coord.view = view
        coord.layoutDelegate.view = view
        coord.layoutDelegate.baseDelegate = view.textLayoutManager?.delegate
        view.textLayoutManager?.delegate = coord.layoutDelegate
        view.onAppearanceChanged = { [weak coord] dark in
            coord?.handleAppearanceChange(dark: dark)
        }
        coord.update(repr, dark: false)

        for _ in 0..<40 {
            if (view.attributedText?.length ?? 0) > 0 { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        var lightAttachment: DiagramBlockAttachment?
        view.attributedText?.enumerateAttribute(.attachment, in: NSRange(location: 0, length: view.attributedText?.length ?? 0)) { value, _, _ in
            if let att = value as? DiagramBlockAttachment { lightAttachment = att }
        }
        XCTAssertEqual(try XCTUnwrap(lightAttachment).diagram.prepared?.scene.identity.dark, false)

        coord.update(repr, dark: true)
        var darkAttachment: DiagramBlockAttachment?
        for _ in 0..<40 {
            view.attributedText?.enumerateAttribute(.attachment, in: NSRange(location: 0, length: view.attributedText?.length ?? 0)) { value, _, _ in
                if let att = value as? DiagramBlockAttachment, att.diagram.prepared?.scene.identity.dark == true {
                    darkAttachment = att
                }
            }
            if darkAttachment != nil { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertEqual(darkAttachment?.diagram.prepared?.scene.identity.dark, true)
    }

    func testM4_07_OpenNoteGenerationPreventsStaleOverwrite() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-m407-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("vault", isDirectory: true)
        try FileManager.default.createDirectory(at: vaultURL, withIntermediateDirectories: true)
        try "# Note A\n".write(to: vaultURL.appendingPathComponent("A.md"), atomically: true, encoding: .utf8)
        try "# Note B\n".write(to: vaultURL.appendingPathComponent("B.md"), atomically: true, encoding: .utf8)

        let appModel = IOSAppModel()
        await appModel.openVault(url: vaultURL, securityScoped: false)
        appModel.beforeOpenDocumentForTesting = { path in
            if path == "A.md" {
                try? await Task.sleep(for: .milliseconds(160))
            }
        }
        appModel.openNote("A.md")
        appModel.openNote("B.md")
        try await Task.sleep(for: .milliseconds(250))

        XCTAssertEqual(appModel.currentPath, "B.md")
        XCTAssertEqual(appModel.currentDocument?.text, "# Note B\n")
        await appModel.closeCurrentVault()
    }

    func testM4_08_ConflictCopyWhenOriginalDeletedNavigatesToCopy() async throws {
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-m408-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }

        let vaultURL = tempRoot.appendingPathComponent("vault", isDirectory: true)
        try FileManager.default.createDirectory(at: vaultURL, withIntermediateDirectories: true)
        let originalURL = vaultURL.appendingPathComponent("Original.md")
        try "# Original\n".write(to: originalURL, atomically: true, encoding: .utf8)

        let appModel = IOSAppModel()
        await appModel.openVault(url: vaultURL, securityScoped: false)
        appModel.openNote("Original.md", editing: true)
        for _ in 0..<40 {
            if appModel.currentDocument != nil { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        let doc = try XCTUnwrap(appModel.currentDocument)
        doc.edited(range: NSRange(location: (doc.text as NSString).length, length: 0), replacement: "\nRecovered edit.\n")

        try FileManager.default.removeItem(at: originalURL)
        await doc.saveNow()
        XCTAssertEqual(doc.issue, .missing)

        await doc.saveNow(.conflictCopy)
        for _ in 0..<40 {
            if appModel.currentPath != "Original.md" { break }
            try await Task.sleep(for: .milliseconds(50))
        }

        XCTAssertEqual(appModel.currentPath, "Original (conflict copy).md")
        XCTAssertEqual(appModel.currentDocument?.issue, DocumentIssue.none)
        let copyText = try String(contentsOf: vaultURL.appendingPathComponent("Original (conflict copy).md"), encoding: .utf8)
        XCTAssertEqual(copyText, "# Original\n\nRecovered edit.\n")
        await appModel.closeCurrentVault()
    }

    func testM4_09_BoundedDiagramLRUAndImageMemoryWarningAndScrollSync() async throws {
        // 1. DiagramService keeps up to 8 documents without cross-eviction between main & embed.
        let service = DiagramService()
        for i in 0..<10 {
            let source = "```mermaid\nflowchart LR\n  A\(i) --> B\(i)\n```\n"
            let blocks = try await service.blocks(source: source, mmd: false, vault: "/tmp/test", note: "doc\(i).md", core: nil, documentIdentity: "doc-\(i)")
            let rendered = await service.render(source: source, mmd: false, blocks: blocks, width: 320, dark: false)
            XCTAssertNotNil(rendered.first?.prepared)
        }
        let cachedCount = await service.cachedDocumentCount
        XCTAssertEqual(cachedCount, 8)
        let mainLines = await service.cachedLineCount(for: "doc-9")
        let embedLines = await service.cachedLineCount(for: "doc-8")
        XCTAssertGreaterThan(mainLines, 0)
        XCTAssertGreaterThan(embedLines, 0)

        // 2. LocalImageCache evicts pixel buffers upon memory warning notification.
        let tempRoot = FileManager.default.temporaryDirectory.appendingPathComponent("hortus-ios-m409-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempRoot) }
        let imgURL = tempRoot.appendingPathComponent("pixel.png")
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: 16, height: 16))
        let png = renderer.pngData { ctx in
            UIColor.systemBlue.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: 16, height: 16))
        }
        try png.write(to: imgURL)
        let image = await LocalImageCache.shared.decode(imgURL.path, vault: tempRoot.path)
        XCTAssertNotNil(image)
        let bytesBefore = await LocalImageCache.shared.cachedPixelBytes
        XCTAssertGreaterThan(bytesBefore, 0)

        NotificationCenter.default.post(name: UIApplication.didReceiveMemoryWarningNotification, object: nil)
        for _ in 0..<40 {
            if await LocalImageCache.shared.cachedPixelBytes == 0 { break }
            try await Task.sleep(for: .milliseconds(25))
        }
        let bytesAfter = await LocalImageCache.shared.cachedPixelBytes
        XCTAssertEqual(bytesAfter, 0)

        // 3. Scroll offset changes do not re-sync attachments when width & text are unchanged.
        let view = HortusReadingUITextView()
        view.frame = CGRect(x: 0, y: 0, width: 320, height: 400)
        let attr = NSMutableAttributedString(string: "Hello ")
        let att = HortusBaseAttachment()
        att.bounds = CGRect(x: 0, y: 0, width: 20, height: 20)
        attr.append(NSAttributedString(attachment: att))
        view.attributedText = attr
        let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
        let window = scene.map { UIWindow(windowScene: $0) } ?? UIWindow(frame: CGRect(x: 0, y: 0, width: 320, height: 400))
        let hostVC = UIViewController()
        hostVC.view.addSubview(view)
        window.rootViewController = hostVC
        window.makeKeyAndVisible()
        view.layoutIfNeeded()
        XCTAssertEqual(view.attachmentSyncCount, 1)

        view.bounds.origin.y = 120
        view.layoutSubviews()
        XCTAssertEqual(view.attachmentSyncCount, 1)
    }

    private func makeSandboxCore(base: URL) throws -> WorkspaceCore {
        let config = base.appendingPathComponent("config", isDirectory: true)
        let cache = base.appendingPathComponent("cache", isDirectory: true)
        let data = base.appendingPathComponent("data", isDirectory: true)
        try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: cache, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: data, withIntermediateDirectories: true)
        setenv("HORTUS_DATA_DIR", data.path, 1)
        return try WorkspaceCore.inDirectories(config: config.path, cache: cache.path)
    }
}
