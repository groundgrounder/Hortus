import AppKit
import Testing
@testable import Hortus

@MainActor
struct TerminalTests {
    @Test func foregroundFollowsViewAppearanceAcrossThemes() throws {
        let session = TerminalSession()
        let view = HortusTerminalView(session: session)
        let dark = try #require(NSAppearance(named: .darkAqua))
        let light = try #require(NSAppearance(named: .aqua))
        for (appearance, ambient, isDark) in [(light, dark, false), (dark, light, true), (light, dark, false)] {
            ambient.performAsCurrentDrawingAppearance {
                view.appearance = appearance
                view.viewDidChangeEffectiveAppearance()
                view.applyAppearance(fontSize: 15)
                let foreground = view.nativeForegroundColor.usingColorSpace(.sRGB)
                #expect(foreground != nil)
                #expect(((foreground?.redComponent ?? 0) > 0.5) == isDark)
                #expect(((foreground?.greenComponent ?? 0) > 0.5) == isDark)
                #expect(((foreground?.blueComponent ?? 0) > 0.5) == isDark)
            }
        }
    }

    @Test func shellLocaleFollowsLanguageOrderAndInstalledLocales() {
        let installed: Set = ["zh_CN.UTF-8", "zh_TW.UTF-8", "en_US.UTF-8", "en_GB.UTF-8", "ja_JP.UTF-8"]
        let has = { installed.contains($0) }
        #expect(TerminalSession.utf8Locale(preferred: ["zh-Hans-CN", "en-US"], region: "CN", installed: has) == "zh_CN.UTF-8")
        // 只有文字没有地区时，简体用 CN、繁体用 TW
        #expect(TerminalSession.utf8Locale(preferred: ["zh-Hans"], region: nil, installed: has) == "zh_CN.UTF-8")
        #expect(TerminalSession.utf8Locale(preferred: ["zh-Hant"], region: nil, installed: has) == "zh_TW.UTF-8")
        #expect(TerminalSession.utf8Locale(preferred: ["en-GB"], region: "GB", installed: has) == "en_GB.UTF-8")
        // 系统没装对应 locale 时退到下一个语言，最后是 en_US
        #expect(TerminalSession.utf8Locale(preferred: ["fr-FR", "ja-JP"], region: "FR", installed: has) == "ja_JP.UTF-8")
        #expect(TerminalSession.utf8Locale(preferred: ["fr-FR"], region: "FR", installed: has) == "en_US.UTF-8")
    }
}
