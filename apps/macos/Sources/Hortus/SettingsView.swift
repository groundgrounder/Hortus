import AppKit
import HortusCore
import SwiftUI

/// 原生分组表单；所有文案来自 String Catalog，源语言英文。
struct SettingsView: View {
    @State private var model: SettingsModel

    init(application: ApplicationWorkspace) {
        _model = State(initialValue: SettingsModel(application: application))
    }

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            TabView(selection: $model.tab) {
                GeneralSettingsView(model: model)
                    .tabItem { Label(L10n.string("General"), systemImage: "gearshape") }
                    .tag(SettingsModel.Tab.general)
                TerminalSettingsView(model: model)
                    .tabItem { Label(L10n.string("Terminal"), systemImage: "terminal") }
                    .tag(SettingsModel.Tab.terminal)
                AgentSettingsView(model: model)
                    .tabItem { Label(L10n.string("Agent"), systemImage: "sparkles") }
                    .tag(SettingsModel.Tab.agent)
                ConfigurationSettingsView(model: model)
                    .tabItem { Label(L10n.string("Configuration"), systemImage: "doc.text") }
                    .tag(SettingsModel.Tab.configuration)
            }
            Divider()
            footer
        }
        .frame(width: 680, height: 720)
        .onAppear {
            #if DEBUG
            SettingsInspectionBridge.current = model
            #endif
        }
        .task {
            await model.loadFonts()
            let deadline = ProcessInfo.processInfo.systemUptime + 10
            while model.application.core == nil, ProcessInfo.processInfo.systemUptime < deadline {
                try? await Task.sleep(for: .milliseconds(50))
            }
            await model.load()
        }
        .alert(L10n.string("config.toml was changed on disk"), isPresented: $model.conflict) {
            Button(L10n.string("Reload from Disk")) { Task { await model.load() } }
            Button(L10n.string("Overwrite")) { Task { await model.save(force: true) } }
            Button(L10n.string("Cancel"), role: .cancel) { model.conflict = false }
        } message: {
            Text(L10n.string("Your edits were not written. Reload the current file, or overwrite it deliberately."))
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(L10n.string("Settings"))
    }

    private var footer: some View {
        HStack(spacing: 10) {
            if model.loading {
                ProgressView().controlSize(.small)
                Text(L10n.string("Loading settings…")).foregroundStyle(.secondary)
            } else if let message = model.statusMessage {
                Label(message, systemImage: model.statusIsError ? "exclamationmark.triangle" : "checkmark.circle")
                    .foregroundStyle(model.statusIsError ? Color.red : Color.secondary)
                    .lineLimit(2)
            }
            Spacer()
            Button(L10n.string("Save")) { Task { await model.save() } }
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canSave)
                .accessibilityLabel(L10n.string("Save settings"))
        }
        .padding(12)
    }
}

private struct GeneralSettingsView: View {
    @Bindable var model: SettingsModel

    var body: some View {
        Form {
            Section(L10n.string("Text")) {
                LabeledContent(L10n.string("Body font size")) {
                    HStack(spacing: 8) {
                        TextField("", value: $model.fontSize, format: .number, prompt: Text("15"))
                            .multilineTextAlignment(.trailing)
                            .frame(width: 90)
                            .accessibilityLabel(L10n.string("Body font size"))
                        Stepper("", value: $model.fontSize, in: 9...40, step: 1)
                            .labelsHidden()
                            .accessibilityLabel(L10n.string("Adjust body font size"))
                        Text(L10n.string("pt")).foregroundStyle(.secondary)
                    }
                }
                if !model.fontSizeIsValid {
                    Label(L10n.string("Font size must be between 9 and 40 points"),
                          systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.red)
                        .accessibilityLabel(L10n.string("Font size must be between 9 and 40 points"))
                }
                Picker(L10n.string("Default view"), selection: $model.defaultView) {
                    Text(L10n.string("Reading")).tag("reading")
                    Text(L10n.string("Editing")).tag("editing")
                }
                .pickerStyle(.segmented)
                .accessibilityLabel(L10n.string("Default view"))
            }
        }
        .formStyle(.grouped)
    }
}

private struct TerminalSettingsView: View {
    @Bindable var model: SettingsModel

