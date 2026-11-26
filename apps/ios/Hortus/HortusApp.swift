import HortusApple
import HortusCore
import SwiftUI
import UniformTypeIdentifiers

@main
struct HortusApp: App {
    @State private var appModel = IOSAppModel()

    var body: some Scene {
        WindowGroup {
            IOSRootView(appModel: appModel)
                .preferredColorScheme(appModel.settings.appearance.colorScheme)
        }
    }
}

struct IOSRootView: View {
    @Bindable var appModel: IOSAppModel
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        Group {
            if appModel.vaultHandle == nil {
                IOSWelcomeView(appModel: appModel)
            } else if sizeClass == .regular {
                NavigationSplitView {
                    IOSKnowledgeBrowser(appModel: appModel)
                        .navigationTitle(appModel.vaultName)
                        .toolbar { sidebarToolbar }
                } detail: {
                    NavigationStack {
                        IOSDocumentDetailView(appModel: appModel, isCompact: false)
                    }
                }
            } else {
                NavigationStack {
                    IOSDocumentDetailView(appModel: appModel, isCompact: true)
                }
            }
        }
        .fileImporter(
            isPresented: $appModel.fileImporterPresented,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false
        ) { result in
            switch result {
            case .success(let urls):
                if let url = urls.first {
                    Task { await appModel.openVault(url: url, securityScoped: true) }
                }
            case .failure(let error):
                appModel.errorMessage = error.localizedDescription
            }
        }
        .onChange(of: scenePhase) { _, phase in
            appModel.handleScenePhase(phase)
        }
        .sheet(isPresented: $appModel.settingsPresented) {
            IOSSettingsView(settings: appModel.settings, diagramService: appModel.diagramService)
        }
        .sheet(isPresented: Binding(
            get: { appModel.knowledgeSheetPresented && sizeClass == .compact },
            set: { appModel.knowledgeSheetPresented = $0 }
        )) {
            NavigationStack {
                IOSKnowledgeBrowser(appModel: appModel)
                    .navigationTitle(appModel.selectedTab.title)
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            if appModel.selectedTab == .files {
                                Button {
                                    appModel.knowledgeSheetPresented = false
                                    appModel.beginNewNote()
                                } label: {
                                    Image(systemName: "square.and.pencil")
                                        .accessibilityLabel(L10n.string("New Note"))
                                }
                            }
                        }
                        ToolbarItem(placement: .confirmationAction) {
                            Button(L10n.string("Done")) {
                                appModel.knowledgeSheetPresented = false
                            }
                        }
                    }
            }
        }
        .fullScreenCover(item: $appModel.activeFocus) { session in
            IOSDiagramFocusView(session: session, appModel: appModel)
        }
        .alert(L10n.string("New Note"), isPresented: $appModel.newNoteAlertPresented) {
            TextField(L10n.string("Name"), text: $appModel.newNoteNameInput)
            Button(L10n.string("Create")) { appModel.confirmNewNote() }
            Button(L10n.string("Cancel"), role: .cancel) {}
        } message: {
            if !appModel.newNoteError.isEmpty {
                Text(appModel.newNoteError)
            }
        }
        .alert(L10n.string("Error"), isPresented: Binding(
            get: { appModel.errorMessage != nil },
            set: { if !$0 { appModel.errorMessage = nil } }
        )) {
            Button(L10n.string("OK"), role: .cancel) { appModel.errorMessage = nil }
        } message: {
            Text(appModel.errorMessage ?? "")
        }
    }

    @ToolbarContentBuilder
    private var sidebarToolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .topBarTrailing) {
            Button {
                appModel.beginNewNote()
            } label: {
                Image(systemName: "square.and.pencil")
                    .accessibilityLabel(L10n.string("New Note"))
            }
            .accessibilityIdentifier("toolbar-new-note")
            Menu {
                Button {
                    appModel.fileImporterPresented = true
                } label: {
                    Label(L10n.string("Change Folder…"), systemImage: "folder")
                }
                Button {
                    appModel.settingsPresented = true
                } label: {
                    Label(L10n.string("Settings"), systemImage: "gearshape")
                }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .accessibilityLabel(L10n.string("More"))
            }
            .accessibilityIdentifier("toolbar-more")
        }
    }
}

private struct IOSWelcomeView: View {
    @Bindable var appModel: IOSAppModel

    var body: some View {
        NavigationStack {
            VStack(spacing: 20) {
                Spacer()
                Image(systemName: "leaf")
                    .font(.system(size: 44, weight: .light))
                    .foregroundStyle(.tint)
                Text(L10n.string("Hortus"))
                    .font(.largeTitle.weight(.semibold))
                Text(L10n.string("Open a folder to start reading."))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                VStack(spacing: 12) {
                    Button {
                        appModel.fileImporterPresented = true
                    } label: {
                        Text(L10n.string("Open Folder…"))
                            .frame(maxWidth: 260)
                    }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("welcome-open-folder")

                    if appModel.hasBundledSampleFolder() {
                        Button {
                            Task { await appModel.importSampleFolder() }
                        } label: {
                            Text(L10n.string("Import Sample Folder"))
                                .frame(maxWidth: 260)
                        }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("welcome-import-sample")
                    }
                }
                Spacer()
            }
            .padding(24)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        appModel.settingsPresented = true
                    } label: {
                        Image(systemName: "gearshape")
                            .accessibilityLabel(L10n.string("Settings"))
                    }
                    .accessibilityIdentifier("welcome-settings")
                }
            }
        }
    }
}

