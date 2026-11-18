import AppKit
import Testing
@testable import Hortus

@MainActor
struct SettingsTests {
    @Test func terminalFontResolvesInstalledFamilyAndFallsBack() throws {
        let families = TerminalFonts.monospacedFamilies()
        #expect(!families.isEmpty)
        #expect(families == families.sorted { $0.localizedStandardCompare($1) == .orderedAscending })
        let family = try #require(families.first { $0.localizedCaseInsensitiveContains("Menlo") } ?? families.first)
        let installed = try #require(TerminalFonts.installedFont(family: family, size: 17))
        #expect((installed.familyName ?? "").caseInsensitiveCompare(family) == .orderedSame)
        #expect(installed.pointSize == 17)
        #expect(TerminalFonts.installedFont(family: "Definitely Missing Font \(UUID().uuidString)", size: 17) == nil)
        let fallback = TerminalFonts.font(family: "Definitely Missing Font \(UUID().uuidString)", size: 17)
        #expect(fallback.fontName == NSFont.monospacedSystemFont(ofSize: 17, weight: .regular).fontName)
        #expect(TerminalFonts.font(family: nil, size: 17).fontName == fallback.fontName)
        #expect(!families.contains("Times New Roman"))
    }

    @Test func terminalAppearanceChangesFontWithoutReplacingTheView() throws {
        let catalog = TerminalFonts.monospacedFamilies()
        let family = try #require(catalog.first { !$0.localizedCaseInsensitiveContains("Nerd") } ?? catalog.first)
        let session = TerminalSession()
        let view = HortusTerminalView(frame: CGRect(x: 0, y: 0, width: 600, height: 200), session: session)
        view.applyAppearance(fontSize: 15, terminalFont: nil)
        #expect(view.font.fontName == NSFont.monospacedSystemFont(ofSize: 15, weight: .regular).fontName)
        view.applyAppearance(fontSize: 19, terminalFont: family)
        #expect(view.font.pointSize == 19)
        #expect((view.font.familyName ?? "").caseInsensitiveCompare(family) == .orderedSame)
        let sameView = view
        view.applyAppearance(fontSize: 21, terminalFont: family)
        #expect(sameView === view)
        #expect(view.font.pointSize == 21)
        view.applyAppearance(fontSize: 21, terminalFont: nil)
        #expect(view.font.fontName == NSFont.monospacedSystemFont(ofSize: 21, weight: .regular).fontName)
    }

    @Test func settingsFontValidationCoversBounds() {
        let model = SettingsModel(application: ApplicationWorkspace())
        model.fontSize = 9
        #expect(model.fontSizeIsValid)
        model.fontSize = 40
        #expect(model.fontSizeIsValid)
        model.fontSize = 8.9
        #expect(!model.fontSizeIsValid)
        model.fontSize = 40.1
        #expect(!model.fontSizeIsValid)
        model.fontSize = .infinity
        #expect(!model.fontSizeIsValid)
    }

    @Test func usingDetectedAgentOnlyPrefillsTheForm() {
        let model = SettingsModel(application: ApplicationWorkspace())
        let agent = DetectedAgent(id: "omp", displayName: "OMP", binaryName: "omp",
                                  executable: "/opt/homebrew/bin/omp",
                                  suggestedArgs: ["--append-system-prompt={guide}"])
        #expect(!model.isAgentSelected(agent))
        model.useAgent(agent)
        #expect(model.agentCommand == "/opt/homebrew/bin/omp")
        #expect(model.agentArgs == ["--append-system-prompt={guide}"])
        #expect(model.isAgentSelected(agent))
        model.agentCommand = "  /opt/homebrew/bin/omp  "
        #expect(model.isAgentSelected(agent))
        model.agentCommand = "/opt/homebrew/bin/codex"
        #expect(!model.isAgentSelected(agent))
    }

    @Test func debugLoginPathInjectionHandlesAbsentAndEmptyValues() {
        #expect(LoginShellEnvironment.injectedPath(arguments: []) == .absent)
        #expect(LoginShellEnvironment.injectedPath(arguments: ["--settings-login-path=/tmp/login"])
                == .path("/tmp/login"))
        #expect(LoginShellEnvironment.injectedPath(arguments: ["--settings-login-path="])
                == .path(nil))
        #expect(LoginShellEnvironment.injectedPath(arguments: ["--settings-login-path=   "])
                == .path(nil))
    }

    @Test func guidePlaceholderDetectionUsesArgvElements() {
        let model = SettingsModel(application: ApplicationWorkspace())
        model.agentArgs = ["--guide", "{guide}"]
        #expect(model.guideIncludesPlaceholder)
        model.agentArgs = ["--guide", "--guide={guide}"]
        #expect(model.guideIncludesPlaceholder)
        model.agentArgs = ["--guide", "guide"]
        #expect(!model.guideIncludesPlaceholder)
    }
}
