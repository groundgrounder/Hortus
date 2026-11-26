import AppKit
import HortusCore
import HortusApple
import ImageIO
import Quartz

private struct TableCellLink {
    let range: NSRange
    let target: String
    let heading: String?
    let broken: Bool
}

private struct TableCellTag {
    let range: NSRange
    let tag: String
}

private final class TableCellAccessibilityElement: NSAccessibilityElement {
    private let frameProvider: @MainActor @Sendable () -> NSRect

    @MainActor init(owner: TableCanvas, row: Int, column: Int, label: String) {
        frameProvider = { [weak owner] in
            guard let owner, let window = owner.window else { return .zero }
            let rect = owner.cellFrame(row: row, column: column)
            guard !rect.isEmpty else { return .zero }
            return window.convertToScreen(owner.convert(rect, to: nil))
        }
        super.init()
        setAccessibilityParent(owner)
        setAccessibilityRole(.cell)
        setAccessibilityLabel(label)
        setAccessibilityRowIndexRange(NSRange(location: row, length: 1))
        setAccessibilityColumnIndexRange(NSRange(location: column, length: 1))
    }

    override func accessibilityFrame() -> NSRect {
        let provider = frameProvider
        return MainActor.assumeIsolated { provider() }
    }
}

@MainActor
private final class TableCanvas: NSView {
    override var isFlipped: Bool { true }
    let columns: [Double]
    private let texts: [NSAttributedString]
    let heights: [CGFloat]
    private let cellLinks: [[TableCellLink]]
    private let cellTags: [[TableCellTag]]
    private var cellElements: [TableCellAccessibilityElement] = []
    private var layoutCache: [Int: (storage: NSTextStorage, layout: NSLayoutManager, container: NSTextContainer)] = [:]
    var edit: (() -> Void)?
    var onOpenLink: ((String, String?) -> Void)?
    var onShowTag: ((String) -> Void)?

