import CoreGraphics
import CoreText
import Foundation
import HortusCore

#if canImport(AppKit)
import AppKit

public typealias HortusFont = NSFont
public typealias HortusColor = NSColor

public enum PlatformGraphics {
    public static var textColor: HortusColor { .textColor }
    public static var labelColor: HortusColor { .labelColor }
    public static var secondaryLabelColor: HortusColor { .secondaryLabelColor }
    public static var tertiaryLabelColor: HortusColor { .tertiaryLabelColor }
    public static var quaternaryLabelColor: HortusColor { .quaternaryLabelColor }
    public static var linkColor: HortusColor { .linkColor }
    public static var accentColor: HortusColor { .controlAccentColor }
    public static var systemYellow: HortusColor { .systemYellow }

    public static func systemFont(ofSize size: Double, weight: HortusFont.Weight = .regular) -> HortusFont {
        NSFont.systemFont(ofSize: size, weight: weight)
    }

    public static func monospacedFont(ofSize size: Double, weight: HortusFont.Weight = .regular) -> HortusFont {
        NSFont.monospacedSystemFont(ofSize: size, weight: weight)
    }

    public static func namedFont(_ name: String, size: Double) -> HortusFont? {
        NSFont(name: name, size: size)
    }

    public static func italicFont(_ font: HortusFont) -> HortusFont {
        NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask)
    }

    public static func hasItalicTrait(_ font: HortusFont) -> Bool {
        font.fontDescriptor.symbolicTraits.contains(.italic)
    }

    public static func fontAscender(_ font: HortusFont) -> Double {
        Double(font.ascender)
    }

    public static func sRGBCGColor(_ color: HortusColor) -> CGColor {
        color.usingColorSpace(.sRGB)?.cgColor ?? color.cgColor
    }

    @MainActor
    public static func legacyScrollbarThickness() -> Double {
        NSScroller.preferredScrollerStyle == .legacy
            ? NSScroller.scrollerWidth(for: .regular, scrollerStyle: .legacy) : 0
    }

    public static func measurePrefixWidth(in text: NSString, range: NSRange, size: Double) -> Double {
        guard range.location <= text.length, range.length <= text.length - range.location else { return 0 }
        let paragraph = text.paragraphRange(for: range)
        let prefix = text.substring(with: NSRange(location: paragraph.location, length: range.location - paragraph.location))
        return Double((prefix as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: size)]).width)
    }

    public static func svgImageSize(url: URL) -> CGSize? {
        guard let image = NSImage(contentsOf: url), image.size.width > 0, image.size.height > 0 else { return nil }
        return image.size
    }

    public static func svgCGImage(url: URL) -> CGImage? {
        guard let native = NSImage(contentsOf: url) else { return nil }
        var rect = CGRect(origin: .zero, size: native.size)
        return native.cgImage(forProposedRect: &rect, context: nil, hints: nil)
    }

    public static func makeDiagramEngine() throws -> DiagramEngine {
        try DiagramEngine()
    }
}

#elseif canImport(UIKit)
import UIKit

public typealias HortusFont = UIFont
public typealias HortusColor = UIColor

public enum PlatformGraphics {
    public static var textColor: HortusColor { .label }
    public static var labelColor: HortusColor { .label }
    public static var secondaryLabelColor: HortusColor { .secondaryLabel }
    public static var tertiaryLabelColor: HortusColor { .tertiaryLabel }
    public static var quaternaryLabelColor: HortusColor { .quaternaryLabel }
    public static var linkColor: HortusColor { .link }
    public static var accentColor: HortusColor { .tintColor }
    public static var systemYellow: HortusColor { .systemYellow }

    public static func systemFont(ofSize size: Double, weight: HortusFont.Weight = .regular) -> HortusFont {
        UIFont.systemFont(ofSize: size, weight: weight)
    }

    public static func monospacedFont(ofSize size: Double, weight: HortusFont.Weight = .regular) -> HortusFont {
        UIFont.monospacedSystemFont(ofSize: size, weight: weight)
    }

    public static func namedFont(_ name: String, size: Double) -> HortusFont? {
        UIFont(name: name, size: size)
    }

    public static func italicFont(_ font: HortusFont) -> HortusFont {
        guard let descriptor = font.fontDescriptor.withSymbolicTraits(font.fontDescriptor.symbolicTraits.union(.traitItalic)) else {
            return font
        }
        return UIFont(descriptor: descriptor, size: font.pointSize)
    }

    public static func hasItalicTrait(_ font: HortusFont) -> Bool {
        font.fontDescriptor.symbolicTraits.contains(.traitItalic)
    }

    public static func fontAscender(_ font: HortusFont) -> Double {
        Double(font.ascender)
    }

    public static func sRGBCGColor(_ color: HortusColor) -> CGColor {
        if let space = CGColorSpace(name: CGColorSpace.sRGB),
           let converted = color.cgColor.converted(to: space, intent: .defaultIntent, options: nil) {
            return converted
        }
        return color.cgColor
    }

    @MainActor
    public static func legacyScrollbarThickness() -> Double { 0 }

    public static func measurePrefixWidth(in text: NSString, range: NSRange, size: Double) -> Double {
        guard range.location <= text.length, range.length <= text.length - range.location else { return 0 }
        let paragraph = text.paragraphRange(for: range)
        let prefix = text.substring(with: NSRange(location: paragraph.location, length: range.location - paragraph.location))
        return Double((prefix as NSString).size(withAttributes: [.font: UIFont.systemFont(ofSize: size)]).width)
    }

    public static func svgImageSize(url: URL) -> CGSize? { nil }
    public static func svgCGImage(url: URL) -> CGImage? { nil }

    public static func systemFontPaths() -> [String] {
        var paths: [String] = []
        var seen = Set<String>()
        func append(_ descriptor: CTFontDescriptor) {
            if let url = CTFontDescriptorCopyAttribute(descriptor, kCTFontURLAttribute) as? URL,
               url.isFileURL,
               FileManager.default.isReadableFile(atPath: url.path),
               seen.insert(url.path).inserted {
                paths.append(url.path)
            }
        }
        if let uiFont = CTFontCreateUIFontForLanguage(.system, 15, nil) {
            append(CTFontCopyFontDescriptor(uiFont))
        }
        for name in ["PingFang SC", "PingFangSC-Regular", "Menlo", "Menlo-Regular", "Apple Color Emoji"] {
            append(CTFontDescriptorCreateWithNameAndSize(name as CFString, 15))
        }
        let collection = CTFontCollectionCreateFromAvailableFonts(nil)
        if let descriptors = CTFontCollectionCreateMatchingFontDescriptors(collection) as? [CTFontDescriptor] {
            for descriptor in descriptors {
                append(descriptor)
            }
        }
        return paths
    }

    public static func makeDiagramEngine() throws -> DiagramEngine {
        let paths = systemFontPaths()
        if !paths.isEmpty {
            return try DiagramEngine.withFontPaths(paths: paths)
        }
        return try DiagramEngine()
    }
}
#endif
