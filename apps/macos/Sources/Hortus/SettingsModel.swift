import AppKit
import HortusCore
import SwiftUI

/// 设置界面展示的已发现 Agent；只含核心静态解析出的绝对路径与预设参数。
struct DetectedAgent: Identifiable, Hashable {
    let id: String
    let displayName: String
    let binaryName: String
    let executable: String
    let suggestedArgs: [String]
}

/// 设置界面编辑中的命名 Agent 配置档。
struct EditableAgentProfile: Identifiable, Hashable {
    let id: String
    var name: String
    var command: String
    var args: [String]
}

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
    var agentProfiles: [EditableAgentProfile] = []
    var selectedProfileID: String?
    var agentDefaultProfile: String?
    var cliStatus: CLIHelper.SymlinkStatus = .notInstalled
    var cliMessage: String?
    var cliMessageIsError = false
    var probe: AgentProbeInfo?
    var detectedAgents: [DetectedAgent] = []
    var agentScanning = false
    var agentScanComplete = false
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
    @ObservationIgnored private var agentScanTask: Task<Void, Never>?
    @ObservationIgnored private var loginPath: String?
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
        refreshCLIStatus()
        refreshDetectedAgents()
        scheduleProbe()
    }

    var selectedProfileName: String {
        get {
            guard let id = selectedProfileID,
                  let profile = agentProfiles.first(where: { $0.id == id }) else { return "" }
            return profile.name
        }
        set {
            guard let id = selectedProfileID,
                  let index = agentProfiles.firstIndex(where: { $0.id == id }) else { return }
            agentProfiles[index].name = newValue
        }
    }

    func syncCurrentProfileFromFields() {
        guard let id = selectedProfileID,
              let index = agentProfiles.firstIndex(where: { $0.id == id }) else { return }
        agentProfiles[index].command = agentCommand
        agentProfiles[index].args = agentArgs
    }

    func selectProfile(_ id: String) {
        syncCurrentProfileFromFields()
        guard let profile = agentProfiles.first(where: { $0.id == id }) else { return }
        selectedProfileID = id
        agentCommand = profile.command
        agentArgs = profile.args
        scheduleProbe()
    }

    func addProfile(name: String? = nil, command: String = "", args: [String] = ["{guide}"]) {
        syncCurrentProfileFromFields()
        // 首次从单配置升级为多配置档时，自动保留当前已填配置为首个配置档。
        if agentProfiles.isEmpty {
            let trimmed = agentCommand.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty {
                let firstID = makeProfileID(base: "default")
                let firstName = URL(fileURLWithPath: trimmed).lastPathComponent
                agentProfiles.append(EditableAgentProfile(
                    id: firstID,
                    name: firstName.isEmpty ? "Default" : firstName,
                    command: agentCommand,
                    args: agentArgs
                ))
                agentDefaultProfile = firstID
            }
        }
        let nextIndex = agentProfiles.count + 1
        let resolvedName = name?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false
            ? name!
            : L10n.string("Profile %d", nextIndex)
        let newID = makeProfileID(base: "profile-\(nextIndex)")
        let profile = EditableAgentProfile(id: newID, name: resolvedName, command: command, args: args)
        agentProfiles.append(profile)
        if agentDefaultProfile == nil {
            agentDefaultProfile = newID
        }
        selectedProfileID = newID
        agentCommand = profile.command
        agentArgs = profile.args
        scheduleProbe()
    }

    func deleteProfile(_ id: String) {
        syncCurrentProfileFromFields()
        agentProfiles.removeAll { $0.id == id }
        if agentDefaultProfile == id {
            agentDefaultProfile = agentProfiles.first?.id
        }
        if selectedProfileID == id {
            if let next = agentProfiles.first {
                selectedProfileID = next.id
                agentCommand = next.command
                agentArgs = next.args
            } else {
                selectedProfileID = nil
                agentCommand = ""
                agentArgs = []
            }
        }
        scheduleProbe()
    }

    func setDefaultProfile(_ id: String) {
        guard agentProfiles.contains(where: { $0.id == id }) else { return }
        agentDefaultProfile = id
    }

    private func makeProfileID(base: String) -> String {
        let existing = Set(agentProfiles.map(\.id))
        if !existing.contains(base) { return base }
        var suffix = 2
        while existing.contains("\(base)-\(suffix)") { suffix += 1 }
        return "\(base)-\(suffix)"
    }

    func refreshCLIStatus() {
        cliStatus = CLIHelper.symlinkStatus()
    }

    func installCLISymlink() {
        do {
            let destination = try CLIHelper.installSymlink()
            refreshCLIStatus()
            cliMessage = destination.path
            cliMessageIsError = false
        } catch {
            refreshCLIStatus()
            cliMessage = error.localizedDescription
            cliMessageIsError = true
        }
    }

    func copyMCPConfiguration() {
        _ = CLIHelper.copyMCPConfiguration(vaultPath: application.active?.workspace.root)
        cliMessage = L10n.string("MCP configuration copied")
        cliMessageIsError = false
    }

    /// 后台只读扫描内置预设；扫描完成前不阻塞表单。
    func refreshDetectedAgents() {
        guard let core = application.core else { return }
        agentScanTask?.cancel()
        agentScanning = true
        agentScanTask = Task { [weak self] in
            let loginPath = await LoginShellEnvironment.resolvedPath()
            guard !Task.isCancelled, let self else { return }
            self.loginPath = loginPath
            let agents = await Task.detached {
                core.discoverAgents(loginPath: loginPath)
            }.value
            guard !Task.isCancelled else { return }
            self.detectedAgents = agents.map { info in
                DetectedAgent(id: info.id,
                              displayName: info.displayName,
                              binaryName: info.binaryName,
                              executable: info.executable,
                              suggestedArgs: info.suggestedArgs)
            }
            self.agentScanning = false
            self.agentScanComplete = true
            // 登录 PATH 到达后重新解析裸命令名与扩展目录。
            self.scheduleProbe()
        }
    }

    /// 只预填表单，不写盘；用户仍需点 Save。
    func useAgent(_ agent: DetectedAgent) {
        agentCommand = agent.executable
        agentArgs = agent.suggestedArgs
        syncCurrentProfileFromFields()
        scheduleProbe()
    }

    func isAgentSelected(_ agent: DetectedAgent) -> Bool {
        agentCommand.trimmingCharacters(in: .whitespacesAndNewlines) == agent.executable
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
        agentProfiles = info.agentProfiles.map {
            EditableAgentProfile(id: $0.id, name: $0.name, command: $0.command, args: $0.args)
        }
        agentDefaultProfile = info.agentDefaultProfile
        if !agentProfiles.isEmpty {
            let preferredID = selectedProfileID.flatMap { id in
                agentProfiles.contains(where: { $0.id == id }) ? id : nil
            } ?? agentDefaultProfile ?? agentProfiles[0].id
            selectedProfileID = preferredID
            if let current = agentProfiles.first(where: { $0.id == preferredID }) {
                agentCommand = current.command
                agentArgs = current.args
            }
        } else {
            selectedProfileID = nil
            agentCommand = info.agentCommand ?? ""
            agentArgs = info.agentArgs
        }
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
        syncCurrentProfileFromFields()
        let input = EditableConfigInputInfo(
            fontSize: fontSize,
            defaultView: defaultView == "editing" ? "editing" : "reading",
            terminalFont: terminalFont?.trimmingCharacters(in: .whitespaces).isEmpty == false ? terminalFont : nil,
            agentCommand: agentCommand.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : agentCommand,
            agentArgs: agentArgs
        )
        let profilesToSave = agentProfiles.map {
            AgentProfileInfo(id: $0.id, name: $0.name, command: $0.command, args: $0.args)
        }
        let defaultProfileToSave = agentDefaultProfile
        do {
            let expected = force ? nil : digest
            let info = try await Task.detached {
                try core.saveEditableConfigWithProfiles(
                    input: input,
                    agentProfiles: profilesToSave,
                    agentDefaultProfile: defaultProfileToSave,
                    expectedDigest: expected
                )
            }.value
            apply(info)
            application.applySettings(fontSize: info.fontSize,
                                      defaultView: info.defaultView,
                                      terminalFont: info.terminalFont)
            application.applyAgentProfiles(info.agentProfiles, defaultProfile: info.agentDefaultProfile)
            statusMessage = L10n.string("Settings saved")
            statusIsError = false
            conflict = false
            scheduleProbe()
        } catch {
            if Self.kind(of: error) == "config.externally_modified" {
                conflict = true
                statusMessage = L10n.errorKey("config.externally_modified")
                statusIsError = true
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
        let loginPath = loginPath
        probeTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled, let self, let core = application.core else { return }
            let status = await Task.detached {
                core.probeAgentLaunchWithPath(command: command, args: args, loginPath: loginPath)
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