    var body: some View {
        Form {
            Section(L10n.string("Terminal Font")) {
                if model.fontsLoading {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text(L10n.string("Reading installed fonts…")).foregroundStyle(.secondary)
                    }
                }
                Picker(L10n.string("Monospaced font"), selection: $model.terminalFont) {
                    Text(L10n.string("System Default (SF Mono)")).tag(String?.none)
                    ForEach(model.fontFamilies, id: \.self) { family in
                        Text(family).tag(Optional(family))
                    }
                    if let current = model.terminalFont, !model.fontFamilies.contains(current) {
                        Text(L10n.string("%@ (not installed)", current)).tag(Optional(current))
                    }
                }
                .accessibilityLabel(L10n.string("Terminal Font"))
                if model.terminalFontUnavailable {
                    Label(L10n.string("This font is not installed; the terminal falls back to SF Mono."),
                          systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
            }
        }
        .formStyle(.grouped)
    }
}

private struct AgentSettingsView: View {
    @Bindable var model: SettingsModel
    @State private var confirmResetGuide = false

    var body: some View {
        let indices: [Int] = Array(model.agentArgs.indices)
        Form {
            Section(L10n.string("Detected Agents")) {
                detectedAgents
            }
            Section(L10n.string("Profiles")) {
                if !model.agentProfiles.isEmpty {
                    ForEach(model.agentProfiles) { profile in
                        let isSelected = model.selectedProfileID == profile.id
                        let isDefault = (model.agentDefaultProfile ?? model.agentProfiles.first?.id) == profile.id
                        HStack(spacing: 8) {
                            Button {
                                model.selectProfile(profile.id)
                            } label: {
                                HStack(spacing: 6) {
                                    Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
                                        .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
                                    Text(profile.name)
                                        .font(.body.weight(isSelected ? .medium : .regular))
                                    if isDefault {
                                        Text(L10n.string("Default"))
                                            .font(.caption2)
                                            .padding(.horizontal, 6)
                                            .padding(.vertical, 2)
                                            .background(Capsule().fill(Color.accentColor.opacity(0.14)))
                                    }
                                    Spacer(minLength: 4)
                                }
                                .contentShape(.rect)
                            }
                            .buttonStyle(.plain)

                            if !isDefault {
                                Button(L10n.string("Set Default")) {
                                    model.setDefaultProfile(profile.id)
                                }
                                .buttonStyle(.borderless)
                            }
                            Button(role: .destructive) {
                                model.deleteProfile(profile.id)
                            } label: {
                                Image(systemName: "trash")
                            }
                            .buttonStyle(.borderless)
                            .accessibilityLabel(L10n.string("Delete %@", profile.name))
                        }
                    }
                }
                Button {
                    model.addProfile()
                } label: {
                    Label(L10n.string("Add Profile"), systemImage: "plus")
                }
                .accessibilityLabel(L10n.string("Add Profile"))
            }
            Section(L10n.string("Agent Launch")) {
                if model.selectedProfileID != nil {
                    LabeledContent(L10n.string("Profile Name")) {
                        TextField("", text: $model.selectedProfileName, prompt: Text("Codex"))
                            .accessibilityLabel(L10n.string("Profile Name"))
                    }
                }
                LabeledContent(L10n.string("Command")) {
                    HStack(spacing: 8) {
                        TextField("", text: $model.agentCommand, prompt: Text("codex"))
                            .accessibilityLabel(L10n.string("Agent command"))
                        Button(L10n.string("Choose…")) { model.chooseAgentCommand() }
                            .accessibilityLabel(L10n.string("Choose Agent executable"))
                    }
                }
                probeStatus
            }
            Section(L10n.string("Arguments")) {
                ForEach<[Int], Int, ArgumentRow>(indices, id: \.self) { index in
                    ArgumentRow(model: model, index: index)
                }
                Button { model.addArgument() } label: {
                    Label(L10n.string("Add Argument"), systemImage: "plus")
                }
                .accessibilityLabel(L10n.string("Add Argument"))
                Label(model.guideIncludesPlaceholder
                        ? L10n.string("Arguments include the {guide} placeholder")
                        : L10n.string("Arguments must include {guide}"),
                      systemImage: model.guideIncludesPlaceholder ? "checkmark.circle" : "exclamationmark.triangle")
                    .foregroundStyle(model.guideIncludesPlaceholder ? Color.secondary : Color.orange)
            }

            Section(L10n.string("Agent Guide")) {
                Text(model.guideOverridden
                        ? L10n.string("Using override file: %@", model.guidePath)
                        : L10n.string("Using the built-in guide"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                HStack(spacing: 10) {
                    Button(L10n.string("Save Guide")) { Task { await model.saveGuide() } }
                        .disabled(model.guideText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityLabel(L10n.string("Save Agent guide"))
                    Button(L10n.string("Restore Built-in Guide")) { confirmResetGuide = true }
                        .disabled(!model.guideOverridden)
                        .accessibilityLabel(L10n.string("Restore built-in Agent guide"))
                    if let message = model.guideMessage {
                        Label(message, systemImage: model.guideMessageIsError ? "exclamationmark.triangle" : "checkmark.circle")
                            .foregroundStyle(model.guideMessageIsError ? Color.red : Color.secondary)
                    }
                }
                TextEditor(text: $model.guideText)
                    .font(.system(.body, design: .monospaced))
                    .frame(minHeight: 100)
                    .accessibilityLabel(L10n.string("Agent guide text"))
                    .accessibilityHint(L10n.string("Saved to agent-guide.md and passed to the next Agent launch."))
            }
        }
        .formStyle(.grouped)
        .onChange(of: model.agentArgs) {
            model.syncCurrentProfileFromFields()
            model.scheduleProbe()
        }
        .onChange(of: model.agentCommand) {
            model.syncCurrentProfileFromFields()
            model.scheduleProbe()
        }
        .alert(L10n.string("Restore the built-in guide?"), isPresented: $confirmResetGuide) {
            Button(L10n.string("Restore"), role: .destructive) { Task { await model.resetGuide() } }
            Button(L10n.string("Cancel"), role: .cancel) {}
        } message: {
            Text(L10n.string("This removes agent-guide.md from the configuration folder. Notes are not affected."))
        }
    }

    @ViewBuilder private var detectedAgents: some View {
        if model.agentScanning && model.detectedAgents.isEmpty {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(L10n.string("Scanning for installed Agents…")).foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel(L10n.string("Scanning for installed Agents"))
        } else if model.detectedAgents.isEmpty {
            if model.agentScanComplete {
                Text(L10n.string("No known Agent CLI was found. Enter a command below or choose an executable."))
                    .foregroundStyle(.secondary)
            }
        } else {
            ForEach(model.detectedAgents) { agent in
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(agent.displayName).font(.body.weight(.medium))
                        Text(agent.executable)
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                    }
                    Spacer(minLength: 8)
                    Button(model.isAgentSelected(agent) ? L10n.string("Selected") : L10n.string("Use")) {
                        model.useAgent(agent)
                    }
                    .disabled(model.isAgentSelected(agent))
                    .accessibilityLabel(model.isAgentSelected(agent)
                                            ? L10n.string("Selected %@", agent.displayName)
                                            : L10n.string("Use %@", agent.displayName))
                }
                .accessibilityElement(children: .contain)
                .accessibilityLabel(agent.displayName)
            }
        }
        HStack(spacing: 10) {
            Button(L10n.string("Rescan")) { model.refreshDetectedAgents() }
                .disabled(model.agentScanning)
                .accessibilityLabel(L10n.string("Rescan for installed Agents"))
            if model.agentScanning, !model.detectedAgents.isEmpty {
                ProgressView().controlSize(.small)
                    .accessibilityLabel(L10n.string("Scanning for installed Agents"))
            }
            Spacer()
        }
    }

    @ViewBuilder private var probeStatus: some View {
        if let probe = model.probe {
            if probe.available, let executable = probe.executable {
                Label(L10n.string("Available: %@", executable), systemImage: "checkmark.circle")
                    .foregroundStyle(.green)
            } else {
                Label(probeMessage(probe), systemImage: "xmark.octagon")
                    .foregroundStyle(.orange)
            }
        } else {
            Text(L10n.string("Not checked")).foregroundStyle(.secondary)
        }
    }

    private func probeMessage(_ probe: AgentProbeInfo) -> String {
        guard let key = probe.error else { return L10n.string("Not available") }
        if key.contains("."), !key.contains(" ") {
            return L10n.errorKey(key)
        }
        return key
    }
}

private struct ArgumentRow: View {
    @Bindable var model: SettingsModel
    let index: Int

