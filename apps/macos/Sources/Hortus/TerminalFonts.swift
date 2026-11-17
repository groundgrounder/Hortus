import AppKit
import CoreText

/// 终端字体解析：配置只存字体族名；未安装或非等宽时回退 SF Mono。
/// 字体族枚举可能较慢，调用方放在后台任务中，主线程只解析单个已知名字。
enum TerminalFonts {
    /// 安装的等宽字体族，按本地化族名排序；枚举本身不缓存，设置模型只跑一次。
    static func monospacedFamilies() -> [String] {
        let families = CTFontManagerCopyAvailableFontFamilyNames() as? [String] ?? []
        return families.filter { family in
            guard let descriptor = fontDescriptor(family: family) else { return false }
            let font = CTFontCreateWithFontDescriptor(descriptor, 15, nil)
            return CTFontGetSymbolicTraits(font).contains(.traitMonoSpace)
        }.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }

    /// 已安装则返回对应字体族的常规字体；否则返回 SF Mono。
    static func font(family: String?, size: CGFloat) -> NSFont {
        if let family, !family.isEmpty, let installed = installedFont(family: family, size: size) {
            return installed
        }
        return NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    static func installedFont(family: String, size: CGFloat) -> NSFont? {
        guard let descriptor = fontDescriptor(family: family) else { return nil }
        let candidate = CTFontCreateWithFontDescriptor(descriptor, size, nil)
        guard CTFontCopyFamilyName(candidate) as String == family
                || (CTFontCopyFamilyName(candidate) as String).caseInsensitiveCompare(family) == .orderedSame,
              CTFontGetSymbolicTraits(candidate).contains(.traitMonoSpace),
              let font = NSFont(descriptor: NSFontDescriptor(fontAttributes: [.family: family, .size: size]), size: size)
        else { return nil }
        return font
    }

    private static func fontDescriptor(family: String) -> CTFontDescriptor? {
        guard !family.isEmpty else { return nil }
        return CTFontDescriptorCreateWithAttributes(
            [kCTFontFamilyNameAttribute: family] as CFDictionary)
    }
}