    init(_ block: MediaBlock, size: Double, math: [[PreparedMathBlock]], knowledge: KnowledgeMarkupInfo? = nil) {
        columns = block.columns
        let styling = SourceStyling(); styling.installReading([], size: size)
        var resolvedLookup: [String: Bool] = [:]
        if let links = knowledge?.links {
            for link in links where !link.inDiagram && !link.attachment {
                let key = Self.linkLookupKey(target: link.target, heading: link.heading)
                resolvedLookup[key] = link.resolvedPath != nil
            }
        }
        var allLinks: [[TableCellLink]] = []
        var allTags: [[TableCellTag]] = []
        var cellLabels: [String] = []
        let texts = block.cells.enumerated().map { index, cell -> NSAttributedString in
            let header: UInt32 = index < block.columns.count ? 2 : 0
            let parsedLinks = Self.extractWikilinks(from: cell.source)
            var linkCursor = 0
            var lastLinkEnd: Int?
            var currentParsed: (target: String, heading: String?)?
            var linksForCell: [TableCellLink] = []
            var tagsForCell: [TableCellTag] = []
            let text = NSMutableAttributedString(string: cell.text, attributes: styling.style(header))
            for run in cell.runs {
                let range = NSRange(location: Int(run.start), length: Int(run.length))
                guard NSMaxRange(range) <= text.length else { continue }
                var mask = run.mask | header
                if run.mask & (1 << 18) != 0 {
                    let parsed: (target: String, heading: String?)?
                    if let prevEnd = lastLinkEnd, prevEnd == range.location {
                        parsed = currentParsed
                    } else {
                        parsed = linkCursor < parsedLinks.count ? parsedLinks[linkCursor] : nil
                        linkCursor += 1
                        currentParsed = parsed
                    }
                    lastLinkEnd = NSMaxRange(range)
                    if let parsed {
                        let key = Self.linkLookupKey(target: parsed.target, heading: parsed.heading)
                        let resolved = resolvedLookup[key] ?? true
                        let broken = knowledge != nil && !resolved
                        if broken { mask |= (1 << 19) }
                        linksForCell.append(TableCellLink(range: range, target: parsed.target,
                                                          heading: parsed.heading, broken: broken))
                    }
                } else {
                    lastLinkEnd = nil
                    currentParsed = nil
                    if run.mask & 512 != 0 {
                        let raw = (cell.text as NSString).substring(with: range)
                        let tag = raw.hasPrefix("#") ? String(raw.dropFirst()) : raw
                        if !tag.isEmpty {
                            tagsForCell.append(TableCellTag(range: range, tag: tag))
                        }
                    }
                }
                text.setAttributes(styling.style(mask), range: range)
            }
            if index < math.count {
                for formula in math[index] {
                    let range = NSRange(location: Int(formula.block.displayStart), length: Int(formula.block.displayLength))
                    guard NSMaxRange(range) <= text.length else { continue }
                    let attachment = NSTextAttachment()
                    let color = styling.style(formula.block.mask | header)[.foregroundColor] as? NSColor ?? .textColor
                    attachment.attachmentCell = TableMathCell(formula, color: color)
                    text.addAttribute(.attachment, value: attachment, range: range)
                }
            }
            let paragraph = NSMutableParagraphStyle()
            paragraph.lineSpacing = 4
            let alignment = block.alignments[index % block.columns.count]
            paragraph.alignment = alignment == 1 ? .center : alignment == 2 ? .right : .left
            // 列宽已保证放得下最长的词；按词换行，超长网址系统会自己按字符断
            paragraph.lineBreakMode = .byWordWrapping
            text.addAttribute(.paragraphStyle, value: paragraph, range: NSRange(location: 0, length: text.length))
            allLinks.append(linksForCell)
            allTags.append(tagsForCell)
            cellLabels.append(Self.accessibleText(for: cell))
            return text
        }
        self.texts = texts
        self.cellLinks = allLinks
        self.cellTags = allTags
        heights = (0..<Int(block.rows)).map { row in
            (0..<block.columns.count).map { column in
                let text = texts[row * block.columns.count + column]
                return ceil(text.boundingRect(with: CGSize(width: max(1, block.columns[column] - 20), height: .greatestFiniteMagnitude),
                    options: [.usesLineFragmentOrigin, .usesFontLeading]).height) + 16
            }.max() ?? 36
        }
        super.init(frame: CGRect(x: 0, y: 0, width: columns.reduce(0, +), height: heights.reduce(0, +)))
        setAccessibilityElement(true); setAccessibilityRole(.table)
        setAccessibilityLabel(cellLabels.joined(separator: ", "))
        toolTip = math.flatMap { $0 }.compactMap(\.reason).joined(separator: "\n")
        setAccessibilityHelp(toolTip)
        if !columns.isEmpty {
            cellElements = cellLabels.enumerated().map { index, label in
                TableCellAccessibilityElement(owner: self, row: index / columns.count,
                                              column: index % columns.count, label: label)
            }
        }
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private static func accessibleText(for cell: TableCell) -> String {
        let label = NSMutableString(string: cell.text)
        for formula in cell.math.reversed() where !formula.latex.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            let range = NSRange(location: Int(formula.displayStart), length: Int(formula.displayLength))
            if NSMaxRange(range) <= label.length {
                label.replaceCharacters(in: range, with: L10n.string("Formula: %@", formula.latex))
            }
        }
        return label as String
    }

    private static func linkLookupKey(target: String, heading: String?) -> String {
        target.lowercased() + "\u{0}" + (heading?.lowercased() ?? "")
    }

    /// 按源码顺序提取单元格内的非嵌入双链目标与标题片段，与 Rust 生成的双链 run 一一对应。
    private static func extractWikilinks(from source: String) -> [(target: String, heading: String?)] {
        var result: [(target: String, heading: String?)] = []
        let chars = Array(source)
        var i = 0
        var inCode = false
        while i < chars.count {
            if chars[i] == "\\" && i + 1 < chars.count {
                i += 2; continue
            }
            if chars[i] == "`" {
                inCode.toggle(); i += 1; continue
            }
            if !inCode && i + 1 < chars.count && chars[i] == "[" && chars[i + 1] == "[" {
                let isEmbed = i > 0 && chars[i - 1] == "!"
                var j = i + 2
                while j + 1 < chars.count && !(chars[j] == "]" && chars[j + 1] == "]") { j += 1 }
                if j + 1 < chars.count {
                    if !isEmbed {
                        let inner = String(chars[(i + 2)..<j])
                        let rawBeforeAlias = inner.split(separator: "|", maxSplits: 1, omittingEmptySubsequences: false).first.map(String.init) ?? inner
                        let beforeAlias = rawBeforeAlias.hasSuffix("\\") ? String(rawBeforeAlias.dropLast()) : rawBeforeAlias
                        let parts = beforeAlias.split(separator: "#", maxSplits: 1, omittingEmptySubsequences: false)
                        let target = String(parts[0]).trimmingCharacters(in: .whitespaces)
                        let heading = parts.count > 1 ? String(parts[1]).trimmingCharacters(in: .whitespaces) : nil
                        if !target.isEmpty || (heading?.isEmpty == false) {
                            result.append((target, heading?.isEmpty == true ? nil : heading))
                        }
                    }
                    i = j + 2
                    continue
                }
            }
            i += 1
        }
        return result
    }

