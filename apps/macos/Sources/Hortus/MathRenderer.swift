import AppKit
import CoreText
import HortusCore

enum MathTypography {
    static let scale = 1.21
    static let padding = 8.0
}

struct MathScene: Decodable, Sendable {
    struct Color: Decodable, Sendable {
        let foreground: Bool
        let rgba: [Double]
    }
    struct Command: Decodable, Sendable {
        let kind: String
        let x, y, x1, y1, x2, y2: Double?
    }
    struct Item: Decodable, Sendable {
        let kind: String
        let x, y: Double
        let scale, width, height, thickness: Double?
        let font: String?
        let codepoint: UInt32?
        let fallback, dashed, fill: Bool?
        let commands: [Command]?
        let color: Color
    }
    let width, height, depth: Double
    let items: [Item]
    let missingMetrics: [UInt32]

    init(_ bytes: Data) throws {
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
            values.allSatisfy { $0.map { $0.isFinite && abs($0) <= 1_000_000 } == true }
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

struct PreparedMath: @unchecked Sendable {
    enum Drawing {
        case glyph(CTFont, CGGlyph, CGPoint, Double, MathScene.Color)
        case line(CGPoint, Double, Double, Bool, MathScene.Color)
        case rect(CGRect, MathScene.Color)
        case path(CGPath, Bool, MathScene.Color)
    }
    let size: CGSize
    let drawing: [Drawing]
    let glyphCount: Int
    let maximumMetricError: Double
    let estimatedBytes: Int
}

/// 场景已在后台解析、选字体和建路径；这里只绘制，不分配字体或进行排版。
enum MathRenderer {
    static func draw(_ math: PreparedMath, in context: CGContext, origin: CGPoint, foreground: CGColor, deviceScale: Double) {
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
                let thickness = max(thickness, 1 / max(1, deviceScale))
                if dashed {
                    let dash = 4 * thickness
                    var offset = 0.0
                    while offset < width {
                        context.fill(CGRect(x: point.x + offset, y: point.y - thickness / 2,
                                            width: min(dash, width - offset), height: thickness))
                        offset += 2 * dash
                    }
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
actor MathService {
    static let shared = MathService()
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
    private(set) var mainThreadPreparations = 0
    private(set) var registrationCount = 0
    private(set) var measurementCount = 0

    init(resourceDirectory: URL? = nil) { self.resourceDirectory = resourceDirectory }

    private func register() throws {
        guard !registered else { return }
        if let registrationFailure { throw registrationFailure }
        do { try registerFonts() } catch { registrationFailure = error; throw error }
    }
    private func registerFonts() throws {
        guard let directory = resourceDirectory ?? Bundle.main.resourceURL?.appendingPathComponent("Fonts/KaTeX") else {
            throw CocoaError(.fileNoSuchFile)
        }
        let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "ttf" && $0.lastPathComponent.hasPrefix("KaTeX_") }.sorted { $0.lastPathComponent < $1.lastPathComponent }
        guard files.count == 20 else { throw CocoaError(.fileNoSuchFile) }
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
    private func measure(_ codepoint: UInt32) throws -> MathGlyphMetrics {
        if let value = metrics[codepoint] { return value }
        let font = try fallback(codepoint)
        var glyph = try glyph(font, codepoint), advance = CGSize.zero, box = CGRect.zero
        CTFontGetAdvancesForGlyphs(font, .horizontal, &glyph, &advance, 1)
        CTFontGetBoundingRectsForGlyphs(font, .horizontal, &glyph, &box, 1)
        let value = MathGlyphMetrics(codepoint: codepoint, width: advance.width / 1000,
                                    height: max(0, box.maxY) / 1000, depth: max(0, -box.minY) / 1000)
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
    func render(_ latex: String, style: MathStyle, bodySize: Double) throws -> PreparedMath {
        if Thread.isMainThread { mainThreadPreparations += 1 }
        try register()
        let key = "\(style):\(bodySize):\(latex)"
        if let value = cached[key] { return value }
        var scene = try MathScene(engine.render(source: latex, style: style))
        if !scene.missingMetrics.isEmpty {
            try engine.setMetrics(metrics: scene.missingMetrics.map { try measure($0) })
            scene = try MathScene(engine.render(source: latex, style: style))
        }
        let em = bodySize * MathTypography.scale
        var drawing: [PreparedMath.Drawing] = [], glyphCount = 0, metricError = 0.0
        var pathBytes = 0
        for item in scene.items {
            let point = CGPoint(x: item.x * em, y: item.y * em)
            switch item.kind {
            case "Glyph":
                guard let codepoint = item.codepoint, let key = item.font, let scale = item.scale, let useFallback = item.fallback else { throw CocoaError(.fileReadCorruptFile) }
                // 系统回退字体保留量字时的 1000 pt 实例，绘制变换保证光学字号不改变字宽。
                let factor = useFallback ? scale * em / 1000 : 1
                let font = try font(key, codepoint: codepoint, fallback: useFallback, size: useFallback ? 1000 : scale * em)
                var glyph = try glyph(font, codepoint)
                drawing.append(.glyph(font, glyph, point, factor, item.color)); glyphCount += 1
                if useFallback, let metric = metrics[codepoint] {
                    var advance = CGSize.zero
                    CTFontGetAdvancesForGlyphs(font, .horizontal, &glyph, &advance, 1)
                    metricError = max(metricError, abs(advance.width * factor - metric.width * scale * em))
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
                                 drawing: drawing, glyphCount: glyphCount, maximumMetricError: metricError,
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
}
