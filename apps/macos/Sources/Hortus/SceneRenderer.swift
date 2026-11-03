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

enum DiagramSceneFailure: Error {
    case schema(UInt32)
    case sourceRange
    case staleRequest
}

struct DiagramScene: Sendable {
    let identity: DiagramIdentity
    let graphMetrics: DiagramGraphMetrics?
    let width, height: Double
    let primitives: [ScenePrimitive]
    let hitRegions: [SceneHit]
    init(_ scene: SceneData) throws {
        guard scene.schemaVersion == 2 else { throw DiagramSceneFailure.schema(scene.schemaVersion) }
        identity = scene.identity; graphMetrics = scene.graphMetrics
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
    let sourceRanges: [String: NSRange]
    let edgeRanges: [UInt64: NSRange]
    let edgeLabels: [UInt64: String]
    let acceptsInteraction: Bool
    init(scene: DiagramScene, payloadBytes: Int = 0, sourceRanges: [String: NSRange] = [:], edgeRanges: [UInt64: NSRange] = [:], edgeLabels: [UInt64: String] = [:], lineProvider: (SceneText) -> CTLine = SceneRenderer.line) {
        self.scene = scene
        self.payloadBytes = payloadBytes
        self.sourceRanges = sourceRanges; self.edgeRanges = edgeRanges; self.edgeLabels = edgeLabels
        acceptsInteraction = true
        lines = scene.primitives.compactMap { primitive in
            if case let .text(span) = primitive { return (span, lineProvider(span)) }
            return nil
        }
    }
    private init(previous: PreparedScene) {
        scene = previous.scene; lines = previous.lines; payloadBytes = previous.payloadBytes
        sourceRanges = [:]; edgeRanges = [:]; edgeLabels = previous.edgeLabels
        acceptsInteraction = false
    }
    func withoutSourceInteraction() -> PreparedScene { PreparedScene(previous: self) }
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
        let font = font(for: span)
        var attributes: [NSAttributedString.Key: Any] = [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): span.color.cgColor,
        ]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: span.text, attributes: attributes))
        if span.fontFamily == "Apple Color Emoji" {
            // CoreText 自动放大 sbix 字体；用字体矩阵恢复 Rust 所用的 OpenType em 度量。
            let width = CTLineGetTypographicBounds(line, nil, nil, nil)
            if width > 0, span.width > 0 {
                let scale = span.width / width
                var matrix = CGAffineTransform(scaleX: scale, y: scale)
                attributes[NSAttributedString.Key(kCTFontAttributeName as String)] = CTFontCreateCopyWithAttributes(font, span.fontSize, &matrix, nil)
                return CTLineCreateWithAttributedString(NSAttributedString(string: span.text, attributes: attributes))
            }
        }
        return line
    }

    private static func path(_ points: [ScenePoint], closed: Bool = false) -> CGPath {
        let path = CGMutablePath()
        if let first = points.first {
            path.move(to: first.cgPoint)
            for point in points.dropFirst() { path.addLine(to: point.cgPoint) }
            if closed { path.closeSubpath() }
        }
        return path
    }

    private static func roundedPath(_ points: [ScenePoint], radius: Double) -> CGPath {
        let result = CGMutablePath()
        guard let first = points.first else { return result }
        result.move(to: first.cgPoint)
        if points.count > 2 {
            for i in 1..<(points.count - 1) {
                let a = points[i - 1], b = points[i], c = points[i + 1]
                let incoming = hypot(b.x - a.x, b.y - a.y), outgoing = hypot(c.x - b.x, c.y - b.y)
                let r = min(radius, incoming / 2, outgoing / 2)
                if r > 0.001 {
                    result.addLine(to: CGPoint(x: b.x + (a.x - b.x) * r / incoming, y: b.y + (a.y - b.y) * r / incoming))
                    result.addQuadCurve(to: CGPoint(x: b.x + (c.x - b.x) * r / outgoing, y: b.y + (c.y - b.y) * r / outgoing), control: b.cgPoint)
                } else { result.addLine(to: b.cgPoint) }
            }
        }
        if let last = points.last { result.addLine(to: last.cgPoint) }
        return result
    }

    private static func marker(_ kind: String, points: [ScenePoint], atStart: Bool, in context: CGContext) {
        guard kind != "None", points.count >= 2 else { return }
        let tip = atStart ? points[0] : points[points.count - 1]
        let next = atStart ? points[1] : points[points.count - 2]
        let length = hypot(tip.x - next.x, tip.y - next.y)
        guard length > 0 else { return }
        let ux = (tip.x - next.x) / length, uy = (tip.y - next.y) / length
        switch kind {
        case "Arrow":
            context.move(to: tip.cgPoint)
            context.addLine(to: CGPoint(x: tip.x - ux * 9 - uy * 4, y: tip.y - uy * 9 + ux * 4))
            context.addLine(to: CGPoint(x: tip.x - ux * 9 + uy * 4, y: tip.y - uy * 9 - ux * 4))
            context.closePath(); context.fillPath()
        case "Circle":
            let center = CGPoint(x: tip.x - ux * 4.25, y: tip.y - uy * 4.25)
            context.setLineWidth(1.5)
            context.addEllipse(in: CGRect(x: center.x - 3.5, y: center.y - 3.5, width: 7, height: 7))
            context.drawPath(using: .fillStroke)
        case "Cross":
            let cx = tip.x - ux * 5, cy = tip.y - uy * 5
            context.setLineWidth(1.5)
            for sign in [-1.0, 1.0] {
                context.move(to: CGPoint(x: cx - ux * 3 - uy * 3 * sign, y: cy - uy * 3 + ux * 3 * sign))
                context.addLine(to: CGPoint(x: cx + ux * 3 + uy * 3 * sign, y: cy + uy * 3 - ux * 3 * sign))
            }
            context.strokePath()
        default: break
        }
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
            case let .graphShape(rect, _, outline, details, fill, stroke, width):
                guard rect.cgRect.insetBy(dx: -width, dy: -width).intersects(clip) else { continue }
                context.setFillColor(fill.cgColor)
                context.setStrokeColor(stroke.cgColor)
                context.setLineWidth(width)
                context.setLineDash(phase: 0, lengths: [])
                context.addPath(path(outline, closed: true))
                context.drawPath(using: .fillStroke)
                for points in details { context.addPath(path(points)); context.strokePath() }
            case let .labelBackground(_, rect, color):
                guard rect.cgRect.intersects(clip) else { continue }
                context.setFillColor(color.cgColor)
                context.fill(rect.cgRect)
            case let .graphPath(_, radius, points, _, _, _, _, line, start, end, color, width):
                guard let first = points.first else { continue }
                let bounds = points.reduce(CGRect(origin: first.cgPoint, size: .zero)) { bounds, point in
                    bounds.union(CGRect(origin: point.cgPoint, size: .zero))
                }.insetBy(dx: -12, dy: -12)
                guard bounds.intersects(clip) else { continue }
                context.setStrokeColor(color.cgColor)
                context.setFillColor(color.cgColor)
                context.setLineWidth(width)
                context.setLineJoin(.round)
                context.setLineCap(.round)
                context.setLineDash(phase: 0, lengths: line == "Dotted" ? [4, 4] : [])
                context.addPath(roundedPath(points, radius: radius))
                context.strokePath()
                context.setLineDash(phase: 0, lengths: [])
                marker(start, points: points, atStart: true, in: context)
                marker(end, points: points, atStart: false, in: context)
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