    var inspectionCells: [[String: Any]] {
        texts.enumerated().map { index, text in
            var sizes: [Double] = [], bold: [Bool] = []
            text.enumerateAttribute(.font, in: NSRange(location: 0, length: text.length)) { value, _, _ in
                if let font = value as? NSFont {
                    sizes.append(font.pointSize)
                    bold.append(NSFontManager.shared.traits(of: font).contains(.boldFontMask))
                }
            }
            let links = index < cellLinks.count ? cellLinks[index].map {
                ["target": $0.target, "heading": $0.heading ?? "", "broken": $0.broken] as [String: Any]
            } : []
            return ["text": text.string, "font_sizes": sizes, "bold": bold, "links": links]
        }
    }

    func cellFrame(row: Int, column: Int) -> NSRect {
        guard row >= 0, row < heights.count, column >= 0, column < columns.count else { return .zero }
        let x = columns.prefix(column).reduce(0, +)
        let y = heights.prefix(row).reduce(0, +)
        return NSRect(x: x, y: y, width: columns[column], height: heights[row])
    }

    override func accessibilityChildren() -> [Any]? { cellElements }

    override func accessibilityHitTest(_ point: NSPoint) -> Any? {
        let hit = MainActor.assumeIsolated { () -> AccessibilityHitResult in
            if let formula = cachedFormulaAccessibility(at: point) {
                return AccessibilityHitResult(value: formula)
            }
            for cell in cellElements {
                let frame = cell.accessibilityFrame()
                if !frame.isEmpty && frame.contains(point) {
                    return AccessibilityHitResult(value: cell)
                }
            }
            return AccessibilityHitResult(value: nil)
        }
        return hit.value ?? super.accessibilityHitTest(point)
    }

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

    override func resetCursorRects() {
        super.resetCursorRects()
        guard onOpenLink != nil || onShowTag != nil, !columns.isEmpty else { return }
        for cellIndex in texts.indices {
            let links = cellLinks[cellIndex]
            let tags = cellTags[cellIndex]
            guard !links.isEmpty || !tags.isEmpty, let cached = layout(for: cellIndex) else { continue }
            let row = cellIndex / columns.count
            let column = cellIndex % columns.count
            let originX = columns.prefix(column).reduce(0, +) + 10
            let originY = heights.prefix(row).reduce(0, +) + 8
            for range in links.map(\.range) + tags.map(\.range) {
                let glyphs = cached.layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
                let rect = cached.layout.boundingRect(forGlyphRange: glyphs, in: cached.container)
                    .offsetBy(dx: originX, dy: originY)
                if !rect.isEmpty { addCursorRect(rect, cursor: .pointingHand) }
            }
        }
    }

    private func hitTestCell(at point: CGPoint) -> (cell: Int, character: Int)? {
        guard !columns.isEmpty, !heights.isEmpty else { return nil }
        var y: CGFloat = 0
        var matchedRow: Int?
        for (row, height) in heights.enumerated() {
            if point.y >= y && point.y < y + height { matchedRow = row; break }
            y += height
        }
        var x: CGFloat = 0
        var matchedCol: Int?
        for (col, width) in columns.enumerated() {
            if point.x >= x && point.x < x + width { matchedCol = col; break }
            x += width
        }
        guard let row = matchedRow, let col = matchedCol else { return nil }
        let cell = row * columns.count + col
        guard let cached = layout(for: cell) else { return nil }
        let local = CGPoint(x: point.x - (x + 10), y: point.y - (y + 8))
        guard local.x >= 0, local.y >= 0 else { return nil }
        var fraction: CGFloat = 0
        let index = cached.layout.characterIndex(for: local, in: cached.container,
                                                 fractionOfDistanceBetweenInsertionPoints: &fraction)
        guard index < texts[cell].length else { return nil }
        let glyphIndex = cached.layout.glyphIndexForCharacter(at: index)
        let rect = cached.layout.boundingRect(forGlyphRange: NSRange(location: glyphIndex, length: 1), in: cached.container)
        guard rect.insetBy(dx: -2, dy: -2).contains(local) else { return nil }
        return (cell, index)
    }

