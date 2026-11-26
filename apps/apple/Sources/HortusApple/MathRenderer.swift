import CoreGraphics
import CoreText
import Foundation
import HortusCore
#if canImport(AppKit)
import AppKit
#endif

public enum MathTypography {
    public static let scale = mathTypographyScale()
    #if canImport(AppKit)
    @MainActor public static func scrollbarHeight(style: NSScroller.Style = NSScroller.preferredScrollerStyle) -> Double {
        style == .legacy ? NSScroller.scrollerWidth(for: .regular, scrollerStyle: style) : 0
    }
    #else
    @MainActor public static func scrollbarHeight() -> Double { 0 }
    #endif
    public static let padding = 8.0
    public static func bodySize(_ size: Double, mask: UInt32) -> Double {
        size * ([1: 1.6, 2: 1.35, 3: 1.15][Int(mask >> 24)] ?? 1)
    }

    public static func prefixWidth(in text: NSString, range: NSRange, size: Double) -> Double {
        PlatformGraphics.measurePrefixWidth(in: text, range: range, size: size)
    }
}

public struct MathScene: Decodable, Sendable {
    public struct Color: Decodable, Sendable {
        public let foreground: Bool
        public let rgba: [Double]
    }
    public struct Command: Decodable, Sendable {
        public let kind: String
        public let x, y, x1, y1, x2, y2: Double?
    }
    public struct Item: Decodable, Sendable {
        public let kind: String
        public let x, y: Double
        public let scale, width, height, thickness: Double?
        public let font: String?
        public let codepoint: UInt32?
        public let fallback, dashed, fill: Bool?
        public let commands: [Command]?
        public let color: Color
    }
    public let width, height, depth: Double
    public let items: [Item]
    public let missingMetrics: [UInt32]

    public init(_ bytes: Data) throws {
        guard bytes.count >= 8, bytes.count <= 32 * 1024 * 1024 else { throw CocoaError(.fileReadCorruptFile) }
        let header = bytes.withUnsafeBytes { raw in
            (UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 0, as: UInt32.self)),
             UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 4, as: UInt32.self)))
        }
        guard header.0 == 1, Int(header.1) == bytes.count - 8 else { throw CocoaError(.fileReadCorruptFile) }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        self = try decoder.decode(Self.self, from: bytes.dropFirst(8))
        func finite(_ values: [Double?]) -> Bool {
            values.allSatisfy { $0.map { $0.isFinite && abs($0) <= 4096 } == true }
        }
        var commandCount = 0
        guard finite([width, height, depth]), width >= 0, height >= 0, depth >= 0, items.count <= 32768,
              missingMetrics.allSatisfy({ Unicode.Scalar($0) != nil }),
              items.allSatisfy({ item in
                  guard finite([item.x, item.y]), item.color.rgba.count == 4,
                        item.color.rgba.allSatisfy({ $0.isFinite && (0...1).contains($0) }) else { return false }
                  switch item.kind {
                  case "Glyph": return finite([item.scale]) && item.scale! > 0 && item.font != nil
                      && item.codepoint.flatMap(Unicode.Scalar.init) != nil && item.fallback != nil
                  case "Line": return finite([item.width, item.thickness]) && item.width! >= 0
                      && item.thickness! >= 0 && item.dashed != nil
                  case "Rect": return finite([item.width, item.height]) && item.width! >= 0 && item.height! >= 0
                  case "Path":
                      guard let commands = item.commands, item.fill != nil else { return false }
                      commandCount += commands.count
                      return commandCount <= 131072 && commands.allSatisfy { command in
                          switch command.kind {
                          case "MoveTo", "LineTo": return finite([command.x, command.y])
                          case "CubicTo": return finite([command.x, command.y, command.x1, command.y1, command.x2, command.y2])
                          case "QuadTo": return finite([command.x, command.y, command.x1, command.y1])
                          case "Close": return true
                          default: return false
                          }
                      }
                  default: return false
                  }
              }) else {
            throw CocoaError(.fileReadCorruptFile)
        }
    }
}

