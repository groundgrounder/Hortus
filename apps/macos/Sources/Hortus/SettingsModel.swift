import AppKit
import HortusCore
import SwiftUI

/// 设置窗口的唯一状态：读取 `config.toml` 快照、校验并保存，再把结果广播给所有窗口。
@Observable @MainActor
final class SettingsModel {
    enum Tab: Hashable {
        case general
        case terminal
        case agent
        case configuration
    }

    let application: ApplicationWorkspace
    var tab: Tab = .general
    var fontSize = 15.0
    var defaultView = "reading"
    var terminalFont: String?
    var agentCommand = ""
    var agentArgs: [String] = []
    var probe: AgentProbeInfo?
    var guideText = ""
    var guideOverridden = false
    var guidePath = ""
    var guideDefaultText = ""
    var configPath = ""
    var digest: String?
    var diagnostics: [String] = []
    var fontFamilies: [String] = []
    var fontsLoading = true
    var loading = true
    var saving = false
    var statusMessage: String?
    var statusIsError = false
    var guideMessage: String?
    var guideMessageIsError = false
    var conflict = false
    @ObservationIgnored private var probeTask: Task<Void, Never>?
    @ObservationIgnored private var loaded = false

    init(application: ApplicationWorkspace) {
        self.application = application
    }

    var fontSizeIsValid: Bool {
        fontSize.isFinite && (9.0...40.0).contains(fontSize)
    }

    var guideIncludesPlaceholder: Bool {
        agentArgs.contains { $0.contains("{guide}") }
    }

    var terminalFontUnavailable: Bool {
        guard let terminalFont, !terminalFont.isEmpty else { return false }
        return TerminalFonts.installedFont(family: terminalFont, size: 15) == nil
    }

    var canSave: Bool {
        !saving && loaded && application.core != nil && fontSizeIsValid
    }

    func load() async {
        guard let core = application.core else {
            loading = false
            statusMessage = L10n.string("Hortus configuration is unavailable")
            statusIsError = true
            return
        }
        loading = true
        defer { loading = false }
        do {
            let info = try await Task.detached { try core.editableConfig() }.value
            apply(info)
            loaded = true
            statusMessage = nil
            statusIsError = false
            conflict = false
        } catch {
            statusMessage = L10n.error(error)
            statusIsError = true
            return
        }
        await loadGuide(core)
        scheduleProbe()
    }

    private func loadGuide(_ core: WorkspaceCore) async {
        do {
            let state = try await Task.detached { try core.agentGuideState() }.value
            applyGuide(state)
        } catch {
            guideText = ""
            guideMessage = L10n.error(error)
            guideMessageIsError = true
        }
    }

    func loadFonts() async {
        guard fontsLoading || fontFamilies.isEmpty else { return }
        let families = await Task.detached(priority: .userInitiated) {
            TerminalFonts.monospacedFamilies()
        }.value
        fontFamilies = families
        fontsLoading = false
    }

    private func apply(_ info: EditableConfigInfo) {
        configPath = info.configPath
        digest = info.digest
        fontSize = info.fontSize
        defaultView = info.defaultView
        terminalFont = info.terminalFont
        agentCommand = info.agentCommand ?? ""
        agentArgs = info.agentArgs
        diagnostics = info.diagnostics
    }

    private func applyGuide(_ info: AgentGuideInfo) {
        guideText = info.text
        guideOverridden = info.overridden
        guidePath = info.path
        guideDefaultText = info.defaultText
        guideMessage = nil
        guideMessageIsError = false
    }

    /// 保存成功后立即更新应用级默认值与所有已打开窗口；不重建终端视图或进程。
    func save(force: Bool = false) async {
        guard let core = application.core, fontSizeIsValid else { return }
        saving = true
        defer { saving = false }
        let input = EditableConfigInputInfo(
            fontSize: fontSize,
            defaultView: defaultView == "editing" ? "editing" : "reading",
            terminalFont: terminalFont?.trimmingCharacters(in: .whitespaces).isEmpty == false ? terminalFont : nil,
            agentCommand: agentCommand.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : agentCommand,
            agentArgs: agentArgs
        )
        do {
            let expected = force ? nil : digest
            let info = try await Task.detached {
                try core.saveEditableConfig(input: input, expectedDigest: expected)
            }.value
            apply(info)
            application.applySettings(fontSize: info.fontSize,
                                      defaultView: info.defaultView,
                                      terminalFont: info.terminalFont)
            statusMessage = L10n.string("Settings saved")
            statusIsError = false
            conflict = false
            scheduleProbe()
        } catch {
            if Self.kind(of: error) == "config.externally_modified" {
                conflict = true
                statusMessage = L10n.errorKey("config.externally_modified")
            } else {
                statusMessage = L10n.error(error)
                statusIsError = true
            }
        }
    }

    func saveGuide() async {
        guard let core = application.core else { return }
        let content = guideText
        do {
            let state = try await Task.detached {
                try core.saveAgentGuide(content: content)
            }.value
            applyGuide(state)
            guideMessage = L10n.string("Guide saved")
            guideMessageIsError = false
        } catch {
            guideMessage = L10n.error(error)
            guideMessageIsError = true
        }
    }

    func resetGuide() async {
        guard let core = application.core else { return }
        do {
            let state = try await Task.detached { try core.resetAgentGuide() }.value
            applyGuide(state)
            guideMessage = L10n.string("Built-in guide restored")
            guideMessageIsError = false
        } catch {
            guideMessage = L10n.error(error)
            guideMessageIsError = true
        }
    }

    func scheduleProbe() {
        probeTask?.cancel()
        let command = agentCommand
        let args = agentArgs
        probeTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled, let self, let core = application.core else { return }
            let status = await Task.detached {
                core.probeAgentLaunch(command: command, args: args)
            }.value
            guard !Task.isCancelled else { return }
            probe = status
        }
    }

    func addArgument() {
        agentArgs.append("")
        scheduleProbe()
    }

    func removeArgument(at index: Int) {
        guard agentArgs.indices.contains(index) else { return }
        agentArgs.remove(at: index)
        scheduleProbe()
    }

    func moveArgument(at index: Int, by offset: Int) {
        let target = index + offset
        guard agentArgs.indices.contains(index), agentArgs.indices.contains(target) else { return }
        agentArgs.swapAt(index, target)
        scheduleProbe()
    }

    func chooseAgentCommand() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.prompt = L10n.string("Choose")
        panel.message = L10n.string("Choose the executable used to start an Agent")
        if panel.runModal() == .OK, let url = panel.url {
            agentCommand = url.path
            scheduleProbe()
        }
    }

    func openConfigFile() {
        guard !configPath.isEmpty else { return }
        NSWorkspace.shared.open(URL(fileURLWithPath: configPath))
    }

    private static func kind(of error: Error) -> String? {
        guard let error = error as? CoreError else { return nil }
        switch error {
        case let .Failure(kind): return kind
        }
    }
}