    override func mouseDown(with event: NSEvent) {
        if onOpenLink != nil || onShowTag != nil {
            let point = convert(event.locationInWindow, from: nil)
            if let (cell, index) = hitTestCell(at: point) {
                if let link = cellLinks[cell].first(where: { NSLocationInRange(index, $0.range) }),
                   let onOpenLink {
                    onOpenLink(link.target, link.heading)
                    return
                }
                if let tag = cellTags[cell].first(where: { NSLocationInRange(index, $0.range) }),
                   let onShowTag {
                    onShowTag(tag.tag)
                    return
                }
            }
        }
        edit?()
    }

    private func layout(for cell: Int) -> (layout: NSLayoutManager, container: NSTextContainer)? {
        guard cell >= 0, cell < texts.count, !columns.isEmpty else { return nil }
        if let cached = layoutCache[cell] { return (cached.layout, cached.container) }
        let storage = NSTextStorage(attributedString: texts[cell])
        let layout = NSLayoutManager()
        let container = NSTextContainer(size: CGSize(width: max(1, columns[cell % columns.count] - 20), height: .greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        layout.addTextContainer(container); storage.addLayoutManager(layout)
        layoutCache[cell] = (storage, layout, container)
        return (layout, container)
    }

    func formulaFrame(row: Int, column: Int, formulaIndex: Int) -> NSRect? {
        let cell = row * columns.count + column
        guard cell >= 0, cell < texts.count, let cached = layout(for: cell) else {
            return nil
        }
        // 位置查询才创建单元格布局，文字存储须和布局一起持有。
        var attachments: [NSRange] = []
        texts[cell].enumerateAttribute(.attachment, in: NSRange(location: 0, length: texts[cell].length)) { value, range, _ in
            if value is NSTextAttachment { attachments.append(range) }
        }
        guard formulaIndex < attachments.count else { return nil }
        let glyphs = cached.layout.glyphRange(forCharacterRange: attachments[formulaIndex], actualCharacterRange: nil)
        let rect = cached.layout.boundingRect(forGlyphRange: glyphs, in: cached.container)
        let x = columns.prefix(column).reduce(0, +) + 10
        let y = heights.prefix(row).reduce(0, +) + 8
        return rect.offsetBy(dx: x, dy: y)
    }

    #if DEBUG
    var inspectionLinks: [[(range: NSRange, target: String, heading: String?, broken: Bool)]] {
        cellLinks.map { links in
            links.map { ($0.range, $0.target, $0.heading, $0.broken) }
        }
    }
    #endif
}

/// 表格的局部滚动和图片的快速查看均使用原生控件。
final class MediaView: NSView, @preconcurrency QLPreviewPanelDataSource {
    override var isFlipped: Bool { true }
    let prepared: PreparedMedia
    let vault: String
    let renderWidth: Double
    let renderSize: Double
    let knowledgeVersion: Int
    let renderDark: Bool
    let renderIncreasedContrast: Bool
    private(set) var preferredSize: CGSize
    var onEdit: (() -> Void)?
    var onOpenLink: ((String, String?) -> Void)? {
        didSet { tableCanvas?.onOpenLink = onOpenLink }
    }
    var onShowTag: ((String) -> Void)? {
        didSet { tableCanvas?.onShowTag = onShowTag }
    }
    var onSizeChange: (() -> Void)?
    private var embeddedCard: EmbeddedNoteView?
    var onRefresh: (() -> Void)?
    var onImageDisplayed: (() -> Void)?
    private var image: CGImage?
    private var decoding: Task<Void, Never>?
    private var watchers: [DispatchSourceFileSystemObject] = []
    private var previewItem: NSURL?
    private var decodeFailed = false
    private var tableCanvas: TableCanvas?
    var imagePending: Bool { decoding != nil }
    var hasDecodedImage: Bool { image != nil }

    init(_ prepared: PreparedMedia, width: Double, size: Double, vault: String,
         knowledge: KnowledgeMarkupInfo? = nil, knowledgeVersion: Int = 0,
         dark: Bool = false, increasedContrast: Bool = false) {
        self.prepared = prepared; self.vault = vault
        self.renderWidth = width; self.renderSize = size; self.knowledgeVersion = knowledgeVersion
        self.renderDark = dark; self.renderIncreasedContrast = increasedContrast
        let block = prepared.block
        var table: TableCanvas?
        if !block.columns.isEmpty {
            let canvas = TableCanvas(block, size: size, math: prepared.math, knowledge: knowledge); table = canvas
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
            tableCanvas = table
            let scroll = FormulaScrollView(frame: CGRect(origin: .zero, size: CGSize(width: preferredSize.width, height: preferredSize.height - 12)))
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
        else if prepared.reason == "image.missing" { watchVaultForMissingImage() }
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func canReuse(for next: PreparedMedia, width: Double, size: Double, vault: String,
                  knowledgeVersion: Int, dark: Bool = false, increasedContrast: Bool = false) -> Bool {
        guard self.vault == vault, abs(renderWidth - width) < 0.25, renderSize == size else { return false }
        guard prepared.reason == next.reason, prepared.version == next.version,
              prepared.imageSize == next.imageSize else { return false }
        let a = prepared.block, b = next.block
        guard a.imagePath == b.imagePath, a.imageTarget == b.imageTarget, a.imageAlt == b.imageAlt,
              a.imageWidth == b.imageWidth, a.imageWiki == b.imageWiki,
              a.columns == b.columns, a.rows == b.rows else { return false }
        if !a.columns.isEmpty {
            guard self.knowledgeVersion == knowledgeVersion,
                  renderDark == dark,
                  renderIncreasedContrast == increasedContrast,
                  a.cells == b.cells,
                  prepared.math.map({ $0.map(\.block.latex) }) == next.math.map({ $0.map(\.block.latex) }) else {
                return false
            }
        }
        return true
    }

    func existingEmbed(matching key: EmbedRequest.ReuseKey) -> EmbeddedNoteView? {
        guard let card = embeddedCard, card.request.reuseKey == key else { return nil }
        return card
    }

    func installEmbed(_ card: EmbeddedNoteView) {
        if embeddedCard === card { return }
        embeddedCard?.cancel()
        embeddedCard?.removeFromSuperview()
        embeddedCard = card
        card.onHeightChange = { [weak self] in self?.resizeEmbed() }
        addSubview(card)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(card.cardAccessibilityLabel)
        resizeEmbed(force: true)
    }

    private func resizeEmbed(force: Bool = false) {
        guard let card = embeddedCard else { return }
        let height = max(36, card.preferredSize.height)
        guard force || abs(preferredSize.height - height) > 0.5 else { return }
        preferredSize = CGSize(width: preferredSize.width, height: height)
        card.frame = CGRect(origin: .zero, size: preferredSize)
        frame.size = preferredSize
        onSizeChange?()
    }

    func detach() {
        decoding?.cancel(); decoding = nil
        embeddedCard?.cancel()
        watchers.forEach { $0.cancel() }; watchers = []
        if QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(), panel.dataSource === self {
            panel.orderOut(nil); panel.dataSource = nil
        }
    }

    var hasMissingImageMedia: Bool {
        if prepared.reason == "image.missing" { return true }
        return embeddedCard?.hasMissingImageMedia == true
    }

    func reloadEmbeddedMediaIfNeeded() {
        guard let card = embeddedCard, card.hasMissingImageMedia else { return }
        card.reload()
    }

    #if DEBUG
    var inspectionEmbeddedCard: EmbeddedNoteView? { embeddedCard }
    var inspectionTableCellLinks: [[(range: NSRange, target: String, heading: String?, broken: Bool)]] {
        tableCanvas?.inspectionLinks ?? []
    }
    func simulateEmbedHeightGrowthForTesting(_ height: CGFloat) {
        guard let card = embeddedCard else { return }
        let nextHeight = max(36, height)
        guard abs(preferredSize.height - nextHeight) > 0.5 else { return }
        preferredSize = CGSize(width: preferredSize.width, height: nextHeight)
        card.frame = CGRect(origin: .zero, size: preferredSize)
        frame.size = preferredSize
        onSizeChange?()
    }
    #endif

    var tableCanvasView: NSView? { tableCanvas }
    var tableInspectionCells: [[String: Any]] { tableCanvas?.inspectionCells ?? [] }
    func formulaFrame(row: Int, column: Int, formulaIndex: Int) -> NSRect? {
        tableCanvas?.formulaFrame(row: row, column: column, formulaIndex: formulaIndex)
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
    /// 缺失图片尚未在库中解析出实体路径时，监听库根与常见附件目录，新增图片后自动清缓存并刷新。
    private func watchVaultForMissingImage() {
        var directories = [vault]
        let attachments = URL(fileURLWithPath: vault).appendingPathComponent("attachments").path
        if FileManager.default.fileExists(atPath: attachments) { directories.append(attachments) }
        for path in directories {
            let fd = open(path, O_EVTONLY)
            guard fd >= 0 else { continue }
            let source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd,
                eventMask: [.write, .rename, .extend], queue: .main)
            source.setEventHandler { [weak self] in
                guard let self else { return }
                Task { [weak self] in
                    await LocalImageCache.shared.invalidateFailed()
                    self?.onRefresh?()
                }
            }
            source.setCancelHandler { close(fd) }
            source.resume(); watchers.append(source)
        }
    }
    override func draw(_ dirtyRect: NSRect) {
        guard prepared.block.columns.isEmpty, embeddedCard == nil else { return }
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
        if embeddedCard != nil { return }
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
    #if DEBUG
    var inspectionEmbeddedCards: [EmbeddedNoteView] { mediaViews.compactMap(\.inspectionEmbeddedCard) }
    #endif

    func installMedia(_ next: [NoteMedia], width: Double, size: Double, vault: String,
                      knowledge: KnowledgeMarkupInfo? = nil, knowledgeVersion: Int = 0,
                      onOpenLink: ((String, String?) -> Void)? = nil,
                      onShowTag: ((String) -> Void)? = nil,
                      refresh: @escaping () -> Void,
                      prepare: ((NoteMedia, MediaView) -> Void)? = nil) {
        guard !next.isEmpty || !mediaBlocks.isEmpty else { return }
        let oldSizes = mediaViews.map(\.preferredSize), oldProjection = mediaProjection
        let dark = effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let contrast = increasedContrast
        var reusable = mediaViews
        mediaPaintedAt = 0
        mediaBlocks = next
        mediaViews = next.map { item in
            let media: MediaView
            if let matchIndex = reusable.firstIndex(where: {
                $0.canReuse(for: item.prepared, width: width, size: size, vault: vault,
                            knowledgeVersion: knowledgeVersion, dark: dark, increasedContrast: contrast)
            }) {
                media = reusable.remove(at: matchIndex)
            } else {
                media = MediaView(item.prepared, width: width, size: size, vault: vault,
                                  knowledge: knowledge, knowledgeVersion: knowledgeVersion,
                                  dark: dark, increasedContrast: contrast)
                media.isHidden = true
                addSubview(media)
            }
            media.onRefresh = refresh
            media.onOpenLink = isReading ? onOpenLink : nil
            media.onShowTag = isReading ? onShowTag : nil
            media.onSizeChange = { [weak self, weak media] in self?.invalidateMediaLayout(for: media) }
            media.onImageDisplayed = { [weak self] in self?.recordMediaDisplay() }
            prepare?(item, media)
            if !isReading {
                media.onEdit = { [weak self, weak media] in
                    guard let self, let media, !compositionActive,
                          let index = mediaViews.firstIndex(where: { $0 === media }) else { return }
                    window?.makeFirstResponder(self)
                    setSelectedRange(NSRange(location: mediaBlocks[index].range.location, length: 0))
                    editingCoordinator?.mediaSelectionChanged()
                }
            } else {
                media.onEdit = nil
            }
            return media
        }
        reusable.forEach { $0.detach(); $0.removeFromSuperview() }
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
        guard range.location <= text.length, range.length <= text.length - range.location else { return false }
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
    func invalidateMediaLayout(for media: MediaView? = nil) {
        guard let content = textLayoutManager?.textContentManager else { return }
        let anchor = (media != nil && isReading && !isEmbedded) ? readingCoordinator?.visibleAnchor() : nil
        let localStart: Int? = media.flatMap { target in
            mediaViews.firstIndex(where: { $0 === target }).flatMap { index in
                index < mediaBlocks.count ? mediaBlocks[index].range.location : nil
            }
        }
        if let localStart, let storage = textStorage, localStart < storage.length,
           let startLocation = content.location(content.documentRange.location, offsetBy: localStart),
           let invalidRange = NSTextRange(location: startLocation, end: content.documentRange.endLocation) {
            content.performEditingTransaction {
                storage.edited(.editedAttributes, range: NSRange(location: localStart, length: storage.length - localStart), changeInLength: 0)
            }
            textLayoutManager?.invalidateLayout(for: invalidRange)
        } else {
            if let storage = textStorage, storage.length > 0 {
                content.performEditingTransaction { storage.edited(.editedAttributes, range: NSRange(location: 0, length: storage.length), changeInLength: 0) }
            }
            textLayoutManager?.invalidateLayout(for: content.documentRange)
        }
        needsLayout = true; needsDisplay = true
        if let anchor {
            layoutSubtreeIfNeeded()
            readingCoordinator?.restore(anchor)
        }
    }
}
