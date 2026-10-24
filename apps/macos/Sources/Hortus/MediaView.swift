import AppKit
import HortusCore
import ImageIO
import Quartz

struct PreparedMedia: Sendable {
    let block: MediaBlock
    let imageSize: CGSize
    let reason: String
    let version: String
}

/// 只在后台读文件、探测尺寸和解码。相同文件版本共享像素，不缓存到笔记目录。
actor LocalImageCache {
    static let shared = LocalImageCache()
    private struct Entry {
        let stamp: String
        let size: CGSize
        var image: CGImage?
        let reason: String
    }
    private var entries: [String: Entry] = [:]
    private var order: [String] = []
    private var pixelBytes = 0
    private(set) var decodes = 0
    private(set) var mainThreadDecodes = 0

    private func checkedURL(_ path: String, vault: String) -> URL? {
        let root = URL(fileURLWithPath: vault).resolvingSymlinksInPath().standardizedFileURL.path
        let url = URL(fileURLWithPath: path).resolvingSymlinksInPath().standardizedFileURL
        return url.path.hasPrefix(root == "/" ? root : root + "/") ? url : nil
    }
    private func stamp(_ url: URL) -> String? {
        guard let values = try? url.resourceValues(forKeys: [.contentModificationDateKey, .fileSizeKey, .fileResourceIdentifierKey]),
              let date = values.contentModificationDate, let size = values.fileSize else { return nil }
        return "\(date.timeIntervalSince1970):\(size):\(String(describing: values.fileResourceIdentifier))"
    }
    private func inspect(_ path: String, vault: String) -> Entry {
        guard let url = checkedURL(path, vault: vault) else { return Entry(stamp: "", size: .zero, reason: "image.outside") }
        guard let stamp = stamp(url) else { return Entry(stamp: "", size: .zero, reason: "image.missing") }
        if let cached = entries[path], cached.stamp == stamp { return cached }
        if let old = entries.removeValue(forKey: path)?.image { pixelBytes -= old.bytesPerRow * old.height }
        let entry: Entry
        if let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
           let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
           let width = properties[kCGImagePropertyPixelWidth] as? Double,
           let height = properties[kCGImagePropertyPixelHeight] as? Double, width > 0, height > 0 {
            let orientation = properties[kCGImagePropertyOrientation] as? Int ?? 1
            entry = Entry(stamp: stamp, size: orientation >= 5 ? CGSize(width: height, height: width) : CGSize(width: width, height: height), reason: "")
        } else if url.pathExtension.lowercased() == "svg", let image = NSImage(contentsOf: url), image.size.width > 0, image.size.height > 0 {
            entry = Entry(stamp: stamp, size: image.size, reason: "")
        } else {
            entry = Entry(stamp: stamp, size: .zero, reason: url.pathExtension.lowercased() == "svg" ? "image.svg_unsupported" : "image.decode_failed")
        }
        entries[path] = entry
        order.removeAll { $0 == path }; order.append(path)
        return entry
    }
    func prepare(_ blocks: [MediaBlock], vault: String) -> [PreparedMedia] {
        blocks.map { block in
            guard block.columns.isEmpty, block.imageReason.isEmpty else {
                return PreparedMedia(block: block, imageSize: .zero, reason: block.imageReason, version: "")
            }
            let entry = inspect(block.imagePath, vault: vault)
            return PreparedMedia(block: block, imageSize: entry.size, reason: entry.reason, version: entry.stamp)
        }
    }
    func version(_ path: String, vault: String) -> String {
        checkedURL(path, vault: vault).flatMap { stamp($0) } ?? ""
    }
    func decode(_ path: String, vault: String) -> CGImage? {
        var entry = inspect(path, vault: vault)
        if let image = entry.image { return image }
        guard entry.reason.isEmpty, let url = checkedURL(path, vault: vault) else { return nil }
        if Thread.isMainThread { mainThreadDecodes += 1 }
        var image: CGImage?
        if let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary) {
            image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceThumbnailMaxPixelSize: 4096,
            ] as CFDictionary)
        }
        if image == nil, url.pathExtension.lowercased() == "svg", let native = NSImage(contentsOf: url) {
            var rect = CGRect(origin: .zero, size: native.size)
            image = native.cgImage(forProposedRect: &rect, context: nil, hints: nil)
        }
        guard let image else { return nil }
        decodes += 1
        entry.image = image; entries[path] = entry
        pixelBytes += image.bytesPerRow * image.height
        while pixelBytes > 128 * 1024 * 1024, order.count > 1 {
            let oldest = order.removeFirst()
            if oldest == path { order.append(oldest); continue }
            if let removed = entries.removeValue(forKey: oldest)?.image { pixelBytes -= removed.bytesPerRow * removed.height }
        }
        return image
    }
}