    var body: some View {
        HStack(spacing: 8) {
            TextField(L10n.string("Argument %d", index + 1), text: binding)
                .accessibilityLabel(L10n.string("Argument %d", index + 1))
            Button { model.moveArgument(at: index, by: -1) } label: {
                Image(systemName: "arrow.up")
            }
            .buttonStyle(.borderless)
            .disabled(index == 0)
            .accessibilityLabel(L10n.string("Move argument up"))
            Button { model.moveArgument(at: index, by: 1) } label: {
                Image(systemName: "arrow.down")
            }
            .buttonStyle(.borderless)
            .disabled(index + 1 >= model.agentArgs.count)
            .accessibilityLabel(L10n.string("Move argument down"))
            Button(role: .destructive) { model.removeArgument(at: index) } label: {
                Image(systemName: "minus.circle")
            }
            .buttonStyle(.borderless)
            .accessibilityLabel(L10n.string("Remove argument"))
        }
    }

    private var binding: Binding<String> {
        Binding(
            get: { model.agentArgs.indices.contains(index) ? model.agentArgs[index] : "" },
            set: { value in
                guard model.agentArgs.indices.contains(index) else { return }
                model.agentArgs[index] = value
            })
    }
}

private struct ConfigurationSettingsView: View {
    @Bindable var model: SettingsModel

