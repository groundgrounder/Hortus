import AppKit
import CoreText
import Testing
@testable import Hortus
@testable import SwiftTerm

@MainActor
struct TerminalNerdFontTests {
    @Test func familyOrderIsStableAndRecognizesOnlyNamedFonts() {
        let input = ["MesloLGS NF", "JetBrainsMono Nerd Font", "JetBrainsMono Nerd Font Propo",
                     "JetBrainsMono Nerd Font Mono", "JetBrainsMonoNL Nerd Font Mono", "ANF Mono",
                     "NFont", "Some NF Extra", "JetBrainsMono Nerd Font Mono", "SF Mono"]
        let expected = ["JetBrainsMono Nerd Font Mono", "JetBrainsMonoNL Nerd Font Mono",
                        "JetBrainsMono Nerd Font", "MesloLGS NF", "JetBrainsMono Nerd Font Propo"]
        #expect(TerminalNerdFonts.orderedFamilies(input) == expected)
        #expect(TerminalNerdFonts.orderedFamilies(input.reversed()) == expected)
        #expect(TerminalNerdFonts.orderedFamilies([]).isEmpty)
        #expect(TerminalNerdFonts(families: []).provider == nil)
    }

    @Test func installedDiscoveryAndProcessCache() async {
        let first = await TerminalNerdFonts.installed.value
        let second = await TerminalNerdFonts.installed.value
        #expect(first === second)
        #expect(!first.enumerationOnMain)
        if let provider = first.provider {
            let font = provider.fallbackFont(forPointSize: 15)
            #expect(font === provider.fallbackFont(forPointSize: 15))
            #expect(provider.placementPolicy(for: "中") == nil)
            #expect(provider.placementPolicy(for: "😀") == nil)
            #expect(provider.placementPolicy(for: "A") == nil)
            #expect(provider.placementPolicy(for: "\u{E0B0}") == nil)
        }
    }

    @Test func existingPrimaryGlyphKeepsAllFourSFMonoFaces() async throws {
        let catalog = await TerminalNerdFonts.installed.value
        let view = HortusTerminalView(frame: CGRect(x: 0, y: 0, width: 600, height: 200), session: TerminalSession())
        view.applyAppearance(fontSize: 15)
        view.glyphFallbackProvider = catalog.provider
        view.feed(text: "\u{1b}[H\u{1b}[0mA\u{F8FF} \u{1b}[1mA\u{F8FF} \u{1b}[0;3mA\u{F8FF} \u{1b}[1;3mA\u{F8FF}")
        let cells = TerminalFontInspection.cells(in: view).filter { ($0["text"] as? String) == "A" || ($0["text"] as? String) == "\u{F8FF}" }
        #expect(cells.count == 8)
        let primaryNames = Set([view.fontSet.normal, view.fontSet.bold, view.fontSet.italic, view.fontSet.boldItalic].map(\.fontName))
        for cell in cells {
            #expect(primaryNames.contains(cell["font"] as? String ?? ""))
            #expect(cell["provider"] as? Bool == false)
            #expect((cell["glyph"] as? Int ?? 0) > 0)
        }
        #expect(Set(cells.compactMap { $0["font"] as? String }).count == 4)
    }

    @Test func missingGlyphWalksPastMeslo() async throws {
        let catalog = await TerminalNerdFonts.installed.value
        guard catalog.families.contains("MesloLGS NF"), catalog.families.contains("JetBrainsMono Nerd Font Mono") else {
            print("SKIP: installed MesloLGS NF and JetBrainsMono Nerd Font Mono required for cascade check")
            return
        }
        let provider = NerdGlyphProvider(families: ["MesloLGS NF", "JetBrainsMono Nerd Font Mono"])
        let font = try #require(provider.fallbackFont(forPointSize: 15))
        for scalar in ["\u{E6B2}", "\u{E8EB}"] {
            let resolved = CTFontCreateForString(font, scalar as CFString, CFRange(location: 0, length: scalar.utf16.count))
            #expect((CTFontCopyPostScriptName(resolved) as String).contains("JetBrainsMono"))
        }
    }
}