private final class TableCanvas: NSView {
    override var isFlipped: Bool { true }
    let columns: [Double]
    private let texts: [NSAttributedString]
    let heights: [CGFloat]
    var edit: (() -> Void)?
    init(_ block: MediaBlock, size: Double) {
        columns = block.columns
        let styling = SourceStyling(); styling.installReading([], size: size)
        let texts = block.cells.enumerated().map { index, cell -> NSAttributedString in
            let header: UInt32 = index < block.columns.count ? 2 : 0
            let text = NSMutableAttributedString(string: cell.text, attributes: styling.style(header))
            for run in cell.runs {
                let range = NSRange(location: Int(run.start), length: Int(run.length))
                if NSMaxRange(range) <= text.length { text.setAttributes(styling.style(run.mask | header), range: range) }
            }
            let paragraph = NSMutableParagraphStyle()
            paragraph.lineSpacing = 4
            let alignment = block.alignments[index % block.columns.count]
            paragraph.alignment = alignment == 1 ? .center : alignment == 2 ? .right : .left
            // 列宽已保证放得下最长的词；按词换行，超长网址系统会自己按字符断
            paragraph.lineBreakMode = .byWordWrapping
            text.addAttribute(.paragraphStyle, value: paragraph, range: NSRange(location: 0, length: text.length))
            return text
        }
        self.texts = texts
        heights = (0..<Int(block.rows)).map { row in
            (0..<block.columns.count).map { column in
                let text = texts[row * block.columns.count + column]
                return ceil(text.boundingRect(with: CGSize(width: max(1, block.columns[column] - 20), height: .greatestFiniteMagnitude),
                    options: [.usesLineFragmentOrigin, .usesFontLeading]).height) + 16
            }.max() ?? 36
        }
        super.init(frame: CGRect(x: 0, y: 0, width: columns.reduce(0, +), height: heights.reduce(0, +)))
        setAccessibilityElement(true); setAccessibilityRole(.table)
        setAccessibilityLabel(block.cells.map(\.text).joined(separator: ", "))
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func draw(_ dirtyRect: NSRect) {
        // 只画行线不画竖线：列之间靠内边距分开，界面少线
        var y: CGFloat = 0
        NSColor.separatorColor.setStroke()
        let lines = NSBezierPath(); lines.lineWidth = 0.5
        for (row, height) in heights.enumerated() {
            var x: CGFloat = 0
            for (column, width) in columns.enumerated() {
                texts[row * columns.count + column].draw(with: CGRect(x: x + 10, y: y + 8, width: width - 20, height: height - 8),
                    options: [.usesLineFragmentOrigin, .usesFontLeading])
                x += width
            }
            y += height
            lines.move(to: CGPoint(x: 0, y: y)); lines.line(to: CGPoint(x: bounds.width, y: y))
        }
        lines.stroke()
    }
    override func mouseDown(with event: NSEvent) { edit?() }
}

/// 表格的局部滚动和图片的快速查看均使用原生控件。
final class MediaView: NSView, @preconcurrency QLPreviewPanelDataSource {
    override var isFlipped: Bool { true }
    let prepared: PreparedMedia
    let vault: String
    let preferredSize: CGSize
    var onEdit: (() -> Void)?
    var onRefresh: (() -> Void)?
    var onImageDisplayed: (() -> Void)?
    private var image: CGImage?
    private var decoding: Task<Void, Never>?
    private var watchers: [DispatchSourceFileSystemObject] = []
    private var previewItem: NSURL?
    private var decodeFailed = false
    var imagePending: Bool { decoding != nil }
    var hasDecodedImage: Bool { image != nil }

