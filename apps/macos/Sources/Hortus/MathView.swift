import AppKit
import CoreText
import HortusCore

struct PreparedMathBlock: @unchecked Sendable {
    let block: MathBlock
    let source: String
    let math: PreparedMath?
    let reason: String?
    let reasonLines: [CTLine]
    let fallbackLines: [CTLine]
    let fallbackHeight: Double
}

extension MathService {
    func prepare(_ blocks: [MathBlock], source: String, size: Double, width: Double, displayText: String = "") -> [PreparedMathBlock] {
        let source = source as NSString
        let display = displayText as NSString
        return blocks.filter { $0.block && !$0.latex.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }.compactMap { block in
            let range = NSRange(location: Int(block.sourceStart), length: Int(block.sourceLength))
            guard range.location <= source.length, range.length <= source.length - range.location else { return nil }
            let raw = source.substring(with: range)
            do {
                return PreparedMathBlock(block: block, source: raw, math: try render(block.latex, style: block.style, bodySize: size),
                                         reason: nil, reasonLines: [], fallbackLines: [], fallbackHeight: 0)
            } catch {
                let detail: String
                let message: String
                if let failure = error as? MathFailure {
                    switch failure {
                    case let .ResourceLimit(value):
                        detail = value
                        message = L10n.string("Formula resource limit exceeded: %@; showing source", value)
                    case let .Parse(value), let .Panic(value), let .InvalidLayout(value):
                        detail = value
                        message = L10n.string("Formula error: %@; showing source", value)
                    }
                } else if let failure = error as? MathService.FontLoadError {
                    detail = failure.description
                    message = L10n.string("Formula fonts could not be loaded: %@; showing source", detail)
                } else {
                    detail = String(describing: error)
                    message = L10n.string("Formula error: %@; showing source", detail)
                }
                let prefixWidth = MathTypography.prefixWidth(in: display,
                    range: NSRange(location: Int(block.displayStart), length: Int(block.displayLength)), size: size)
                let lineWidth = max(80, width - prefixWidth) - 16
                func wrap(_ text: String, attributes: [NSAttributedString.Key: Any]) -> [CTLine] {
                    guard !text.isEmpty else { return [] }
                    let typesetter = CTTypesetterCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
                    let length = (text as NSString).length
                    var offset = 0, lines: [CTLine] = []
                    while offset < length {
                        let count = max(1, CTTypesetterSuggestClusterBreak(typesetter, offset, lineWidth))
                        lines.append(CTTypesetterCreateLine(typesetter, CFRange(location: offset, length: min(count, length - offset))))
                        offset += count
                    }
                    return lines
                }
                let sourceFont = NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
                let reasonFont = NSFont.systemFont(ofSize: 12)
                let contextColor = NSAttributedString.Key(kCTForegroundColorFromContextAttributeName as String)
                let reasonLines = wrap(message, attributes: [.font: reasonFont, contextColor: true])
                let lines = wrap(block.displaySource, attributes: [.font: sourceFont, contextColor: true])
                let textHeight = Double(reasonLines.count) * 16 + Double(lines.count) * (size + 5) + 16
                return PreparedMathBlock(block: block, source: raw, math: nil,
                    reason: message, reasonLines: reasonLines, fallbackLines: lines,
                    fallbackHeight: textHeight)
            }
        }
    }
}

struct NoteMath {
    let range: NSRange
    let prepared: PreparedMathBlock
}

@MainActor
private final class MathCanvas: NSView {
    let prepared: PreparedMathBlock
    let bodySize: Double
    weak var owner: MathBlockView?
    override var isFlipped: Bool { true }
    init(_ prepared: PreparedMathBlock, size: Double) {
        self.prepared = prepared; bodySize = size
        super.init(frame: .zero)
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { nil }
    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let began = ProcessInfo.processInfo.systemUptime
        defer { owner?.recordDraw(since: began) }
        let foreground = NSColor.labelColor.usingColorSpace(.sRGB)!.cgColor
        if let math = prepared.math {
            MathRenderer.draw(math, in: context,
                origin: CGPoint(x: max(0, (bounds.width - math.size.width) / 2), y: MathTypography.padding),
                foreground: foreground, deviceScale: Double(window?.backingScaleFactor ?? 2))
        } else {
            NSColor.quaternaryLabelColor.withAlphaComponent(0.07).setFill()
            bounds.insetBy(dx: 0, dy: MathTypography.padding).fill()
            var lineY = 8.0
            if !prepared.reasonLines.isEmpty {
                context.setFillColor(NSColor.secondaryLabelColor.usingColorSpace(.sRGB)!.cgColor)
                for line in prepared.reasonLines {
                    context.saveGState()
                    context.translateBy(x: 8, y: lineY + 14)
                    context.scaleBy(x: 1, y: -1)
                    context.textMatrix = .identity; context.textPosition = .zero
                    CTLineDraw(line, context)
                    context.restoreGState()
                    lineY += 16
                }
            }
            context.setFillColor(foreground)
            for (index, line) in prepared.fallbackLines.enumerated() {
                context.saveGState()
                context.translateBy(x: 8, y: lineY + bodySize + Double(index) * (bodySize + 5))
                context.scaleBy(x: 1, y: -1)
                context.textMatrix = .identity; context.textPosition = .zero
                CTLineDraw(line, context)
                context.restoreGState()
            }
        }
    }
    override func mouseDown(with event: NSEvent) { owner?.onSelect?() }
    override func menu(for event: NSEvent) -> NSMenu? { owner?.menu(for: event) }
}

