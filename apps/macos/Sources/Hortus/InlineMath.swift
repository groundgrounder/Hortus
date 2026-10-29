import AppKit
import CoreText

/// 附件只有一个 UTF-16 单元；原生 TextKit 2 负责换行、光标和整体选择。
@MainActor
final class InlineMathAttachment: NSTextAttachment {
    var item: NoteMath
    let availableWidth: Double
    let bodySize: Double
    let foreground: NSColor
    let naturalSize: CGSize
    let ascent: Double
    let descent: Double
    var maximumDrawMilliseconds = 0.0
    var wide: Bool { naturalSize.width > availableWidth }
    init(_ item: NoteMath, width: Double, size: Double, foreground: NSColor) {
        self.item = item; availableWidth = max(1, width); bodySize = size; self.foreground = foreground
        if let math = item.prepared.math {
            naturalSize = math.size; ascent = math.ascent; descent = math.descent
        } else {
            var up: CGFloat = 0, down: CGFloat = 0
            let width = item.prepared.fallbackLines.first.map { CTLineGetTypographicBounds($0, &up, &down, nil) } ?? 0
            naturalSize = CGSize(width: width, height: up + down); ascent = Double(up); descent = Double(down)
        }
        super.init(data: nil, ofType: nil)
        allowsTextAttachmentView = false
        bounds = CGRect(x: 0, y: -descent - (wide ? 14 : 0),
            width: min(naturalSize.width, availableWidth), height: naturalSize.height + (wide ? 14 : 0))
        attachmentCell = InlineMathCell(size: bounds.size, descent: descent + (wide ? 14 : 0))
    }
    required init?(coder: NSCoder) { nil }
    override func image(for bounds: CGRect, attributes: [NSAttributedString.Key: Any],
                        location: any NSTextLocation, textContainer: NSTextContainer?) -> NSImage? { nil }
}