    init(_ prepared: PreparedMedia, width: Double, size: Double, vault: String) {
        self.prepared = prepared; self.vault = vault
        let block = prepared.block
        var table: TableCanvas?
        if !block.columns.isEmpty {
            let canvas = TableCanvas(block, size: size); table = canvas
            let scrolls = canvas.frame.width > width + 0.5
            preferredSize = CGSize(width: min(width, canvas.frame.width), height: canvas.frame.height + (scrolls ? 15 : 0) + 12)
        } else if prepared.imageSize.width > 0 {
            let maximum = min(width, block.imageWidth.map(Double.init) ?? width)
            let scale = min(1, maximum / prepared.imageSize.width)
            preferredSize = CGSize(width: min(maximum, prepared.imageSize.width), height: prepared.imageSize.height * scale + 12)
        } else {
            let height: Double = prepared.reason.hasPrefix("embed.") ? 36 : prepared.reason == "image.remote" ? 84 : 64
            preferredSize = CGSize(width: width, height: height)
        }
        super.init(frame: CGRect(origin: .zero, size: preferredSize))
        if prepared.reason.isEmpty, !block.imagePath.isEmpty { previewItem = NSURL(fileURLWithPath: block.imagePath) }
        if let table {
            let scroll = NSScrollView(frame: CGRect(origin: .zero, size: CGSize(width: preferredSize.width, height: preferredSize.height - 12)))
            scroll.drawsBackground = false; scroll.hasHorizontalScroller = table.frame.width > width + 0.5
            scroll.hasVerticalScroller = false; scroll.autohidesScrollers = true
            scroll.documentView = table; addSubview(scroll)
            table.edit = { [weak self] in self?.onEdit?() }
        } else {
            setAccessibilityElement(true)
            setAccessibilityRole(prepared.reason.hasPrefix("embed.") ? .staticText : .image)
            setAccessibilityLabel(prepared.reason.hasPrefix("embed.") ? L10n.string(prepared.reason, block.imageTarget) : block.imageAlt)
        }
        if !block.imagePath.isEmpty { watch() }
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    func detach() {
        decoding?.cancel(); decoding = nil
        watchers.forEach { $0.cancel() }; watchers = []
        if QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(), panel.dataSource === self {
            panel.orderOut(nil); panel.dataSource = nil
        }
    }
    func becameVisible() {
        guard prepared.block.columns.isEmpty else { return }
        guard image == nil, decoding == nil, prepared.reason.isEmpty, !decodeFailed else { return }
        let path = prepared.block.imagePath, vault = vault
        decoding = Task { [weak self] in
            let decoded = await LocalImageCache.shared.decode(path, vault: vault)
            guard let self, !Task.isCancelled else { return }
            image = decoded; decodeFailed = decoded == nil; decoding = nil; needsDisplay = true
            displayIfNeeded()
            onImageDisplayed?()
        }
    }
    private func watch() {
        let url = URL(fileURLWithPath: prepared.block.imagePath)
        for path in [url.path, url.deletingLastPathComponent().path] {
            let fd = open(path, O_EVTONLY)
            guard fd >= 0 else { continue }
            let source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd,
                eventMask: [.write, .delete, .rename, .attrib, .extend], queue: .main)
            source.setEventHandler { [weak self] in
                guard let self else { return }
                Task { [weak self] in
                    guard let self else { return }
                    let current = await LocalImageCache.shared.version(prepared.block.imagePath, vault: vault)
                    if current != prepared.version { onRefresh?() }
                }
            }
            source.setCancelHandler { close(fd) }
            source.resume(); watchers.append(source)
        }
    }
    override func draw(_ dirtyRect: NSRect) {
        guard prepared.block.columns.isEmpty else { return }
        if let image, let context = NSGraphicsContext.current?.cgContext {
            context.saveGState()
            context.translateBy(x: 0, y: preferredSize.height - 12); context.scaleBy(x: 1, y: -1)
            context.draw(image, in: CGRect(x: 0, y: 0, width: preferredSize.width, height: preferredSize.height - 12))
            context.restoreGState()
        } else {
            let block = prepared.block
            let reason = decodeFailed ? (block.imagePath.lowercased().hasSuffix(".svg") ? "image.svg_unsupported" : "image.decode_failed") : prepared.reason
            if reason.hasPrefix("embed.") {
                (L10n.string(reason, block.imageTarget) as NSString).draw(
                    with: CGRect(x: 0, y: 4, width: bounds.width, height: 20),
                    options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin],
                    attributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: NSColor.secondaryLabelColor])
                return
            }
            let text = block.imageAlt.isEmpty ? block.imageTarget : block.imageAlt
            let title = NSAttributedString(string: text, attributes: [.font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.labelColor])
            title.draw(with: CGRect(x: 0, y: 4, width: bounds.width, height: 20), options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin])
            if reason == "image.remote" {
                let link = NSAttributedString(string: block.imageTarget, attributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.linkColor, .underlineStyle: NSUnderlineStyle.single.rawValue])
                link.draw(with: CGRect(x: 0, y: 27, width: bounds.width, height: 20), options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin])
            }
            if !reason.isEmpty {
                (L10n.string(reason) as NSString).draw(at: CGPoint(x: 0, y: reason == "image.remote" ? 51 : 28), withAttributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.secondaryLabelColor])
            }
        }
    }
    override func mouseDown(with event: NSEvent) {
        if let onEdit { onEdit(); return }
        if prepared.reason == "image.remote", let url = URL(string: prepared.block.imageTarget),
           ["http", "https"].contains(url.scheme?.lowercased() ?? "") {
            NSWorkspace.shared.open(url); return
        }
        showPreview()
    }
    func showPreview() {
        guard prepared.reason.isEmpty, !prepared.block.imagePath.isEmpty,
              let panel = QLPreviewPanel.shared() else { return }
        panel.dataSource = self; panel.reloadData(); panel.makeKeyAndOrderFront(nil)
    }
    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { previewItem == nil ? 0 : 1 }
    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! { previewItem }
}

