import Foundation
import HortusApple
import HortusCore
import SwiftUI
import UIKit

enum AppAppearance: String, Codable, CaseIterable, Identifiable {
    case system
    case light
    case dark

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: return L10n.string("System")
        case .light: return L10n.string("Light")
        case .dark: return L10n.string("Dark")
        }
    }

    var colorScheme: ColorScheme? {
        switch self {
        case .system: return nil
        case .light: return .light
        case .dark: return .dark
        }
    }

    var interfaceStyle: UIUserInterfaceStyle {
        switch self {
        case .system: return .unspecified
        case .light: return .light
        case .dark: return .dark
        }
    }
}

struct IOSSettingsPayload: Codable {
    var appearance: AppAppearance = .system
    var bodyFontSize: Double = 16.0
}

@Observable @MainActor
final class IOSSettingsStore {
    var appearance: AppAppearance = .system {
        didSet { if appearance != oldValue { persist() } }
    }
    var bodyFontSize: Double = 16.0 {
        didSet {
            let clamped = min(28.0, max(12.0, bodyFontSize))
            if clamped != bodyFontSize {
                bodyFontSize = clamped
                return
            }
            if bodyFontSize != oldValue {
                persist()
                syncConfigFontSize()
            }
        }
    }
    var cacheClearedNotice = false

    private let settingsURL: URL
    private let cacheDirectory: URL
    private weak var core: WorkspaceCore?
    private var loading = false
    private var persistTask: Task<Void, Never>?
    private var syncFontTask: Task<Void, Never>?
    private let writeQueue = DispatchQueue(label: "app.hortus.ios.settings", qos: .utility)

    init(supportDirectory: URL, cacheDirectory: URL) {
        self.settingsURL = supportDirectory.appendingPathComponent("ios-settings.json")
        self.cacheDirectory = cacheDirectory
        loadFromDisk()
    }

    func attachCore(_ core: WorkspaceCore?) {
        self.core = core
        guard let core else { return }
        if let configured = try? core.fontSize(), (12.0...28.0).contains(configured),
           !FileManager.default.fileExists(atPath: settingsURL.path) {
            loading = true
            bodyFontSize = configured
            loading = false
        }
    }

    func applyOverrides(appearance: AppAppearance?, fontSize: Double?) {
        loading = true
        if let appearance { self.appearance = appearance }
        if let fontSize { self.bodyFontSize = min(28.0, max(12.0, fontSize)) }
        loading = false
    }

    private func loadFromDisk() {
        guard let data = try? Data(contentsOf: settingsURL),
              let decoded = try? JSONDecoder().decode(IOSSettingsPayload.self, from: data) else {
            return
        }
        loading = true
        appearance = decoded.appearance
        bodyFontSize = min(28.0, max(12.0, decoded.bodyFontSize))
        loading = false
    }

    private func persist() {
        guard !loading else { return }
        persistTask?.cancel()
        persistTask = Task { @MainActor [weak self] in
            do { try await Task.sleep(for: .milliseconds(150)) } catch { return }
            guard let self, !Task.isCancelled else { return }
            let payload = IOSSettingsPayload(appearance: self.appearance, bodyFontSize: self.bodyFontSize)
            let url = self.settingsURL
            self.writeQueue.async {
                try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                if let data = try? JSONEncoder().encode(payload) {
                    try? data.write(to: url, options: .atomic)
                }
            }
        }
    }

    private func syncConfigFontSize() {
        guard !loading, let core else { return }
        syncFontTask?.cancel()
        syncFontTask = Task { @MainActor [weak self] in
            do { try await Task.sleep(for: .milliseconds(150)) } catch { return }
            guard let self, !Task.isCancelled else { return }
            let size = self.bodyFontSize
            self.writeQueue.async {
                guard let current = try? core.editableConfig() else { return }
                let input = EditableConfigInputInfo(
                    fontSize: size,
                    defaultView: current.defaultView,
                    terminalFont: current.terminalFont,
                    agentCommand: current.agentCommand,
                    agentArgs: current.agentArgs
                )
                _ = try? core.saveEditableConfig(input: input, expectedDigest: current.digest)
            }
        }
    }

