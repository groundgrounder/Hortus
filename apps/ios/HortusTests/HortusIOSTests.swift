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
        appModel.openVault(url: vaultURL, securityScoped: false)
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
