import AppKit
import SwiftTerm
import Testing
@testable import Hortus

/// 截获终端回给程序的字节，不启动 shell。
@MainActor
private final class ReplyRecorder: TerminalViewDelegate {
    var replies: [[UInt8]] = []
    func send(source: TerminalView, data: ArraySlice<UInt8>) { replies.append(Array(data)) }
    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) { }
    func setTerminalTitle(source: TerminalView, title: String) { }
    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) { }
    func scrolled(source: TerminalView, position: Double) { }
    func rangeChanged(source: TerminalView, startY: Int, endY: Int) { }
}

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

    /// 视图透明，但程序通过 OSC 11 查询到的背景必须是面板实际的颜色，否则会按黑底配色。
    @Test func backgroundStaysTransparentButReportsPanelColor() async throws {
        let session = TerminalSession()
        let view = HortusTerminalView(session: session)
        let recorder = ReplyRecorder()
        view.terminalDelegate = recorder
        let dark = try #require(NSAppearance(named: .darkAqua))
        let light = try #require(NSAppearance(named: .aqua))
        for (appearance, isDark) in [(light, false), (dark, true)] {
            view.appearance = appearance
            view.viewDidChangeEffectiveAppearance()
            #expect(view.backgroundOpacity == 0)
            let background = try #require(view.nativeBackgroundColor.usingColorSpace(.sRGB))
            #expect((background.redComponent > 0.5) == !isDark)
            #expect((background.blueComponent > 0.5) == !isDark)

            recorder.replies = []
            view.feed(text: "\u{1b}]11;?\u{07}")
            for _ in 0..<100 where recorder.replies.isEmpty { try await Task.sleep(for: .milliseconds(10)) }
            let reply = String(decoding: recorder.replies.joined(), as: UTF8.self)
            let red = try #require(reply.firstRange(of: "rgb:").map { reply[$0.upperBound...].prefix(4) })
            #expect(((Int(red, radix: 16) ?? 0) > 0x8000) == !isDark, "\(reply.debugDescription)")
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