struct NoteMedia {
    var range: NSRange
    let prepared: PreparedMedia
}

extension DiagramTextView {
    func installMedia(_ next: [NoteMedia], width: Double, size: Double, vault: String, refresh: @escaping () -> Void) {
        guard !next.isEmpty || !mediaBlocks.isEmpty else { return }
        let oldSizes = mediaViews.map(\.preferredSize), oldProjection = mediaProjection
        mediaViews.forEach { $0.detach(); $0.removeFromSuperview() }
        mediaPaintedAt = 0
        mediaBlocks = next
        mediaViews = next.map { item in
            let media = MediaView(item.prepared, width: width, size: size, vault: vault)
            media.isHidden = true; media.onRefresh = refresh
            media.onImageDisplayed = { [weak self] in self?.recordMediaDisplay() }
            if !isReading {
                media.onEdit = { [weak self, weak media] in
                    guard let self, let media, !compositionActive,
                          let index = mediaViews.firstIndex(where: { $0 === media }) else { return }
                    window?.makeFirstResponder(self)
                    setSelectedRange(NSRange(location: mediaBlocks[index].range.location, length: 0))
                    editingCoordinator?.mediaSelectionChanged()
                }
            }
            addSubview(media); return media
        }
        updateMediaProjection()
        if oldProjection == mediaProjection, oldSizes != mediaViews.map(\.preferredSize) { invalidateMediaLayout() }
        needsLayout = true; needsDisplay = true
    }
    func recordMediaDisplay() {
        guard mediaPaintedAt == 0, mediaViews.contains(where: { !$0.isHidden }),
              !mediaViews.contains(where: { !$0.isHidden && $0.imagePending }) else { return }
        mediaPaintedAt = ProcessInfo.processInfo.systemUptime
    }
    func updateMediaProjection() {
        let selections = selectedRanges.map(\.rangeValue)
        let projected = mediaBlocks.filter { item in
            isReading || !selections.contains { selection in
                selection.length == 0 ? NSLocationInRange(selection.location, item.range) : NSIntersectionRange(selection, item.range).length > 0
            }
        }.map(\.range)
        guard projected != mediaProjection else { return }
        mediaProjection = projected
        updateProjectionRanges()
        invalidateMediaLayout()
    }
    func updateProjectionRanges() {
        diagramStorage?.diagrams = (isReading ? [] : blocks.filter { $0.prepared != nil && !expanded.contains($0.ordinal) }.map(\.range)) + projectedMediaParagraphs
        diagramStorage?.inlineMedia = isReading ? [] : mediaProjection.filter { !mediaReplacesParagraph($0) }
    }
    private func paragraphRange(_ range: NSRange) -> NSRange {
        guard let text = textStorage?.string as NSString?, NSMaxRange(range) <= text.length else { return range }
        return text.paragraphRange(for: range)
    }
    func mediaReplacesParagraph(_ range: NSRange) -> Bool {
        guard !isReading, let text = textStorage?.string as NSString? else { return true }
        let paragraph = paragraphRange(range)
        let before = NSRange(location: paragraph.location, length: range.location - paragraph.location)
        let after = NSRange(location: NSMaxRange(range), length: NSMaxRange(paragraph) - NSMaxRange(range))
        return text.substring(with: before).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && text.substring(with: after).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
    var projectedMediaParagraphs: [NSRange] {
        isReading ? [] : mediaProjection.filter { mediaReplacesParagraph($0) }.map { paragraphRange($0) }
    }
    func mediaIndices(at offset: Int) -> [Int] {
        mediaBlocks.indices.filter {
            mediaProjection.contains(mediaBlocks[$0].range)
                && (isReading ? mediaBlocks[$0].range.location : paragraphRange(mediaBlocks[$0].range).location) == offset
        }
    }
    func invalidateMediaLayout() {
        guard let content = textLayoutManager?.textContentManager else { return }
        if let storage = textStorage, storage.length > 0 {
            content.performEditingTransaction { storage.edited(.editedAttributes, range: NSRange(location: 0, length: storage.length), changeInLength: 0) }
        }
        textLayoutManager?.invalidateLayout(for: content.documentRange)
        needsLayout = true; needsDisplay = true
    }
}