private struct IOSDocumentDetailView: View {
    @Bindable var appModel: IOSAppModel
    let isCompact: Bool

    var body: some View {
        Group {
            if let document = appModel.currentDocument, let path = appModel.currentPath {
                VStack(spacing: 0) {
                    if document.issue == .conflict || document.issue == .recovery || document.issue == .missing {
                        IOSConflictBanner(document: document)
                    } else if let reason = document.readOnlyReason {
                        HStack {
                            Text(L10n.documentMessage(issue: document.issue, readOnlyReason: reason, raw: document.message))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Spacer()
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 6)
                        .background(Color(uiColor: .secondarySystemBackground))
                    }

                    if document.reading {
                        IOSReadingTextView(
                            document: document,
                            path: path,
                            vault: appModel.vaultRoot ?? "",
                            fontSize: appModel.settings.bodyFontSize,
                            diagramService: appModel.diagramService,
                            core: appModel.core,
                            vaultHandle: appModel.vaultHandle,
                            knowledgeVersion: appModel.knowledgeVersion,
                            refreshToken: appModel.refreshToken,
                            onOpenLink: { target, heading in
                                appModel.openLink(target: target, heading: heading)
                            },
                            onShowTag: { tag in
                                appModel.selectedTag = tag
                                appModel.selectedTab = .tags
                                if isCompact { appModel.knowledgeSheetPresented = true }
                            },
                            onFocus: { ordinal in
                                appModel.openFocus(ordinal: ordinal)
                            },
                            onToggleDiagram: { ordinal, collapsed in
                                appModel.rememberDiagramCollapsed(ordinal: ordinal, collapsed: collapsed)
                                appModel.refreshToken &+= 1
                            }
                        )
                    } else {
                        IOSEditingTextView(document: document, fontSize: appModel.settings.bodyFontSize)
                    }

                    if isCompact {
                        Divider()
                        compactBottomBar
                    }
                }
                .navigationTitle(URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    if isCompact {
                        ToolbarItem(placement: .topBarLeading) {
                            Button {
                                appModel.selectedTab = .files
                                appModel.knowledgeSheetPresented = true
                            } label: {
                                Image(systemName: "sidebar.left")
                                    .accessibilityLabel(L10n.string("Files"))
                            }
                            .accessibilityIdentifier("toolbar-files")
                        }
                    }
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        if !document.reading, document.dirty {
                            Button(L10n.string("Save")) {
                                Task { await document.saveNow() }
                            }
                            .accessibilityIdentifier("toolbar-save")
                        }
                        Button {
                            Task { await appModel.toggleReadingMode() }
                        } label: {
                            Image(systemName: document.reading ? "pencil" : "book")
                                .accessibilityLabel(L10n.string(document.reading ? "Edit" : "Read"))
                        }
                        .accessibilityIdentifier("toolbar-toggle-mode")

                        if isCompact {
                            Menu {
                                Button {
                                    appModel.beginNewNote()
                                } label: {
                                    Label(L10n.string("New Note"), systemImage: "square.and.pencil")
                                }
                                Button {
                                    appModel.fileImporterPresented = true
                                } label: {
                                    Label(L10n.string("Change Folder…"), systemImage: "folder")
                                }
                                Button {
                                    appModel.settingsPresented = true
                                } label: {
                                    Label(L10n.string("Settings"), systemImage: "gearshape")
                                }
                            } label: {
                                Image(systemName: "ellipsis.circle")
                                    .accessibilityLabel(L10n.string("More"))
                            }
                            .accessibilityIdentifier("toolbar-more")
                        }
                    }
                }
            } else {
                ContentUnavailableView(L10n.string("Select a note"), systemImage: "doc.text")
            }
        }
    }

    private var compactBottomBar: some View {
        HStack {
            ForEach(KnowledgeTab.allCases) { tab in
                Button {
                    appModel.selectedTab = tab
                    appModel.knowledgeSheetPresented = true
                } label: {
                    VStack(spacing: 3) {
                        Image(systemName: tab.systemImage)
                            .font(.system(size: 16))
                        Text(tab.title)
                            .font(.caption2)
                    }
                    .frame(maxWidth: .infinity)
                }
                .buttonStyle(.plain)
                .foregroundStyle(appModel.selectedTab == tab && appModel.knowledgeSheetPresented ? Color.accentColor : Color.secondary)
                .accessibilityIdentifier("bottom-tab-\(tab.rawValue)")
            }
        }
        .padding(.vertical, 6)
        .background(.bar)
    }
}

private struct IOSConflictBanner: View {
    @Bindable var document: IOSNoteDocument

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(L10n.documentMessage(issue: document.issue, readOnlyReason: document.readOnlyReason, raw: document.message))
                .font(.caption.weight(.medium))
            HStack(spacing: 8) {
                if document.issue == .recovery {
                    Button(L10n.string("Restore")) {
                        Task { await document.recover(true) }
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    Button(L10n.string("Discard")) {
                        Task { await document.recover(false) }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                } else {
                    Button(document.issue == .missing ? L10n.string("Save to Original Path") : L10n.string("Keep Mine")) {
                        Task { await document.saveNow(document.issue == .missing ? .recreate : .keepMine) }
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    Button(L10n.string("Use Disk Version")) {
                        Task { await document.useDisk() }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    Button(L10n.string("Save Conflict Copy")) {
                        Task { await document.saveNow(.conflictCopy) }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(uiColor: .secondarySystemBackground))
    }
}