/// 视图绘制矢量场景；原生附件格不再绘制缺失文件图标。
private final class InlineMathCell: NSTextAttachmentCell {
    let size: CGSize
    let descent: Double
    init(size: CGSize, descent: Double) {
        self.size = size; self.descent = descent
        super.init(textCell: "")
    }
    required init(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func cellSize() -> NSSize { size }
    override func cellBaselineOffset() -> NSPoint { NSPoint(x: 0, y: -descent) }
    override func draw(withFrame cellFrame: NSRect, in controlView: NSView?) {}
}

@MainActor
final class InlineMathView: NSView {
    let attachment: InlineMathAttachment
    lazy var scroll = NSScrollView()
    override var isFlipped: Bool { true }
    init(_ attachment: InlineMathAttachment) {
        self.attachment = attachment
        super.init(frame: CGRect(origin: .zero, size: attachment.bounds.size))
        let canvas = InlineMathCanvas(attachment)
        if attachment.wide {
            scroll.frame = bounds; scroll.autoresizingMask = [.width, .height]
            scroll.drawsBackground = false; scroll.contentView.drawsBackground = false
            scroll.borderType = .noBorder; scroll.hasHorizontalScroller = true
            scroll.hasVerticalScroller = false; scroll.autohidesScrollers = true
            scroll.documentView = canvas; addSubview(scroll)
        } else { addSubview(canvas) }
        toolTip = attachment.item.prepared.reason
        setAccessibilityElement(true); setAccessibilityRole(.image)
        setAccessibilityLabel(L10n.string("Formula: %@", attachment.item.prepared.block.latex))
        setAccessibilityHelp(attachment.item.prepared.reason)
    }
    required init?(coder: NSCoder) { nil }
    override func accessibilityChildren() -> [Any]? { [] }
    // 文字选择交给宿主；横向滚动仍由原生 scroll view 处理。
    override func hitTest(_ point: NSPoint) -> NSView? { attachment.wide ? super.hitTest(point) : nil }
}

@MainActor
private final class InlineMathCanvas: NSView {
    let attachment: InlineMathAttachment
    override var isFlipped: Bool { true }
    init(_ attachment: InlineMathAttachment) {
        self.attachment = attachment
        super.init(frame: CGRect(origin: .zero, size: attachment.naturalSize))
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { nil }
    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let began = ProcessInfo.processInfo.systemUptime
        defer { attachment.maximumDrawMilliseconds = max(attachment.maximumDrawMilliseconds, (ProcessInfo.processInfo.systemUptime - began) * 1000) }
        var ancestor = superview
        while ancestor != nil && !(ancestor is DiagramTextView) { ancestor = ancestor?.superview }
        let selected = (ancestor as? DiagramTextView)?.selectedRanges.contains {
            NSIntersectionRange($0.rangeValue, attachment.item.range).length > 0
        } ?? false
        if selected {
            NSColor.selectedTextBackgroundColor.setFill(); bounds.fill()
        } else if attachment.item.prepared.math == nil {
            NSColor.quaternaryLabelColor.withAlphaComponent(0.10).setFill(); bounds.fill()
        }
        let foreground = (selected ? NSColor.selectedTextColor : attachment.foreground).usingColorSpace(.sRGB)!.cgColor
        if let math = attachment.item.prepared.math {
            MathRenderer.draw(math, in: context, origin: .zero, foreground: foreground,
                deviceScale: Double(window?.backingScaleFactor ?? 2))
        } else if let line = attachment.item.prepared.fallbackLines.first {
            context.saveGState()
            context.setFillColor(foreground)
            context.translateBy(x: 0, y: attachment.ascent); context.scaleBy(x: 1, y: -1)
            context.textMatrix = .identity; context.textPosition = .zero
            CTLineDraw(line, context); context.restoreGState()
        }
    }
}

extension DiagramTextView {
    func installInlineMath(_ items: [NoteMath], width: Double, size: Double) {
        let previous = inlineMath
        var next: [Int: InlineMathAttachment] = [:]
        var changed: [NSRange] = []
        for item in items {
            let color = sourceStyling?.style(item.prepared.block.mask)[.foregroundColor] as? NSColor ?? .textColor
            if let old = previous[item.range.location], old.item.prepared.block.blockId == item.prepared.block.blockId,
               old.item.prepared.source == item.prepared.source, old.availableWidth == width,
               old.bodySize == size, old.foreground == color {
                old.item = item; next[item.range.location] = old
            } else {
                next[item.range.location] = InlineMathAttachment(item, width: width, size: size, foreground: color)
                changed.append(item.range)
            }
        }
        inlineMath = next
        if let storage = textStorage {
            diagramStorage?.performEditingTransaction {
                // 附件随可见段落附加；只失效一次，避免逐个更新整篇文本存储。
                if let first = changed.first, let last = changed.last, NSMaxRange(last) <= storage.length {
                    storage.edited(.editedAttributes, range: NSUnionRange(first, last), changeInLength: 0)
                }
            }
        }
    }

    func copyWithFormulas() -> Bool {
        guard isReading, let storage = textStorage else { return false }
        let items = inlineMath.values.map { $0.item } + mathViews.map { $0.item }
        guard !items.isEmpty else { return false }
        let pieces = selectedRanges.compactMap { value -> String? in
            let range = value.rangeValue
            guard range.length > 0, NSMaxRange(range) <= storage.length else { return nil }
            let result = NSMutableString(string: (storage.string as NSString).substring(with: range))
            for item in items.sorted(by: { $0.range.location > $1.range.location })
                where NSIntersectionRange(item.range, range).length > 0 {
                let local = NSRange(location: item.range.location - range.location, length: item.range.length)
                result.replaceCharacters(in: local, with: item.prepared.source)
            }
            return result as String
        }
        guard !pieces.isEmpty else { return false }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(pieces.joined(separator: "\n"), forType: .string)
        return true
    }
}