public struct PreparedMath: @unchecked Sendable {
    public enum Drawing {
        case glyph(CTFont, CGGlyph, CGPoint, Double, MathScene.Color)
        case line(CGPoint, Double, Double, Bool, MathScene.Color)
        case rect(CGRect, MathScene.Color)
        case path(CGPath, Bool, MathScene.Color)
    }
    public let size: CGSize
    public let ascent: Double
    public let descent: Double
    public let drawing: [Drawing]
    public let glyphCount: Int
    public let fallbackGlyphCount: Int
    public let unresolvedGlyphCount: Int
    public let unresolvedMetricCount: Int
    public let maximumMetricError: Double
    public let estimatedBytes: Int
}

public struct PreparedMathBlock: @unchecked Sendable {
    public let block: MathBlock
    public let source: String
    public let math: PreparedMath?
    public let reason: String?
    public let reasonLines: [CTLine]
    public let fallbackLines: [CTLine]
    public let fallbackHeight: Double

    public init(block: MathBlock, source: String, math: PreparedMath?, reason: String?,
                reasonLines: [CTLine], fallbackLines: [CTLine], fallbackHeight: Double) {
        self.block = block
        self.source = source
        self.math = math
        self.reason = reason
        self.reasonLines = reasonLines
        self.fallbackLines = fallbackLines
        self.fallbackHeight = fallbackHeight
    }
}

public struct MathIdentity: Hashable, Sendable {
    public let block: UInt64
    public let offset: Int
    public init(block: UInt64, offset: Int) {
        self.block = block
        self.offset = offset
    }
}

public struct NoteMath: @unchecked Sendable {
    public let range: NSRange
    public let prepared: PreparedMathBlock
    public let offset: Int
    public var identity: MathIdentity { MathIdentity(block: prepared.block.blockId, offset: offset) }
    public init(range: NSRange, prepared: PreparedMathBlock, offset: Int) {
        self.range = range
        self.prepared = prepared
        self.offset = offset
    }
}

/// 场景已在后台解析、选字体和建路径；这里只绘制，不分配字体或进行排版。
public enum MathRenderer {
    public static func draw(_ math: PreparedMath, in context: CGContext, origin: CGPoint, foreground: CGColor, deviceScale: Double) {
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        func color(_ value: MathScene.Color) -> CGColor {
            value.foreground ? foreground : CGColor(colorSpace: space, components: value.rgba.map { CGFloat($0) })!
        }
        context.saveGState()
        context.translateBy(x: origin.x, y: origin.y)
        for drawing in math.drawing {
            switch drawing {
            case let .glyph(font, glyph, point, factor, tint):
                context.setFillColor(color(tint))
                context.saveGState()
                context.translateBy(x: point.x, y: point.y)
                context.scaleBy(x: factor, y: -factor)
                context.textMatrix = .identity; context.textPosition = .zero
                var glyph = glyph, position = CGPoint.zero
                CTFontDrawGlyphs(font, &glyph, &position, 1, context)
                context.restoreGState()
            case let .line(point, width, thickness, dashed, tint):
                context.setFillColor(color(tint))
                context.setStrokeColor(color(tint))
                let thickness = max(thickness, 1 / max(1, deviceScale))
                if dashed {
                    let dash = 4 * thickness
                    context.setLineWidth(thickness)
                    context.setLineDash(phase: 0, lengths: [dash, dash])
                    context.move(to: point)
                    context.addLine(to: CGPoint(x: point.x + width, y: point.y))
                    context.strokePath()
                    context.setLineDash(phase: 0, lengths: [])
                } else {
                    context.fill(CGRect(x: point.x, y: point.y - thickness / 2, width: width, height: thickness))
                }
            case let .rect(rect, tint):
                context.setFillColor(color(tint)); context.fill(rect)
            case let .path(path, fill, tint):
                context.setFillColor(color(tint)); context.setStrokeColor(color(tint))
                context.addPath(path)
                if fill { context.fillPath() } else { context.setLineWidth(1.5); context.strokePath() }
            }
        }
        context.restoreGState()
    }
}

