import AppKit
import CoreText
import HortusCore

typealias DiagramText = SceneText

extension ScenePoint {
    var cgPoint: CGPoint { CGPoint(x: x, y: y) }
}
extension SceneRect {
    var cgRect: CGRect { CGRect(x: x, y: y, width: width, height: height) }
}
extension SceneColor {
    var cgColor: CGColor { CGColor(srgbRed: r, green: g, blue: b, alpha: a) }
}
extension SceneHit {
    var accessibilityLabel: String {
        [prefix, label.replacingOccurrences(of: "<br\\s*/?>", with: "，", options: [.regularExpression, .caseInsensitive])]
            .compactMap { $0 }.joined(separator: "，")
    }
}
typealias DiagramHit = SceneHit

struct DiagramScene: Sendable {
    let width, height: Double
    let primitives: [ScenePrimitive]
    let hitRegions: [SceneHit]
    init(_ scene: SceneData) {
        width = scene.width; height = scene.height
        primitives = scene.primitives; hitRegions = scene.hitRegions
    }
}

/// 缓存键不含排版坐标；同一段文字改变位置后继续复用不可变 CTLine。
struct DiagramLineKey: Hashable {
    let text, family: String
    let size: Double
    let weight: UInt16
    let color: SceneColor
    init(_ span: SceneText) {
        text = span.text; family = span.fontFamily; size = span.fontSize; weight = span.fontWeight; color = span.color
    }
}

/// CoreText 对象不可变，可在后台准备，主线程只提交绘制指令。
final class PreparedScene: @unchecked Sendable {
    let scene: DiagramScene
    let lines: [(DiagramText, CTLine)]
    let payloadBytes: Int
    init(scene: DiagramScene, payloadBytes: Int = 0, lineProvider: (SceneText) -> CTLine = SceneRenderer.line) {
        self.scene = scene
        self.payloadBytes = payloadBytes
        lines = scene.primitives.compactMap { primitive in
            if case let .text(span) = primitive { return (span, lineProvider(span)) }
            return nil
        }
    }
}

/// 不访问视图、文件或全局状态；所有坐标来自内核，原点为左上角。
enum SceneRenderer {
    static func font(for span: DiagramText) -> CTFont {
        let font: CTFont
        if span.fontFamily == "System Font" || span.fontFamily == ".AppleSystemUIFont" {
            font = CTFontCreateUIFontForLanguage(.system, span.fontSize, nil)!
        } else if span.fontFamily == "PingFang SC" {
            let suffix = span.fontWeight >= 550 ? "Semibold" : (span.fontWeight >= 450 ? "Medium" : "Regular")
            font = CTFontCreateWithName("PingFangSC-\(suffix)" as CFString, span.fontSize, nil)
        } else {
            font = CTFontCreateWithName(span.fontFamily as CFString, span.fontSize, nil)
        }
        // 与 Rust 的 OpenType wght / opsz 设置一致，避免系统默认光学尺寸带来的字宽差。
        let variations: [NSNumber: NSNumber] = [0x77676874: NSNumber(value: span.fontWeight),
                                               0x6f70737a: NSNumber(value: span.fontSize)]
        let descriptor = CTFontDescriptorCreateWithAttributes([
            kCTFontVariationAttribute: variations,
            // 单独保留 opsz 轴，关闭 CoreText 额外的尺寸相关字距度量。
            kCTFontOpticalSizeAttribute: 0,
            kCTFontCascadeListAttribute: [CTFontDescriptorCreateWithNameAndSize("PingFang SC" as CFString, span.fontSize)],
        ] as CFDictionary)
        return CTFontCreateCopyWithAttributes(font, span.fontSize, nil, descriptor)
    }

    static func line(for span: DiagramText) -> CTLine {
        let attributes: [NSAttributedString.Key: Any] = [
            NSAttributedString.Key(kCTFontAttributeName as String): font(for: span),
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): span.color.cgColor,
        ]
        return CTLineCreateWithAttributedString(NSAttributedString(string: span.text, attributes: attributes))
    }

    static func draw(_ prepared: PreparedScene, in context: CGContext) {
        context.saveGState()
        let clip = context.boundingBoxOfClipPath
        for primitive in prepared.scene.primitives {
            switch primitive {
            case let .roundedRect(rect, radius, color):
                guard rect.cgRect.intersects(clip) else { continue }
                context.setFillColor(color.cgColor)
                context.addPath(CGPath(roundedRect: rect.cgRect, cornerWidth: radius, cornerHeight: radius, transform: nil))
                context.fillPath()
            case let .line(from, to, width, color):
                let bounds = CGRect(x: min(from.x, to.x), y: min(from.y, to.y),
                                    width: abs(to.x - from.x), height: abs(to.y - from.y)).insetBy(dx: -width, dy: -width)
                guard bounds.intersects(clip) else { continue }
                context.setStrokeColor(color.cgColor)
                context.setLineWidth(width)
                context.move(to: from.cgPoint)
                context.addLine(to: to.cgPoint)
                context.strokePath()
            case .text: break
            }
        }
        context.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        for (span, line) in prepared.lines {
            // 留足上下延伸与边缘字形的余量；长图只提交视口内的文字。
            let bounds = CGRect(x: span.baseline.x - 2, y: span.baseline.y - span.fontSize * 1.5,
                                width: span.width + 4, height: span.fontSize * 2)
            guard bounds.intersects(clip) else { continue }
            context.textPosition = CGPoint(x: span.baseline.x, y: span.baseline.y)
            CTLineDraw(line, context)
        }
        context.restoreGState()
    }
}
