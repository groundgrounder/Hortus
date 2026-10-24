import AppKit
import CoreText
import HortusCore
import Testing
@testable import Hortus

@Suite @MainActor
struct DiagramTests {
    private var root: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../..").standardizedFileURL
    }

    @Test func coreTextWidthsMatchEveryPublicAndAvailablePrivateSpan() async throws {
        let root = root
        let result = try await Task.detached {
            let engine = try DiagramEngine()
            let manager = FileManager.default
            var files = try manager.contentsOfDirectory(at: root.appendingPathComponent("fixtures/mermaid/mindmap"), includingPropertiesForKeys: nil)
            let vault = root.appendingPathComponent("fixtures/vault-sample")
            let enumerator = manager.enumerator(at: vault, includingPropertiesForKeys: nil)!
            while let file = enumerator.nextObject() as? URL { if ["md", "markdown", "mmd"].contains(file.pathExtension) { files.append(file) } }
            let privateURL = root.appendingPathComponent("fixtures/mermaid/tree")
            if manager.fileExists(atPath: privateURL.path) {
                files += try manager.contentsOfDirectory(at: privateURL, includingPropertiesForKeys: nil).filter { $0.pathExtension == "md" }
            } else { print("私人样本缺失：跳过私人字宽检查") }
            var errors: [Double] = []
            for file in files {
                let source = try String(contentsOf: file, encoding: .utf8)
                let mmd = file.pathExtension == "mmd"
                for width in [380.0, 700, 1100] {
                    for dark in [false, true] {
                        let count = mmd ? 1 : diagramBlocks(note: source).count
                        for block in 0..<count {
                            let scene = try engine.render(source: source, mmd: mmd, block: UInt32(block + 1), width: width, dark: dark, collapsed: [])
                            let prepared = PreparedScene(scene: DiagramScene(scene))
                            errors += prepared.lines.map { abs(CTLineGetTypographicBounds($0.1, nil, nil, nil) - $0.0.width) }
                        }
                    }
                }
            }
            errors.sort()
            return (errors.count, errors.last ?? 0, errors.isEmpty ? 0 : errors[Int(Double(errors.count - 1) * 0.95)])
        }.value
        print("字宽：spans=\(result.0) max=\(result.1) p95=\(result.2)")
        #expect(result.0 > 100)
        #expect(result.1 <= 0.5)
    }

    @Test func sourcePreservingTextKitAndAccessibility() async throws {
        let source = "开头😀\n\n```mermaid\nflowchart TB\nR[Root] --> A[Branch]\nA --> B[Leaf]\n```\n\n结尾\n"
        let service = DiagramService()
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 700, height: 500))
        let view = DiagramTextView(diagramLayout: true)
        view.frame = scroll.bounds
        view.textContainerInset = NSSize(width: 30, height: 24)
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = true
        view.isEditable = false
        view.isSelectable = true
        scroll.documentView = view
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = scroll
        window.orderFront(nil)
        defer { window.close() }
        let coordinator = NoteText.Coordinator()
        coordinator.view = view
        view.textLayoutManager?.delegate = coordinator
        var toggled = false
        let input = NoteText(text: source, path: "note.md", vault: "test", fontSize: 15,
                                 service: service, core: nil) { _, nodes in toggled = !nodes.isEmpty }
        coordinator.update(input)
        try await wait(view)
        #expect(view.string == source)
        let pasteboard = NSPasteboard.withUniqueName()
        window.makeFirstResponder(view)
        pasteboard.declareTypes(view.writablePasteboardTypes, owner: nil)
        view.setSelectedRange(NSRange(location: 0, length: (source as NSString).length))
        #expect(view.writeSelection(to: pasteboard, types: view.writablePasteboardTypes))
        #expect(pasteboard.string(forType: .string) == source)
        pasteboard.releaseGlobally()
        #expect(view.textLayoutManager != nil)
        #expect(view.diagramViews.count == 1)
        let diagram = try #require(view.diagramViews[1])
        #expect(abs(diagram.frame.width - 640) < 0.1)
        #expect(diagram.frame.minY > 24)
        let nodes = try #require(diagram.accessibilityChildren() as? [NSAccessibilityElement])
        #expect(nodes.count == diagram.prepared.scene.hitRegions.count)
        #expect(nodes.map { $0.accessibilityLabel() } == diagram.prepared.scene.hitRegions.map(\.accessibilityLabel))
        #expect(nodes[0].isAccessibilityExpanded())
        #expect(nodes[0].accessibilityRole() == .disclosureTriangle)
        #expect(nodes[0].accessibilityPerformPress())
        try await wait(view)
        #expect(toggled)
        let folded = try #require(view.diagramViews[1])
        #expect(folded.prepared.scene.hitRegions.count == 1)
        #expect(folded.prepared.lines.map { $0.0.text }.joined().contains("+1"))
        #expect((folded.accessibilityChildren()?.first as? NSAccessibilityElement)?.isAccessibilityExpanded() == false)
        #expect((folded.accessibilityChildren()?.first as? NSAccessibilityElement) === nodes[0])
        #expect(view.string == source)
        view.layoutSubtreeIfNeeded()
        var followingY = 0.0
        let manager = try #require(view.textLayoutManager)
        let content = try #require(manager.textContentManager)
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: []) { fragment in
            if content.offset(from: content.documentRange.location, to: fragment.rangeInElement.location) >= NSMaxRange(view.blocks[0].range) {
                followingY = max(followingY, fragment.layoutFragmentFrame.minY)
            }
            return true
        }
        #expect(followingY >= folded.frame.maxY - view.textContainerInset.height)
    }

    @Test func offscreenDiagramAppearsAfterNativeScrollAndSourceCanBeCleared() async throws {
        let source = "```mermaid\nmindmap\n  Root\n    First\n```\n" + String(repeating: "普通段落测试\n", count: 120)
            + "```mermaid\nflowchart TB\nA --> B\n```\n结尾\n"
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 700, height: 400))
        let view = DiagramTextView(diagramLayout: true)
        view.frame = scroll.bounds
        view.textContainer?.widthTracksTextView = true
        view.isVerticallyResizable = true
        scroll.documentView = view
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = scroll
        window.orderFront(nil)
        defer { window.close() }
        let coordinator = NoteText.Coordinator()
        coordinator.view = view
        view.textLayoutManager?.delegate = coordinator
        let service = DiagramService()
        func input(_ source: String) -> NoteText {
            NoteText(text: source, path: "scroll.md", vault: "test", fontSize: 15, service: service, core: nil) { _, _ in }
        }
        coordinator.update(input(source))
        try await wait(view)
        let second = try #require(view.diagramViews[2])
        #expect(second.isHidden || second.frame.minY > 400)
        view.scrollRangeToVisible(NSRange(location: view.blocks[1].range.location, length: 1))
        try await Task.sleep(for: .milliseconds(40))
        view.layoutSubtreeIfNeeded()
        #expect(!second.isHidden)
        #expect(second.frame.minY > 400)
        #expect(second.frame.intersects(view.visibleRect))
        #expect(view.string == source)
        coordinator.update(input(""))
        try await wait(view)
        #expect(view.string.isEmpty)
        #expect(view.diagramViews.isEmpty)
    }

    @Test func projectedElementsPreserveTheEnumerationEdgesInBothDirections() throws {
        let view = DiagramTextView(diagramLayout: true)
        view.string = "flowchart TB\nA --> B\nB --> C\n"
        let storage = try #require(view.diagramStorage)
        storage.diagrams = [NSRange(location: 0, length: (view.string as NSString).length)]
        var forward = 0
        let end = storage.enumerateTextElements(from: nil, options: []) { _ in forward += 1; return true }
        #expect(forward == 1)
        #expect(end?.compare(storage.documentRange.endLocation) == .orderedSame)
        var backward = 0
        let start = storage.enumerateTextElements(from: nil, options: [.reverse]) { _ in backward += 1; return true }
        #expect(backward == 1)
        #expect(start?.compare(storage.documentRange.location) == .orderedSame)
    }

    @Test func errorsKeepSourceAndDescribeAllThreeReasons() async throws {
        let service = DiagramService()
        for (source, reason) in [("sequenceDiagram\nA->>B: Hi", L10n.string("Unsupported %@ diagram; showing source", "sequenceDiagram")),
                                 ("flowchart TB\nA --> C\nB --> C", L10n.string("Not a tree; showing source")),
                                 ("flowchart TB\nA --> B\nB ??? C", L10n.string("Syntax error on line %d; showing source", 3))] {
            let blocks = try await service.blocks(source: source, mmd: true, vault: "test", note: "x.mmd", core: nil)
            let result = await service.render(source: source, mmd: true, blocks: blocks, width: 700, dark: false)
            #expect(result[0].prepared == nil)
            #expect(result[0].reason == reason)
        }
    }

    private func wait(_ view: DiagramTextView) async throws {
        for _ in 0..<250 {
            if !view.diagramPending { view.layoutSubtreeIfNeeded(); return }
            try await Task.sleep(for: .milliseconds(20))
        }
        Issue.record("图排版超时")
    }
}