    var body: some View {
        Form {
            Section(L10n.string("Configuration File")) {
                LabeledContent(L10n.string("Path")) {
                    Text(model.configPath)
                        .font(.system(.callout, design: .monospaced))
                        .textSelection(.enabled)
                        .multilineTextAlignment(.trailing)
                }
                HStack(spacing: 10) {
                    Button(L10n.string("Open config.toml")) { model.openConfigFile() }
                        .disabled(model.configPath.isEmpty)
                        .accessibilityLabel(L10n.string("Open config.toml"))
                    Button(L10n.string("Reload from Disk")) { Task { await model.load() } }
                        .accessibilityLabel(L10n.string("Reload configuration from disk"))
                }
            }
            Section(L10n.string("Command Line & MCP")) {
                LabeledContent(L10n.string("Command Line Tool")) {
                    cliStatusView
                }
                HStack(spacing: 10) {
                    Button(L10n.string("Link Command Line Tool")) { model.installCLISymlink() }
                        .accessibilityLabel(L10n.string("Link Command Line Tool"))
                    Button(L10n.string("Copy MCP Configuration")) { model.copyMCPConfiguration() }
                        .accessibilityLabel(L10n.string("Copy MCP Configuration"))
                }
                if let message = model.cliMessage {
                    Label(message, systemImage: model.cliMessageIsError ? "exclamationmark.triangle" : "checkmark.circle")
                        .foregroundStyle(model.cliMessageIsError ? Color.red : Color.secondary)
                        .lineLimit(2)
                }
            }
            if !model.diagnostics.isEmpty {
                Section(L10n.string("Diagnostics")) {
                    ForEach(model.diagnostics, id: \.self) { key in
                        Label(L10n.errorKey(key), systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .onAppear { model.refreshCLIStatus() }
    }

    @ViewBuilder private var cliStatusView: some View {
        switch model.cliStatus {
        case .notInstalled:
            Text(L10n.string("Not linked")).foregroundStyle(.secondary)
        case .linked:
            Label(L10n.string("Linked"), systemImage: "checkmark.circle").foregroundStyle(.green)
        case let .linkedElsewhere(target):
            Text(L10n.string("Linked elsewhere: %@", target))
                .font(.caption)
                .foregroundStyle(.orange)
                .lineLimit(1)
                .truncationMode(.middle)
        case .conflictFile:
            Label(L10n.string("Existing file blocks ~/.local/bin/hortus"), systemImage: "exclamationmark.triangle")
                .foregroundStyle(.orange)
        }
    }
}
