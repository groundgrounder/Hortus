import AppKit
import HortusCore
import ImageIO
import Testing
@testable import Hortus

@MainActor private func waitForMedia(_ predicate: () -> Bool) async throws {
    for _ in 0..<500 {
        if predicate() { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    Issue.record("等待媒体视图超时")
}

@Suite(.serialized) @MainActor
struct MediaTests {
    static let table = "| 中文 | Description |\n| :--- | ---: |\n| 短 | **Long text** and `code` |\n"
    static func writeImage(_ url: URL, width: Int = 320, height: Int = 120, type: String = "public.png") throws {
        let context = try #require(CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        context.setFillColor(CGColor(red: 0.3, green: 0.6, blue: 0.5, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        let destination = try #require(CGImageDestinationCreateWithURL(url as CFURL, type as CFString, 1, nil))
        CGImageDestinationAddImage(destination, try #require(context.makeImage()), nil)
        #expect(CGImageDestinationFinalize(destination))
    }

    @Test func sourceProjectionSelectionCompositionAndSingleUndo() async throws {
        let source = "前文😀\n\n" + Self.table + "\n![说明](a.png)\n\n结尾\n"
        let f = try EditingTests.Fixture(source)
        try Self.writeImage(f.vault.appendingPathComponent("a.png"))
        try await f.ready()
        f.view.layoutSubtreeIfNeeded()
        #expect(f.view.mediaBlocks.count == 2)
        #expect(f.view.mediaProjection.count == 2)
        #expect(f.view.string == source)
        let table = try #require(f.view.mediaBlocks.first)
        f.view.setSelectedRange(NSRange(location: table.range.location + 2, length: 0))
        f.coordinator.mediaSelectionChanged()
        #expect(!f.view.mediaProjection.contains(table.range))
        f.coordinator.alignTable()
        try await waitForMedia { f.view.string != source }
        let aligned = f.view.string
        #expect(aligned.contains("| 中文 |"))
        #expect(try await f.text() == aligned)
        f.view.undoManager?.undo()
        try await waitForMedia { f.view.string == source }
        #expect(try await f.text() == source)
        f.view.undoManager?.redo()
        try await waitForMedia { f.view.string == aligned }
        try await f.ready()
        let originalProjection = f.view.mediaProjection
        f.view.setMarkedText("zhong", selectedRange: NSRange(location: 5, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        #expect(f.view.hasMarkedText())
        #expect(f.view.mediaProjection.count == originalProjection.count)
        f.view.insertText("中", replacementRange: NSRange(location: NSNotFound, length: 0))
        #expect(!f.view.hasMarkedText())
        #expect(try await f.text() == f.view.string)
        f.view.setSelectedRange(NSRange(location: f.view.sourceUTF16Length, length: 0))
        try await f.ready()
        #expect(f.view.mediaProjection.count == 2)
        await f.close()
    }

    @Test func readingGeometryCacheQuickLookAndExternalReplacement() async throws {
        let source = "![Local](a.png)\n\n" + Self.table + "\n![Remote](https://example.invalid/a.png)\n"
        let f = try ReadingTests.Fixture(source)
        let url = f.editing.vault.appendingPathComponent("a.png")
        try Self.writeImage(url)
        try await f.start()
        try await waitForMedia { f.view.mediaViews.first?.hasDecodedImage == true }
        #expect(f.view.mediaBlocks.count == 3)
        let first = try #require(f.view.mediaViews.first)
        #expect(first.preferredSize == CGSize(width: 320, height: 132))
        let remote = try #require(f.view.mediaViews.last)
        #expect(remote.prepared.reason == "image.remote")
        #expect(!remote.imagePending)
        let decodeCount = await LocalImageCache.shared.decodes
        let path = first.prepared.block.imagePath
        let cached = await LocalImageCache.shared.decode(path, vault: f.editing.vault.path)
        #expect(cached != nil)
        #expect(await LocalImageCache.shared.decode(path, vault: "/") != nil)
        #expect(await LocalImageCache.shared.decode(path, vault: f.editing.vault.path + "-other") == nil)
        #expect(await LocalImageCache.shared.decodes == decodeCount)
        #expect(await LocalImageCache.shared.mainThreadDecodes == 0)
        let manager = try #require(f.view.textLayoutManager), content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        var sizes: [CGSize] = []
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            if let size = (fragment as? NoteLayoutFragment)?.replacement { sizes.append(size) }
            return true
        }
        #expect(sizes.count == 3)
        #expect(sizes.allSatisfy { $0.width <= f.view.bounds.width - 2 * f.view.textContainerInset.width })
        #expect(first.numberOfPreviewItems(in: nil) == 1)
        #expect(first.previewPanel(nil, previewItemAt: 0)?.previewItemURL?.resolvingSymlinksInPath() == url.resolvingSymlinksInPath())
        try Self.writeImage(url, width: 320, height: 200)
        try await waitForMedia { f.view.mediaViews.first?.preferredSize.height == 212 && f.view.mediaViews.first?.hasDecodedImage == true }
        #expect(await LocalImageCache.shared.decodes == decodeCount + 1)
        f.view.layoutSubtreeIfNeeded()
        #expect(f.view.mediaViews[1].frame.minY >= f.view.mediaViews[0].frame.maxY - 1)
        #expect(try FileManager.default.contentsOfDirectory(atPath: f.editing.vault.path).sorted() == ["a.png", "note.md"])
        await f.close()
    }

    @Test func inlineImagesKeepSurroundingTextAndIndentedTablesProject() async throws {
        let source = "前文 ![One](a.png) 中间 ![[a.png]] 后文\n\n" + Self.table.split(separator: "\n").map { "  " + $0 }.joined(separator: "\n") + "\n\n结尾\n"
        let f = try EditingTests.Fixture(source)
        try Self.writeImage(f.vault.appendingPathComponent("a.png"))
        try await f.ready()
        f.view.setSelectedRange(NSRange(location: f.view.sourceUTF16Length, length: 0))
        f.coordinator.mediaSelectionChanged()
        f.view.layoutSubtreeIfNeeded()
        #expect(f.view.mediaBlocks.count == 3)
        #expect(f.view.mediaViews.allSatisfy { !$0.isHidden })
        #expect(f.view.mediaViews[1].frame.minY >= f.view.mediaViews[0].frame.maxY)
        #expect(f.view.string == source)
        let content = try #require(f.view.diagramStorage)
        var displayed = ""
        _ = content.enumerateTextElements(from: content.documentRange.location) { element in
            if let paragraph = element as? NSTextParagraph { displayed += paragraph.attributedString.string }
            return true
        }
        #expect(displayed.contains("前文"))
        #expect(displayed.contains("中间"))
        #expect(displayed.contains("后文"))
        #expect(!displayed.contains("!["))
        let image = f.view.mediaBlocks[0].range
        f.view.setSelectedRange(NSRange(location: image.location + 3, length: 0))
        f.coordinator.mediaSelectionChanged()
        #expect(!f.view.mediaProjection.contains(image))
        #expect(f.view.mediaProjection.count == 2)
        #expect(try await f.text() == source)
        await f.close()
    }

    @Test func nativeSVGAndMetadataEventsDoNotRefreshUnchangedPixels() async throws {
        let f = try ReadingTests.Fixture("![[sample.svg]]\n")
        let url = f.editing.vault.appendingPathComponent("sample.svg")
        try Data("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"240\" height=\"80\"><rect width=\"240\" height=\"80\" fill=\"green\"/></svg>".utf8).write(to: url)
        try await f.start()
        try await waitForMedia { f.view.mediaViews.first?.hasDecodedImage == true }
        let first = try #require(f.view.mediaViews.first)
        #expect(first.preferredSize == CGSize(width: 240, height: 92))
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        try await Task.sleep(for: .milliseconds(400))
        #expect(f.view.mediaViews.first === first)
        #expect(await LocalImageCache.shared.mainThreadDecodes == 0)
        await f.close()
    }

    @Test func rasterFormatsDecodeAndInvalidFilesExplainTheFailure() async throws {
        let formats = [("png", "public.png"), ("jpg", "public.jpeg"), ("jpeg", "public.jpeg"),
                       ("gif", "com.compuserve.gif"), ("heic", "public.heic")]
        let source = (formats.map { $0.0 } + ["webp", "broken.png", "broken.svg"])
            .map { "![[sample.\($0)]]\n\n" }.joined()
        let f = try ReadingTests.Fixture(source)
        for (ext, type) in formats {
            try Self.writeImage(f.editing.vault.appendingPathComponent("sample.\(ext)"), type: type)
        }
        // 单像素公开格式夹具，不依赖额外的 WebP 编码器。
        let webp = try #require(Data(base64Encoded: "UklGRiIAAABXRUJQVlA4IBYAAAAwAQCdASoBAAEADsD+JaQAA3AAAAAA"))
        try webp.write(to: f.editing.vault.appendingPathComponent("sample.webp"))
        for name in ["broken.png", "broken.svg"] {
            try Data("invalid image".utf8).write(to: f.editing.vault.appendingPathComponent("sample.\(name)"))
        }
        try await f.start()
        #expect(f.view.mediaViews.count == 8)
        for media in f.view.mediaViews {
            let path = media.prepared.block.imagePath
            if path.contains("broken") {
                #expect(media.prepared.reason == (path.hasSuffix(".svg") ? "image.svg_unsupported" : "image.decode_failed"))
            } else {
                #expect(media.prepared.reason.isEmpty)
                #expect(await LocalImageCache.shared.decode(path, vault: f.editing.vault.path) != nil)
            }
        }
        #expect(await LocalImageCache.shared.mainThreadDecodes == 0)
        await f.close()
    }

    @Test func attachmentFallbackWidthAndEmbeddedNotesUseNativeMediaViews() async throws {
        let source = "![[nested.png|160x999]]\n\n![300](nested.png)\n\n![[Another note]]\n\n![[document.pdf]]\n"
        let f = try ReadingTests.Fixture(source)
        let folder = f.editing.vault.appendingPathComponent("attachments")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try Self.writeImage(folder.appendingPathComponent("nested.png"))
        try await f.start()
        let blocks = f.view.mediaBlocks.map { $0.prepared.block }
        #expect(blocks.count == 4)
        #expect(blocks[0].imagePath.hasSuffix("/attachments/nested.png"))
        #expect(blocks[0].imageWidth == 160)
        #expect(f.view.mediaViews[0].preferredSize == CGSize(width: 160, height: 72))
        #expect(blocks[1].imageWidth == nil)
        #expect(f.view.mediaViews[1].preferredSize == CGSize(width: 320, height: 132))
        #expect(blocks[2].imageReason == "embed.note")
        #expect(blocks[3].imageReason == "embed.file")
        #expect(f.view.mediaViews[2].preferredSize.height == 36)
        #expect(f.view.mediaViews[2].accessibilityLabel() == L10n.string("embed.note", "Another note"))
        let metrics = try await f.document.io.perform { $0.attachmentMetrics() }
        #expect(metrics.builds == 1)
        #expect(metrics.files == 2)
        #expect(await LocalImageCache.shared.mainThreadDecodes == 0)
        await f.close()
    }
}