    func flushPendingWrites() async {
        persistTask?.cancel()
        syncFontTask?.cancel()
        let payload = IOSSettingsPayload(appearance: appearance, bodyFontSize: bodyFontSize)
        let url = settingsURL
        let core = core
        await withCheckedContinuation { continuation in
            writeQueue.async {
                try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                if let data = try? JSONEncoder().encode(payload) {
                    try? data.write(to: url, options: .atomic)
                }
                if let core, let current = try? core.editableConfig() {
                    let input = EditableConfigInputInfo(
                        fontSize: payload.bodyFontSize,
                        defaultView: current.defaultView,
                        terminalFont: current.terminalFont,
                        agentCommand: current.agentCommand,
                        agentArgs: current.agentArgs
                    )
                    _ = try? core.saveEditableConfig(input: input, expectedDigest: current.digest)
                }
                continuation.resume()
            }
        }
    }

    func clearCaches(diagramService: DiagramService) async {
        await MathService.shared.clearCache()
        await LocalImageCache.shared.clear()
        await diagramService.clearCache()
        let cacheDir = cacheDirectory
        let preserved: Set<String> = ["session.toml", "recovery"]
        await Task.detached(priority: .utility) {
            let fm = FileManager.default
            if let items = try? fm.contentsOfDirectory(at: cacheDir, includingPropertiesForKeys: nil) {
                for item in items where !preserved.contains(item.lastPathComponent) {
                    try? fm.removeItem(at: item)
                }
            }
            let tmpExports = fm.temporaryDirectory.appendingPathComponent("hortus-exports")
            try? fm.removeItem(at: tmpExports)
        }.value
        cacheClearedNotice = true
    }
}

struct IOSSettingsView: View {
    @Bindable var settings: IOSSettingsStore
    let diagramService: DiagramService
    @Environment(\.dismiss) private var dismiss

    private var appVersion: String {
        let short = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0.0"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
        return "\(short) (\(build))"
    }

    var body: some View {
        NavigationStack {
            Form {
                Section(L10n.string("Appearance")) {
                    Picker(L10n.string("Appearance"), selection: $settings.appearance) {
                        ForEach(AppAppearance.allCases) { item in
                            Text(item.title).tag(item)
                        }
                    }
                    .pickerStyle(.segmented)
                    .accessibilityIdentifier("settings-appearance")
                }

                Section(L10n.string("Body font size")) {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Text(L10n.string("Body font size"))
                            Spacer()
                            Text("\(Int(settings.bodyFontSize.rounded())) \(L10n.string("pt"))")
                                .monospacedDigit()
                                .foregroundStyle(.secondary)
                                .accessibilityIdentifier("settings-font-size-value")
                        }
                        HStack(spacing: 12) {
                            Slider(value: $settings.bodyFontSize, in: 12...28, step: 1)
                                .accessibilityIdentifier("settings-font-size-slider")
                            Stepper("", value: $settings.bodyFontSize, in: 12...28, step: 1)
                                .labelsHidden()
                                .accessibilityIdentifier("settings-font-size-stepper")
                        }
                    }
                }

                Section {
                    Button {
                        Task { await settings.clearCaches(diagramService: diagramService) }
                    } label: {
                        HStack {
                            Text(L10n.string("Clear Cache"))
                            Spacer()
                            if settings.cacheClearedNotice {
                                Text(L10n.string("Cache cleared"))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    .accessibilityIdentifier("settings-clear-cache")
                }

                Section(L10n.string("About")) {
                    LabeledContent(L10n.string("Hortus"), value: appVersion)
                    LabeledContent(L10n.string("License"), value: L10n.string("GPL-3.0-only"))
                    if let url = URL(string: "https://github.com/groundgrounder/Hortus") {
                        Link(destination: url) {
                            HStack {
                                Text(L10n.string("Repository"))
                                Spacer()
                                Text("github.com/groundgrounder/Hortus")
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                            }
                        }
                    }
                }
            }
            .navigationTitle(L10n.string("Settings"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(L10n.string("Done")) { dismiss() }
                        .accessibilityIdentifier("settings-done")
                }
            }
            .onAppear { settings.cacheClearedNotice = false }
        }
    }
}
