import AppKit
import CoreText

/// 表格使用原生富文本绘制，附件格仅把同一个公式场景画到单元格基线上。
final class TableMathCell: NSTextAttachmentCell {
    let prepared: PreparedMathBlock
    let foreground: NSColor
    let measuredSize: CGSize
    let descent: CGFloat
    init(_ prepared: PreparedMathBlock, color: NSColor) {
        self.prepared = prepared; foreground = color
        if let math = prepared.math {
            measuredSize = math.size; descent = math.descent
        } else {
            var ascent: CGFloat = 0, depth: CGFloat = 0
            let width = prepared.fallbackLines.first.map { CTLineGetTypographicBounds($0, &ascent, &depth, nil) } ?? 0
            measuredSize = CGSize(width: width, height: ascent + depth); descent = depth
        }
        super.init(textCell: "")
    }
    required init(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func cellSize() -> NSSize { measuredSize }
    override func cellBaselineOffset() -> NSPoint { NSPoint(x: 0, y: -descent) }
    override func draw(withFrame cellFrame: NSRect, in controlView: NSView?) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let color = foreground.usingColorSpace(.sRGB)!.cgColor
        if let math = prepared.math {
            MathRenderer.draw(math, in: context, origin: cellFrame.origin, foreground: color,
                deviceScale: Double(controlView?.window?.backingScaleFactor ?? 2))
        } else if let line = prepared.fallbackLines.first {
            context.saveGState(); context.setFillColor(color)
            context.translateBy(x: cellFrame.minX, y: cellFrame.minY + measuredSize.height - descent)
            context.scaleBy(x: 1, y: -1); context.textMatrix = .identity; context.textPosition = .zero
            CTLineDraw(line, context); context.restoreGState()
        }
    }
}
