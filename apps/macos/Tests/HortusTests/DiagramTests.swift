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
            files += try manager.contentsOfDirectory(at: root.appendingPathComponent("fixtures/mermaid/flowchart"), includingPropertiesForKeys: nil)
                .filter { !["unsupported.mmd", "subgraphs.mmd", "class-styles.mmd"].contains($0.lastPathComponent) }
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
                for width in (file.path.contains("/flowchart/") ? [700.0, 1100] : [380.0, 700, 1100]) {
                    for dark in [false, true] {
                        let count = mmd ? 1 : diagramBlocks(note: source).count
                        for block in 0..<count {
                            let scene = try engine.render(source: source, mmd: mmd, block: UInt32(block + 1), width: width, dark: dark, collapsed: [])
                            let prepared = PreparedScene(scene: try DiagramScene(scene))
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
        let coordinator = NoteText.EditorCoordinator()
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
        let coordinator = NoteText.EditorCoordinator()
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
        // 图尺寸改变只失效后文；结果仍须与强制整篇重排相同。
        for width in [420.0, 800.0] {
            scroll.setFrameSize(NSSize(width: width, height: 400))
            view.setFrameSize(NSSize(width: width, height: view.frame.height))
            try await wait(view)
            let manager = try #require(view.textLayoutManager)
            let content = try #require(manager.textContentManager)
            manager.ensureLayout(for: content.documentRange)
            view.needsLayout = true; view.layoutSubtreeIfNeeded()
            let projected = second.frame
            manager.invalidateLayout(for: content.documentRange)
            manager.ensureLayout(for: content.documentRange)
            view.needsLayout = true; view.layoutSubtreeIfNeeded()
            #expect(abs(projected.minY - second.frame.minY) <= 0.5)
            #expect(abs(projected.height - second.frame.height) <= 0.5)
        }
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

    @Test func unsupportedAndMalformedDiagramsKeepSource() async throws {
        let service = DiagramService()
        for (source, reason) in [("sequenceDiagram\nA->>B: Hi", L10n.string("Unsupported %@ diagram; showing source", "sequenceDiagram")),
                                 ("flowchart TB\nA --> B\nB ??? C", L10n.string("Syntax error on line %d; showing source", 3))] {
            let blocks = try await service.blocks(source: source, mmd: true, vault: "test", note: "x.mmd", core: nil)
            let result = await service.render(source: source, mmd: true, blocks: blocks, width: 700, dark: false)
            #expect(result[0].prepared == nil)
            #expect(result[0].reason == reason)
        }
    }

    @Test func graphSourceRangesAccessibilityAndFailureRetention() async throws {
        let source = "前言😀\r\n\r\n```mermaid\r\nflowchart LR\r\nA[中文😀 é] -->|是| C{决定}\r\nB(备用) --> C\r\nC -.-> A\r\n```\r\n"
        let service = DiagramService()
        let blocks = try await service.blocks(source: source, mmd: false, vault: "test", note: "graph.md", core: nil)
        let result = await service.render(source: source, mmd: false, blocks: blocks, width: 700, dark: false, requestGeneration: 1)
        let prepared = try #require(result[0].prepared, "\(result[0].reason ?? "No diagnostic")")
        #expect(prepared.acceptsInteraction)
        #expect(result[0].sceneVersion == 2)
        #expect(prepared.edgeRanges.count == 3)
        let diagram = DiagramView(prepared: prepared)
        let children = try #require(diagram.accessibilityChildren() as? [NSAccessibilityElement])
        let hit = try #require(prepared.scene.hitRegions.first(where: { $0.nodeId == "A" }))
        let range = try #require(diagram.sourceRange(at: CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY)))
        #expect((source as NSString).substring(with: range) == "A[中文😀 é]")
        #expect(diagram.accessibilityNodeCount == 3)
        #expect(diagram.accessibilityEdgeCount == 3)
        let dark = await service.render(source: source, mmd: false, blocks: result, width: 900, dark: true, requestGeneration: 2)
        diagram.update(try #require(dark[0].prepared))
        let updated = try #require(diagram.accessibilityChildren() as? [NSAccessibilityElement])
        #expect(children.count == updated.count)
        #expect(zip(children, updated).allSatisfy { $0 === $1 })
        let brokenSource = source.replacingOccurrences(of: "C -.-> A", with: "C ??? A")
        let brokenBlocks = try await service.blocks(source: brokenSource, mmd: false, vault: "test", note: "graph.md", core: nil)
        let failed = await service.render(source: brokenSource, mmd: false, blocks: brokenBlocks, width: 900, dark: true, requestGeneration: 3)
        #expect(failed[0].prepared != nil)
        #expect(failed[0].prepared?.acceptsInteraction == false)
        #expect(failed[0].reason != nil)
        let otherBlocks = try await service.blocks(source: brokenSource, mmd: false, vault: "test", note: "other.md", core: nil)
        let other = await service.render(source: brokenSource, mmd: false, blocks: otherBlocks, width: 900, dark: true)
        #expect(other[0].prepared == nil)
    }

    @Test func insertingAndDeletingFailedBlockNeverBorrowsAnotherDiagram() async throws {
        let valid = "```mermaid\nflowchart LR\nA[Existing] --> C\nB --> C\n```\n"
        let bad = "```mermaid\nflowchart TB\nX ??? Y\n```\n"
        let service = DiagramService()
        func render(_ source: String) async throws -> [NoteDiagram] {
            let blocks = try await service.blocks(source: source, mmd: false, vault: "test", note: "insert.md", core: nil)
            return await service.render(source: source, mmd: false, blocks: blocks, width: 700, dark: false)
        }
        let first = try await render(valid)
        #expect(first[0].prepared != nil)
        let inserted = try await render(bad + valid)
        #expect(inserted.count == 2)
        #expect(inserted[0].prepared == nil)
        #expect(inserted[0].previous == nil)
        #expect(inserted[0].reason != nil)
        #expect(inserted[1].prepared?.scene.hitRegions.contains(where: { $0.label == "Existing" }) == true)
        #expect(inserted[0].sourceStart != inserted[1].sourceStart)
        let restored = try await render(valid)
        #expect(restored.count == 1)
        #expect(restored[0].reason == nil)
        #expect(restored[0].prepared?.scene.hitRegions.count == 3)
        var moved = restored[0]
        moved.sourceStart += 1
        moved.prepared = nil; moved.previous = nil
        moved.inherit(restored[0])
        #expect(moved.prepared == nil && moved.previous == nil)
    }

    @Test func graphWindowServicesAndRapidGenerationsStayIsolated() async throws {
        let source = "flowchart LR\nA[甲😀] --> C((汇合))\nB[乙] --> C\n"
        func surface(width: Double) -> (NSWindow, DiagramTextView, NoteText.EditorCoordinator) {
            let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: width, height: 500))
            let view = DiagramTextView(diagramLayout: true)
            view.frame = scroll.bounds
            view.textContainer?.widthTracksTextView = true
            scroll.documentView = view
            let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.contentView = scroll
            window.orderFront(nil)
            let coordinator = NoteText.EditorCoordinator()
            coordinator.view = view
            return (window, view, coordinator)
        }
        let (leftWindow, left, leftCoordinator) = surface(width: 700)
        let (rightWindow, right, rightCoordinator) = surface(width: 1100)
        let leftService = DiagramService(), rightService = DiagramService()
        defer {
            leftCoordinator.detach(); rightCoordinator.detach()
            leftWindow.close(); rightWindow.close()
        }
        func input(_ source: String, service: DiagramService) -> NoteText {
            NoteText(text: source, path: "same.mmd", vault: "test", fontSize: 15, service: service, core: nil) { _, _ in }
        }
        leftCoordinator.update(input(source, service: leftService))
        rightCoordinator.update(input(source, service: rightService))
        try await wait(left); try await wait(right)
        let rightScene = try #require(right.blocks.first?.prepared)
        for generation in 0..<10 {
            let changed = source.replacingOccurrences(of: "甲😀", with: "更新😀\(generation)")
            leftCoordinator.update(input(changed, service: leftService))
            leftCoordinator.schedule(force: true)
            await Task.yield()
        }
        try await wait(left)
        #expect(left.string.contains("更新😀9"))
        #expect(left.blocks.first?.prepared?.scene.hitRegions.contains(where: { $0.label == "更新😀9" }) == true)
        #expect(right.string == source)
        #expect(right.blocks.first?.prepared === rightScene)
        #expect(left.blocks.first?.prepared?.scene.width != rightScene.scene.width)
        leftCoordinator.schedule(force: true)
        leftCoordinator.detach()
        try await Task.sleep(for: .milliseconds(50))
        #expect(left.blocks.isEmpty)
        #expect(leftCoordinator.retainedBlockCount == 0)
        #expect(right.blocks.first?.prepared === rightScene)
    }

    @Test func utfRangesRejectTruncatedScalarsAndSurrogates() {
        let source = "甲😀\r\né乙"
        let bytes = source.utf8.count
        let full = DiagramService.sourceRange(start: 0, end: UInt64(bytes), in: source)
        #expect(full == NSRange(location: 0, length: (source as NSString).length))
        #expect(DiagramService.sourceRange(start: 1, end: 3, in: source) == nil)
        #expect(DiagramService.sourceRange(start: 3, end: 5, in: source) == nil)
        #expect(DiagramService.sourceRange(start: 7, end: UInt64(bytes + 1), in: source) == nil)
        #expect(DiagramService.byteRange(NSRange(location: 2, length: 1), in: source) == nil)
        #expect(DiagramService.byteRange(NSRange(location: 0, length: (source as NSString).length), in: source) == 0..<bytes)
    }

    @Test func cachedSourceOffsetsValidateEveryScalarBoundary() {
        let source = "甲😀\r\né乙"
        let offsets = DiagramSourceOffsets(source)
        var byte: UInt64 = 0, unit = 0
        for scalar in source.unicodeScalars {
            let bytes = UInt64(scalar.utf8.count), units = scalar.utf16.count
            #expect(offsets.range(start: byte, end: byte + bytes) == NSRange(location: unit, length: units))
            if bytes > 1 { #expect(offsets.range(start: byte + 1, end: byte + bytes) == nil) }
            byte += bytes; unit += units
        }
        #expect(offsets.range(start: 0, end: byte) == NSRange(location: 0, length: unit))
        #expect(offsets.range(start: byte, end: byte + 1) == nil)
        #expect(offsets.range(start: 2, end: 1) == nil)
        #expect(offsets.range(start: UInt64.max, end: UInt64.max) == nil)
    }

    private func wait(_ view: DiagramTextView) async throws {
        for _ in 0..<250 {
            if !view.diagramPending { view.layoutSubtreeIfNeeded(); return }
            try await Task.sleep(for: .milliseconds(20))
        }
        Issue.record("图排版超时")
    }
}