/// 按需初始化。字体注册、宿主测量、首次排版和绘制准备都在 actor 执行器上。
public actor MathService {
    public static let shared = MathService()
    private let engine = MathEngine()
    private let resourceDirectory: URL?
    private var faces: [String: String] = [:]
    private var registered = false
    private var registrationFailure: Error?
    private var fonts: [String: CTFont] = [:]
    private var fallbackFonts: [UInt32: CTFont] = [:]
    private var metrics: [UInt32: MathGlyphMetrics] = [:]
    private var cached: [String: PreparedMath] = [:]
    private var order: [String] = []
    private var cachedBytes = 0
    private let cacheLimit = 16 * 1024 * 1024
    public private(set) var mainThreadPreparations = 0
    public private(set) var layoutCount = 0
    public private(set) var registrationCount = 0
    public private(set) var measurementCount = 0
    public var measuredFallbackCodepoints: [UInt32] { metrics.keys.sorted() }

    private static let fontFiles: Set<String> = [
        "KaTeX_AMS-Regular.ttf", "KaTeX_Caligraphic-Bold.ttf", "KaTeX_Caligraphic-Regular.ttf",
        "KaTeX_Fraktur-Bold.ttf", "KaTeX_Fraktur-Regular.ttf", "KaTeX_Main-Bold.ttf",
        "KaTeX_Main-BoldItalic.ttf", "KaTeX_Main-Italic.ttf", "KaTeX_Main-Regular.ttf",
        "KaTeX_Math-BoldItalic.ttf", "KaTeX_Math-Italic.ttf", "KaTeX_SansSerif-Bold.ttf",
        "KaTeX_SansSerif-Italic.ttf", "KaTeX_SansSerif-Regular.ttf", "KaTeX_Script-Regular.ttf",
        "KaTeX_Size1-Regular.ttf", "KaTeX_Size2-Regular.ttf", "KaTeX_Size3-Regular.ttf",
        "KaTeX_Size4-Regular.ttf", "KaTeX_Typewriter-Regular.ttf"
    ]

    public struct FontLoadError: Error, CustomStringConvertible {
        public let cause: Error
        public var description: String { String(describing: cause) }
    }

    public init(resourceDirectory: URL? = nil) { self.resourceDirectory = resourceDirectory }

    public func clearCache() {
        cached.removeAll(keepingCapacity: false)
        order.removeAll(keepingCapacity: false)
        cachedBytes = 0
    }

    private func register() throws {
        guard !registered else { return }
        if let registrationFailure { throw registrationFailure }
        do { try registerFonts() } catch {
            let failure = FontLoadError(cause: error)
            registrationFailure = failure
            throw failure
        }
    }
    private func registerFonts() throws {
        let candidateDirectory: URL?
        if let resourceDirectory {
            candidateDirectory = resourceDirectory
        } else if let bundled = Bundle.main.resourceURL?.appendingPathComponent("Fonts/KaTeX"),
                  FileManager.default.fileExists(atPath: bundled.path) {
            candidateDirectory = bundled
        } else {
            candidateDirectory = Bundle.main.resourceURL
        }
        guard let directory = candidateDirectory else {
            throw CocoaError(.fileNoSuchFile)
        }
        let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "ttf" && $0.lastPathComponent.hasPrefix("KaTeX_") }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
        guard Set(files.map(\.lastPathComponent)) == Self.fontFiles else { throw CocoaError(.fileNoSuchFile) }
        for url in files {
            var error: Unmanaged<CFError>?
            if !CTFontManagerRegisterFontsForURL(url as CFURL, .process, &error) {
                guard let failure = error?.takeRetainedValue() else { throw CocoaError(.fileReadCorruptFile) }
                // 同进程中的测试服务可以复用已经注册的同一字体。
                guard CFErrorGetCode(failure) == CTFontManagerError.alreadyRegistered.rawValue else { throw failure }
            }
            guard let descriptors = CTFontManagerCreateFontDescriptorsFromURL(url as CFURL) as? [CTFontDescriptor],
                  let descriptor = descriptors.first,
                  let name = CTFontDescriptorCopyAttribute(descriptor, kCTFontNameAttribute) as? String else {
                throw CocoaError(.fileReadCorruptFile)
            }
            let key = String(url.deletingPathExtension().lastPathComponent.dropFirst("KaTeX_".count))
            faces[key] = name
        }
        registered = true; registrationCount += 1
        _ = try engine.render(source: "x^2", style: .display)
    }
    private func glyph(_ face: CTFont, _ codepoint: UInt32) throws -> CGGlyph {
        guard let scalar = Unicode.Scalar(codepoint) else { throw CocoaError(.fileReadCorruptFile) }
        var chars = Array(String(scalar).utf16), glyphs = Array(repeating: CGGlyph(0), count: String(scalar).utf16.count)
        _ = CTFontGetGlyphsForCharacters(face, &chars, &glyphs, chars.count)
        guard let glyph = glyphs.first(where: { $0 != 0 }) else { throw CocoaError(.fileReadCorruptFile) }
        return glyph
    }
    private func fallback(_ codepoint: UInt32) throws -> CTFont {
        if let font = fallbackFonts[codepoint] { return font }
        guard let scalar = Unicode.Scalar(codepoint), let system = CTFontCreateUIFontForLanguage(.system, 1000, nil) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let text = String(scalar)
        let font = CTFontCreateForString(system, text as CFString, CFRange(location: 0, length: text.utf16.count))
        fallbackFonts[codepoint] = font
        return font
    }
    private func measure(_ codepoint: UInt32, at size: Double) throws -> MathGlyphMetrics {
        if let value = metrics[codepoint] { return value }
        let font = try self.font("", codepoint: codepoint, fallback: true, size: size)
        let found = try? glyph(font, codepoint)
        var advance = CGSize.zero, box = CGRect.zero
        if var glyph = found {
            CTFontGetAdvancesForGlyphs(font, .horizontal, &glyph, &advance, 1)
            CTFontGetBoundingRectsForGlyphs(font, .horizontal, &glyph, &box, 1)
        }
        let value = MathGlyphMetrics(codepoint: codepoint, width: advance.width / size,
                                    height: max(0, box.maxY) / size, depth: max(0, -box.minY) / size)
        metrics[codepoint] = value; measurementCount += 1
        return value
    }
    private func font(_ key: String, codepoint: UInt32, fallback useFallback: Bool, size: Double) throws -> CTFont {
        let cacheKey = "\(useFallback ? String(codepoint) : key):\(size)"
        if let font = fonts[cacheKey] { return font }
        let font: CTFont
        if useFallback {
            font = CTFontCreateCopyWithAttributes(try fallback(codepoint), size, nil, nil)
        } else {
            guard let name = faces[key] else { throw CocoaError(.fileReadCorruptFile) }
            font = CTFontCreateWithName(name as CFString, size, nil)
        }
        if fonts.count >= 4096 { fonts.removeAll(keepingCapacity: true) }
        fonts[cacheKey] = font
        return font
    }
    public func render(_ latex: String, style: MathStyle, bodySize: Double) throws -> PreparedMath {
        if Thread.isMainThread { mainThreadPreparations += 1 }
        try register()
        let key = "\(style):\(bodySize):\(latex)"
        if let value = cached[key] { return value }
        layoutCount += 1
        var scene = try MathScene(engine.render(source: latex, style: style))
        if !scene.missingMetrics.isEmpty {
            let fallbackScales = Dictionary(scene.items.compactMap { item -> (UInt32, Double)? in
                guard item.kind == "Glyph", item.fallback == true, let codepoint = item.codepoint,
                      let scale = item.scale else { return nil }
                return (codepoint, scale)
            }, uniquingKeysWith: { first, _ in first })
            try engine.setMetrics(metrics: scene.missingMetrics.map { codepoint in
                let scale = fallbackScales[codepoint] ?? 1
                return try measure(codepoint, at: bodySize * MathTypography.scale * scale)
            })
            scene = try MathScene(engine.render(source: latex, style: style))
        }
        let em = bodySize * MathTypography.scale
        var drawing: [PreparedMath.Drawing] = [], glyphCount = 0, fallbackGlyphCount = 0
        var unresolvedGlyphCount = 0, metricError = 0.0
        var pathBytes = 0
        for item in scene.items {
            let point = CGPoint(x: item.x * em, y: item.y * em)
            switch item.kind {
            case "Glyph":
                guard let codepoint = item.codepoint, let key = item.font, let scale = item.scale,
                      let markedFallback = item.fallback else { throw CocoaError(.fileReadCorruptFile) }
                var useFallback = markedFallback
                var selectedFont: CTFont
                var resolvedGlyph: CGGlyph?
                if markedFallback {
                    selectedFont = try font(key, codepoint: codepoint, fallback: true, size: scale * em)
                    resolvedGlyph = try? glyph(selectedFont, codepoint)
                } else {
                    selectedFont = try font(key, codepoint: codepoint, fallback: false, size: scale * em)
                    do { resolvedGlyph = try glyph(selectedFont, codepoint) }
                    catch {
                        useFallback = true
                        selectedFont = try font(key, codepoint: codepoint, fallback: true, size: scale * em)
                        resolvedGlyph = try? glyph(selectedFont, codepoint)
                    }
                }
                guard var glyph = resolvedGlyph else { unresolvedGlyphCount += 1; continue }
                drawing.append(.glyph(selectedFont, glyph, point, 1, item.color)); glyphCount += 1
                if useFallback { fallbackGlyphCount += 1 }
                if useFallback, let metric = metrics[codepoint] {
                    var advance = CGSize.zero
                    CTFontGetAdvancesForGlyphs(selectedFont, .horizontal, &glyph, &advance, 1)
                    metricError = max(metricError, abs(advance.width - metric.width * scale * em))
                }
            case "Line":
                drawing.append(.line(point, (item.width ?? 0) * em, (item.thickness ?? 0) * em, item.dashed ?? false, item.color))
            case "Rect":
                drawing.append(.rect(CGRect(origin: point, size: CGSize(width: (item.width ?? 0) * em, height: (item.height ?? 0) * em)), item.color))
            case "Path":
                let path = CGMutablePath()
                pathBytes += (item.commands?.count ?? 0) * 7 * MemoryLayout<Double>.stride
                for command in item.commands ?? [] {
                    func p(_ x: Double?, _ y: Double?) -> CGPoint { CGPoint(x: point.x + (x ?? 0) * em, y: point.y + (y ?? 0) * em) }
                    switch command.kind {
                    case "MoveTo": path.move(to: p(command.x, command.y))
                    case "LineTo": path.addLine(to: p(command.x, command.y))
                    case "CubicTo": path.addCurve(to: p(command.x, command.y), control1: p(command.x1, command.y1), control2: p(command.x2, command.y2))
                    case "QuadTo": path.addQuadCurve(to: p(command.x, command.y), control: p(command.x1, command.y1))
                    case "Close": path.closeSubpath()
                    default: throw CocoaError(.fileReadCorruptFile)
                    }
                }
                drawing.append(.path(path, item.fill ?? false, item.color))
            default: throw CocoaError(.fileReadCorruptFile)
            }
        }
        let value = PreparedMath(size: CGSize(width: scene.width * em, height: (scene.height + scene.depth) * em),
                                 ascent: scene.height * em, descent: scene.depth * em,
                                 drawing: drawing, glyphCount: glyphCount, fallbackGlyphCount: fallbackGlyphCount,
                                 unresolvedGlyphCount: unresolvedGlyphCount, unresolvedMetricCount: scene.missingMetrics.count,
                                 maximumMetricError: metricError,
                                 estimatedBytes: drawing.count * MemoryLayout<PreparedMath.Drawing>.stride + pathBytes + key.utf8.count)
        // Core Text 首次绘制还会准备字形缓存；在后台完成，避免首次滚入时阻塞主线程。
        if let context = CGContext(data: nil, width: 128, height: 128, bitsPerComponent: 8, bytesPerRow: 0,
                                   space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                   bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) {
            context.translateBy(x: 0, y: 128); context.scaleBy(x: 2, y: -2)
            MathRenderer.draw(value, in: context, origin: .zero,
                              foreground: CGColor(gray: 0, alpha: 1), deviceScale: 2)
        }
        if value.estimatedBytes <= cacheLimit {
            while order.count >= 128 || cachedBytes + value.estimatedBytes > cacheLimit {
                if let removed = cached.removeValue(forKey: order.removeFirst()) { cachedBytes -= removed.estimatedBytes }
            }
            cached[key] = value; order.append(key); cachedBytes += value.estimatedBytes
        }
        return value
    }

    public func prepare(_ blocks: [MathBlock], source: String, size: Double, width: Double, displayText: String,
                        previous: [PreparedMathBlock] = [], cell: Bool = false) -> [PreparedMathBlock] {
        let source = source as NSString
        let display = displayText as NSString
        var retained: [String: PreparedMathBlock] = [:]
        for item in previous {
            retained["\(item.block.style):\(item.block.mask):\(item.block.latex)"] = item
        }
        return blocks.filter { !$0.latex.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }.compactMap { block in
            let range = NSRange(location: Int(block.sourceStart), length: Int(block.sourceLength))
            guard range.location <= source.length, range.length <= source.length - range.location else { return nil }
            let raw = source.substring(with: range)
            let size = cell ? size : MathTypography.bodySize(size, mask: block.mask)
            do {
                let key = "\(block.style):\(block.mask):\(block.latex)"
                if let old = retained[key], old.math == nil, !block.block {
                    return PreparedMathBlock(block: block, source: raw, math: nil,
                        reason: old.reason, reasonLines: old.reasonLines, fallbackLines: old.fallbackLines,
                        fallbackHeight: old.fallbackHeight)
                }
                let math = try retained[key]?.math ?? render(block.latex, style: block.style, bodySize: size)
                return PreparedMathBlock(block: block, source: raw, math: math,
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
                // Rust 与原生错误源码共用系统随附的 Menlo，避免私有系统等宽字体的可变字宽不一致。
                let sourceFont = PlatformGraphics.namedFont("Menlo-Regular", size: size)
                    ?? PlatformGraphics.monospacedFont(ofSize: size, weight: .regular)
                let reasonFont = PlatformGraphics.systemFont(ofSize: 12)
                let contextColor = NSAttributedString.Key(kCTForegroundColorFromContextAttributeName as String)
                let reasonLines = wrap(message, attributes: [.font: reasonFont, contextColor: true])
                let lines = block.block ? wrap(block.displaySource, attributes: [.font: sourceFont, contextColor: true])
                    : [CTLineCreateWithAttributedString(NSAttributedString(string: block.displaySource,
                        attributes: [.font: sourceFont, contextColor: true]))]
                let textHeight = Double(reasonLines.count) * 16 + Double(lines.count) * (size + 5) + 16
                return PreparedMathBlock(block: block, source: raw, math: nil,
                    reason: message, reasonLines: reasonLines, fallbackLines: lines,
                    fallbackHeight: textHeight)
            }
        }
    }
}