@MainActor
final class MathBlockView: NSView {
    var item: NoteMath
    let preferredSize: CGSize
    let scroll = NSScrollView()
    let bodySize: Double
    let reservedPrefixWidth: Double
    private let canvas: MathCanvas
    var containerPrefix: NSAttributedString?
    var quote = false
    var quoteHead = true
    var quoteTail = true
    var onSelect: (() -> Void)?
    private(set) var maximumDrawMilliseconds = 0.0
    override var isFlipped: Bool { true }
    init(_ item: NoteMath, width: Double, size: Double, prefixWidth: Double = 0) {
        self.item = item
        bodySize = size
        reservedPrefixWidth = prefixWidth
        canvas = MathCanvas(item.prepared, size: size)
        let availableWidth = max(80, width - prefixWidth)
        let wide = (item.prepared.math?.size.width ?? 0) > availableWidth
        preferredSize = CGSize(width: width, height: (item.prepared.math.map { Double($0.size.height) } ?? item.prepared.fallbackHeight)
            + 2 * MathTypography.padding + (wide ? 14 : 0))
        super.init(frame: CGRect(x: 0, y: 0, width: width + 12, height: preferredSize.height))
        canvas.owner = self
        toolTip = item.prepared.reason
        scroll.frame = CGRect(x: 12 + prefixWidth, y: 0, width: availableWidth, height: preferredSize.height)
        scroll.autoresizingMask = [.width, .height]
        scroll.drawsBackground = false; scroll.borderType = .noBorder
        scroll.contentView.drawsBackground = false
        scroll.hasHorizontalScroller = wide; scroll.hasVerticalScroller = false
        scroll.autohidesScrollers = true
        canvas.frame = CGRect(x: 0, y: 0, width: max(availableWidth, item.prepared.math?.size.width ?? 0),
                              height: preferredSize.height - (wide ? 14 : 0))
        scroll.documentView = canvas; addSubview(scroll)
        setAccessibilityElement(true); setAccessibilityRole(.image)
        setAccessibilityLabel(L10n.string("Formula: %@", item.prepared.block.latex))
    }
    required init?(coder: NSCoder) { nil }
    override func draw(_ dirtyRect: NSRect) {
        if quote {
            NSColor.controlAccentColor.withAlphaComponent(0.22).setFill()
            CGRect(x: 0, y: quoteHead ? 2 : 0, width: 2,
                   height: max(0, bounds.height - (quoteHead ? 2 : 0) - (quoteTail ? 2 : 0))).fill()
        }
        containerPrefix?.draw(at: CGPoint(x: 12, y: max(0, (bounds.height - (containerPrefix?.size().height ?? 0)) / 2)))
    }
    override func accessibilityChildren() -> [Any]? { [] }
    override func mouseDown(with event: NSEvent) { onSelect?() }
    override func menu(for event: NSEvent) -> NSMenu? {
        onSelect?()
        let menu = NSMenu()
        let copy = NSMenuItem(title: L10n.string("Copy Markdown Source"), action: #selector(copySource(_:)), keyEquivalent: "")
        copy.target = self; menu.addItem(copy)
        return menu
    }
    @objc private func copySource(_ sender: Any?) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(item.prepared.source, forType: .string)
    }
    func recordDraw(since start: Double) {
        maximumDrawMilliseconds = max(maximumDrawMilliseconds, (ProcessInfo.processInfo.systemUptime - start) * 1000)
    }
    func capture() throws -> Data {
        guard let bitmap = canvas.bitmapImageRepForCachingDisplay(in: canvas.visibleRect) else { throw CocoaError(.fileWriteUnknown) }
        canvas.cacheDisplay(in: canvas.visibleRect, to: bitmap)
        guard let data = bitmap.representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
        return data
    }
}

extension DiagramTextView {
    func installMath(_ items: [NoteMath], width: Double, size: Double) {
        guard !items.isEmpty || !mathViews.isEmpty else { return }
        let old = mathViews
        var unused = old
        var changed: [NSRange] = []
        let storage = textStorage?.string as NSString?
        mathViews = items.map { item in
            let prefixWidth: Double
            if let storage {
                prefixWidth = MathTypography.prefixWidth(in: storage, range: item.range, size: size)
            } else {
                prefixWidth = 0
            }
            let oldIndex = unused.firstIndex { $0.item.prepared.block.blockId == item.prepared.block.blockId
                && $0.item.prepared.block.latex == item.prepared.block.latex
                && $0.preferredSize.width == width && $0.bodySize == size
                && $0.reservedPrefixWidth == prefixWidth }
            let reused = oldIndex.map { unused.remove(at: $0) }
            let math = reused ?? MathBlockView(item, width: width, size: size, prefixWidth: prefixWidth)
            if reused == nil || math.item.range != item.range { changed.append(item.range) }
            math.item = item
            math.onSelect = { [weak self] in self?.setSelectedRange(item.range) }
            if math.superview !== self { math.isHidden = true; addSubview(math) }
            return math
        }
        if let storage {
            for math in mathViews { mathParagraphs[storage.paragraphRange(for: math.item.range).location] = math }
        }
        old.filter { previous in !mathViews.contains { $0 === previous } }.forEach { $0.removeFromSuperview() }
        if let manager = textLayoutManager, let content = manager.textContentManager {
            for range in changed {
                let paragraph = storage?.paragraphRange(for: range) ?? range
                if let start = content.location(content.documentRange.location, offsetBy: paragraph.location),
                   let end = content.location(start, offsetBy: paragraph.length),
                   let range = NSTextRange(location: start, end: end) { manager.invalidateLayout(for: range) }
            }
        }
        needsLayout = true
    }
    func mathView(at offset: Int) -> MathBlockView? {
        isReading ? mathParagraphs[offset] : nil
    }
}
