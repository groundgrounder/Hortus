import AppKit
import CoreText
import HortusCore
import HortusApple

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
    override func accessibilityHitTest(_ point: NSPoint) -> Any? {
        let hit = MainActor.assumeIsolated { AccessibilityHitResult(value: cachedFormulaAccessibility(at: point)) }
        return hit.value ?? super.accessibilityHitTest(point)
    }
    override func mouseDown(with event: NSEvent) { owner?.onSelect?() }
    override func menu(for event: NSEvent) -> NSMenu? { owner?.menu(for: event) }
}

@MainActor
final class MathBlockView: NSView {
    var item: NoteMath
    let preferredSize: CGSize
    let scroll = FormulaScrollView()
    let bodySize: Double
    let reservedPrefixWidth: Double
    let scrollbarHeight: Double
    private let canvas: MathCanvas
    var containerPrefix: NSAttributedString?
    var quote = false
    var quoteHead = true
    var quoteTail = true
    var onSelect: (() -> Void)?
    private(set) var maximumDrawMilliseconds = 0.0
    override var isFlipped: Bool { true }
    init(_ item: NoteMath, width: Double, size: Double, prefixWidth: Double = 0, scrollerStyle: NSScroller.Style = NSScroller.preferredScrollerStyle) {
        self.item = item
        bodySize = size
        reservedPrefixWidth = prefixWidth
        canvas = MathCanvas(item.prepared, size: size)
        let availableWidth = max(80, width - prefixWidth)
        let wide = (item.prepared.math?.size.width ?? 0) > availableWidth
        scrollbarHeight = wide ? MathTypography.scrollbarHeight(style: scrollerStyle) : 0
        preferredSize = CGSize(width: width, height: (item.prepared.math.map { Double($0.size.height) } ?? item.prepared.fallbackHeight)
            + 2 * MathTypography.padding + scrollbarHeight)
        super.init(frame: CGRect(x: 0, y: 0, width: width + 12, height: preferredSize.height))
        canvas.owner = self
        toolTip = item.prepared.reason
        scroll.frame = CGRect(x: 12 + prefixWidth, y: 0, width: availableWidth, height: preferredSize.height)
        scroll.autoresizingMask = [.width, .height]
        scroll.drawsBackground = false; scroll.borderType = .noBorder
        scroll.contentView.drawsBackground = false
        scroll.scrollerStyle = scrollerStyle
        scroll.hasHorizontalScroller = wide; scroll.hasVerticalScroller = false
        scroll.autohidesScrollers = true
        canvas.frame = CGRect(x: 0, y: 0, width: max(availableWidth, item.prepared.math?.size.width ?? 0),
                              height: preferredSize.height - scrollbarHeight)
        scroll.documentView = canvas; addSubview(scroll)
        setAccessibilityElement(false); setAccessibilityRole(.image)
        setAccessibilityLabel(L10n.string("Formula: %@", item.prepared.block.latex))
        setAccessibilityHelp(item.prepared.reason)
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
    override func accessibilityHitTest(_ point: NSPoint) -> Any? {
        let hit = MainActor.assumeIsolated { AccessibilityHitResult(value: cachedFormulaAccessibility(at: point)) }
        return hit.value ?? super.accessibilityHitTest(point)
    }
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
        guard !items.isEmpty || !mathViews.isEmpty || !inlineMath.isEmpty else { return }
        let old = mathViews
        var unused = old
        var changed: [NSRange] = []
        let scrollerStyle = formulaScrollerStyle
        let scrollbarHeight = MathTypography.scrollbarHeight(style: scrollerStyle)
        let storage = textStorage?.string as NSString?
        mathViews = items.filter { $0.prepared.block.block }.map { item in
            let prefixWidth: Double
            if let storage {
                prefixWidth = MathTypography.prefixWidth(in: storage, range: item.range, size: size)
            } else {
                prefixWidth = 0
            }
            let oldIndex = unused.firstIndex { $0.item.prepared.block.blockId == item.prepared.block.blockId
                && $0.item.prepared.block.latex == item.prepared.block.latex
                && $0.preferredSize.width == width && $0.bodySize == size
                && $0.reservedPrefixWidth == prefixWidth
                && $0.scrollbarHeight == (($0.item.prepared.math?.size.width ?? 0) > max(80, width - prefixWidth) ? scrollbarHeight : 0) }
            let reused = oldIndex.map { unused.remove(at: $0) }
            let math = reused ?? MathBlockView(item, width: width, size: size, prefixWidth: prefixWidth, scrollerStyle: scrollerStyle)
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
        if let manager = textLayoutManager, let content = manager.textContentManager,
           let changedRange = changed.reduce(nil as NSRange?, { current, next in current.map { NSUnionRange($0, next) } ?? next }) {
            // 一次更新可能包含很多公式；合并失效范围，避免逐式重建原生元素缓存。
            let paragraph = storage?.paragraphRange(for: changedRange) ?? changedRange
            if let start = content.location(content.documentRange.location, offsetBy: paragraph.location),
               let end = content.location(start, offsetBy: paragraph.length),
               let range = NSTextRange(location: start, end: end) { manager.invalidateLayout(for: range) }
        }
        needsLayout = true
    }
    func mathView(at offset: Int) -> MathBlockView? {
        isReading ? mathParagraphs[offset] : nil
    }
}
