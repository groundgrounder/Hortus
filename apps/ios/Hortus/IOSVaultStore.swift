import Foundation
import HortusApple
import HortusCore
import SwiftUI
import UIKit

enum KnowledgeTab: String, CaseIterable, Identifiable {
    case files
    case outline
    case backlinks
    case tags
    case search

    var id: String { rawValue }

    var title: String {
        switch self {
        case .files: return L10n.string("Files")
        case .outline: return L10n.string("Outline")
        case .backlinks: return L10n.string("Backlinks")
        case .tags: return L10n.string("Tags")
        case .search: return L10n.string("Search")
        }
    }

    var systemImage: String {
        switch self {
        case .files: return "folder"
        case .outline: return "list.bullet.indent"
        case .backlinks: return "arrow.uturn.backward"
        case .tags: return "number"
        case .search: return "magnifyingglass"
        }
    }
}

struct IOSDiagramFocusSession: Identifiable {
    let id = UUID()
    let note: String
    let vault: String
    let ordinal: UInt32
    let document: IOSNoteDocument
    let initial: DiagramFocus?
}

@Observable @MainActor
final class IOSAppModel {
    let core: WorkspaceCore?
    let settings: IOSSettingsStore
    let diagramService = DiagramService()

    var vaultRoot: String?
    var vaultName: String {
        guard let vaultRoot else { return L10n.string("Hortus") }
        return URL(fileURLWithPath: vaultRoot).lastPathComponent
    }
    var vaultHandle: VaultHandle?
    var entries: [String: TreeEntry] = [:]
    var treeVersion = 0
    var knowledgeVersion = 0
    var scanning = false

    var currentPath: String?
    var currentDocument: IOSNoteDocument?
    var refreshToken = 0
    var errorMessage: String?

    var selectedTab: KnowledgeTab = .files
    var searchQuery = ""
    var quickOpenHits: [QuickOpenHit] = []
    var searchResults: [SearchResult] = []
    var selectedTag: String?
    var tagFilter = ""

    var activeFocus: IOSDiagramFocusSession?
    var settingsPresented = false
    var knowledgeSheetPresented = false
    var fileImporterPresented = false
    var newNoteAlertPresented = false
    var newNoteNameInput = ""
    var newNoteError = ""

    private var subscription: VaultSubscription?
    private var activeSecurityScopedURL: URL?
    private var searchTask: Task<Void, Never>?
    private var searchCancellation: SearchCancellation?
    private var openNoteTask: Task<Void, Never>?
    private var openRequestID: UInt64 = 0
    private var vaultGeneration: UInt64 = 0
    private var togglingReadingMode = false
    private var hasRequestedNoteInVault = false
    private var suppressAutoStartNote = false
    private var preserveKnowledgeSheetOnOpen = false
    private var pendingLaunchFocus: UInt32?
    private var pendingLaunchReveal: String?
    private let supportDirectory: URL
    private let bookmarksURL: URL
    @ObservationIgnored var beforeOpenDocumentForTesting: ((String) async -> Void)?

    init() {
        let fm = FileManager.default
        let appSupport = (try? fm.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true))
            ?? fm.temporaryDirectory
        let caches = (try? fm.url(for: .cachesDirectory, in: .userDomainMask, appropriateFor: nil, create: true))
            ?? fm.temporaryDirectory
        let support = appSupport.appendingPathComponent("Hortus", isDirectory: true)
        let configDir = support.appendingPathComponent("config", isDirectory: true)
        let dataDir = support.appendingPathComponent("data", isDirectory: true)
        let cacheDir = caches.appendingPathComponent("Hortus", isDirectory: true)
        try? fm.createDirectory(at: configDir, withIntermediateDirectories: true)
        try? fm.createDirectory(at: dataDir, withIntermediateDirectories: true)
        try? fm.createDirectory(at: cacheDir, withIntermediateDirectories: true)
        setenv("HORTUS_DATA_DIR", dataDir.path, 1)

        self.supportDirectory = support
        self.bookmarksURL = support.appendingPathComponent("bookmarks.plist")
        self.settings = IOSSettingsStore(supportDirectory: support, cacheDirectory: cacheDir)
        self.core = try? WorkspaceCore.inDirectories(config: configDir.path, cache: cacheDir.path)
        self.settings.attachCore(self.core)

        #if DEBUG
        if applyLaunchArgumentsIfNeeded() {
            return
        }
        #endif
        restoreLastVaultIfAvailable()
    }

    var sortedTreeEntries: [TreeEntry] {
        entries.values.sorted { a, b in
            if a.directory != b.directory { return a.directory && !b.directory }
            return a.path.localizedStandardCompare(b.path) == .orderedAscending
        }
    }

    func childEntries(of parent: String) -> [TreeEntry] {
        entries.values
            .filter { $0.parent == parent }
            .sorted { a, b in
                if a.directory != b.directory { return a.directory && !b.directory }
                return a.name.localizedStandardCompare(b.name) == .orderedAscending
            }
    }

    func hasBundledSampleFolder() -> Bool {
        Bundle.main.url(forResource: "demo-vault", withExtension: nil) != nil
    }

    private func prepareDemoFolder() -> URL? {
        guard let source = Bundle.main.url(forResource: "demo-vault", withExtension: nil),
              let docs = try? FileManager.default.url(for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true) else {
            errorMessage = L10n.string("Operation failed")
            return nil
        }
        let target = docs.appendingPathComponent("HortusDemo", isDirectory: true)
        do {
            if !FileManager.default.fileExists(atPath: target.path) {
                try FileManager.default.copyItem(at: source, to: target)
            }
            return target
        } catch {
            errorMessage = L10n.error(error)
            return nil
        }
    }

    func importSampleFolder() async {
        guard let target = prepareDemoFolder() else { return }
        await openVault(url: target, securityScoped: false)
    }

    func openPickedFolder(_ result: Result<URL, Error>) async {
        switch result {
        case .success(let url):
            await openVault(url: url, securityScoped: true)
        case .failure(let error):
            errorMessage = error.localizedDescription
        }
    }

    func handleScenePhase(_ phase: ScenePhase) {
        guard phase == .inactive || phase == .background else { return }
        Task { await settings.flushPendingWrites() }
        guard let doc = currentDocument, doc.dirty || doc.marked || doc.issue == .missing else { return }
        flushDocumentForBackground(doc)
    }

    @discardableResult
    func flushDocumentForBackground(_ doc: IOSNoteDocument) -> Task<DocumentPersistence, Never> {
        var taskID: UIBackgroundTaskIdentifier = .invalid
        taskID = UIApplication.shared.beginBackgroundTask(withName: "hortus-prepare-close") {
            Task { @MainActor in
                if taskID != .invalid {
                    UIApplication.shared.endBackgroundTask(taskID)
                    taskID = .invalid
                }
            }
        }
        return Task { @MainActor in
            let persistence = await doc.prepareToClose()
            if taskID != .invalid {
                UIApplication.shared.endBackgroundTask(taskID)
                taskID = .invalid
            }
            return persistence
        }
    }

    private func openVaultImmediate(url: URL, securityScoped: Bool) {
        vaultGeneration &+= 1
        let accessed = securityScoped ? url.startAccessingSecurityScopedResource() : false
        activeSecurityScopedURL = accessed ? url : nil
        openVaultCore(url: url, securityScoped: securityScoped)
    }

    func openVault(url: URL, securityScoped: Bool) async {
        vaultGeneration &+= 1
        let expectedGen = vaultGeneration
        let normalizedTarget = url.standardizedFileURL
        let previousScoped = activeSecurityScopedURL
        let reusingScoped = securityScoped && previousScoped?.standardizedFileURL == normalizedTarget
        let newlyAccessed = (securityScoped && !reusingScoped) ? url.startAccessingSecurityScopedResource() : false

        await closeCurrentVault(stopSecurity: false, incrementGeneration: false)
        guard expectedGen == vaultGeneration else {
            if newlyAccessed {
                url.stopAccessingSecurityScopedResource()
            }
            return
        }
        if let previousScoped, !reusingScoped {
            previousScoped.stopAccessingSecurityScopedResource()
        }
        activeSecurityScopedURL = reusingScoped ? previousScoped : (newlyAccessed ? url : nil)
        openVaultCore(url: url, securityScoped: securityScoped)
    }

    private func openVaultCore(url: URL, securityScoped: Bool) {
        hasRequestedNoteInVault = false
        if securityScoped {
            saveBookmark(for: url)
        }

        let path = url.path
        let expectedGen = vaultGeneration
        do {
            guard let core else { throw CoreError.Failure(kind: "config.home_missing") }
            let handle = try core.openVault(path: path)
            vaultRoot = path
            vaultHandle = handle
            scanning = true
            entries = [:]
            treeVersion += 1
            subscription = VaultSubscription(handle: handle) { [weak self] batches in
                Task { @MainActor [weak self] in
                    self?.applyVaultBatches(batches, generation: expectedGen)
                }
                let upserted = batches.flatMap { $0.upserted.filter { !$0.directory }.map(\.path) }
                let removed = batches.flatMap(\.removed)
                handle.updateSearchIndex(upserted: upserted, removed: removed)
                let changed = !upserted.isEmpty || !removed.isEmpty
                let scanFinished = batches.contains(where: \.scanComplete)
                if changed || scanFinished {
                    Task { @MainActor [weak self] in
                        guard let self, self.vaultGeneration == expectedGen else { return }
                        self.knowledgeVersion &+= 1
                    }
                }
            }
            try? core.remember(path: path, tabs: [], selected: nil)
        } catch {
            if let scoped = activeSecurityScopedURL {
                scoped.stopAccessingSecurityScopedResource()
                activeSecurityScopedURL = nil
            }
            errorMessage = L10n.error(error)
        }
    }

    func closeCurrentVault(stopSecurity: Bool = true, incrementGeneration: Bool = true) async {
        if incrementGeneration {
            vaultGeneration &+= 1
        }
        hasRequestedNoteInVault = false
        openRequestID &+= 1
        openNoteTask?.cancel()
        openNoteTask = nil
        searchTask?.cancel()
        searchCancellation?.cancel()
        if let doc = currentDocument {
            if doc.dirty || doc.marked || doc.issue == .missing {
                _ = await doc.prepareToClose()
            }
            doc.stopForShutdown()
        }
        currentDocument = nil
        currentPath = nil
        activeFocus = nil
        if let oldSub = subscription {
            subscription = nil
            DispatchQueue.global(qos: .userInitiated).async { oldSub.close() }
        }
        vaultHandle = nil
        vaultRoot = nil
        entries = [:]
        if stopSecurity, let url = activeSecurityScopedURL {
            url.stopAccessingSecurityScopedResource()
            activeSecurityScopedURL = nil
        }
    }

    private func applyVaultBatches(_ batches: [TreeBatch], generation: UInt64) {
        guard generation == vaultGeneration else { return }
        for batch in batches {
            for removed in batch.removed { entries.removeValue(forKey: removed) }
            for item in batch.upserted { entries[item.path] = item }
            if batch.scanComplete { scanning = false }
        }
        treeVersion += 1
        if let currentPath, batches.contains(where: { $0.upserted.contains(where: { $0.path == currentPath }) }) {
            currentDocument?.refresh()
        }
        if !suppressAutoStartNote, !hasRequestedNoteInVault, currentPath == nil,
           let start = entries["Start here.md"]?.path ?? sortedTreeEntries.first(where: { !$0.directory })?.path {
            openNote(start)
        }
    }

    func openNote(_ relativePath: String, heading: String? = nil, sourceOffset: Int? = nil,
                  selectionLength: Int = 0, editing: Bool? = nil) {
        guard let core, let vaultRoot else { return }
        hasRequestedNoteInVault = true
        openRequestID &+= 1
        let requestID = openRequestID
        let expectedVaultGen = vaultGeneration
        openNoteTask?.cancel()

        if currentPath == relativePath, let doc = currentDocument {
            if let editing { doc.reading = !editing }
            if let sourceOffset {
                doc.pendingReveal = (sourceOffset, selectionLength)
                refreshToken &+= 1
            } else if let heading {
                revealHeadingTitle(heading, in: doc)
                refreshToken &+= 1
            }
            knowledgeSheetPresented = false
            return
        }

        let previous = currentDocument
        let beforeHook = beforeOpenDocumentForTesting
        openNoteTask = Task { [weak self] in
            if let previous, previous.dirty || previous.marked || previous.issue == .missing {
                _ = await previous.prepareToClose()
            }
            guard !Task.isCancelled, let self,
                  self.openRequestID == requestID,
                  self.vaultGeneration == expectedVaultGen else { return }
            previous?.stopForShutdown()
            if let beforeHook {
                await beforeHook(relativePath)
            }
            guard !Task.isCancelled,
                  self.openRequestID == requestID,
                  self.vaultGeneration == expectedVaultGen else { return }
            do {
                let (io, initial) = try await Task.detached(priority: .userInitiated) {
                    let session = try core.openDocument(vault: vaultRoot, path: relativePath)
                    let initial = try session.initial()
                    let url = URL(fileURLWithPath: vaultRoot).appendingPathComponent(relativePath)
                    return (DocumentIO(session: session, url: url), initial)
                }.value
                let doc = IOSNoteDocument(io: io, initial: initial)
                guard !Task.isCancelled,
                      self.openRequestID == requestID,
                      self.vaultGeneration == expectedVaultGen else {
                    doc.stopForShutdown()
                    return
                }
                if let editing { doc.reading = !editing }
                doc.onSaved = { [weak self, weak doc] in
                    guard let self, let doc, self.currentDocument === doc else { return }
                    self.updateIndexForNote(relativePath)
                }
                doc.onConflictCopy = { [weak self] copyPath in
                    self?.openNote(copyPath)
                }
                if let staleCurrent = self.currentDocument, staleCurrent !== doc {
                    staleCurrent.stopForShutdown()
                }
                self.currentPath = relativePath
                self.currentDocument = doc
                if self.preserveKnowledgeSheetOnOpen {
                    self.preserveKnowledgeSheetOnOpen = false
                } else {
                    self.knowledgeSheetPresented = false
                }
                if let focusOrdinal = self.pendingLaunchFocus {
                    self.pendingLaunchFocus = nil
                    self.openFocus(ordinal: focusOrdinal)
                }
                if let revealQuery = self.pendingLaunchReveal {
                    self.pendingLaunchReveal = nil
                    let range = (doc.text as NSString).range(of: revealQuery)
                    if range.location != NSNotFound {
                        doc.pendingReveal = (range.location, range.length)
                    }
                }
                if let sourceOffset {
                    doc.pendingReveal = (sourceOffset, selectionLength)
                } else if let heading {
                    self.revealHeadingTitle(heading, in: doc)
                }
                if let core = self.core {
                    Task.detached(priority: .utility) {
                        try? core.remember(path: vaultRoot, tabs: [relativePath], selected: relativePath)
                    }
                }
            } catch {
                guard !Task.isCancelled,
                      self.openRequestID == requestID,
                      self.vaultGeneration == expectedVaultGen else { return }
                self.errorMessage = L10n.error(error)
            }
        }
    }

    private func revealHeadingTitle(_ heading: String, in document: IOSNoteDocument) {
        let normalized = heading.trimmingCharacters(in: .whitespacesAndNewlines)
        if let found = document.headings.first(where: { $0.title.caseInsensitiveCompare(normalized) == .orderedSame }) {
            document.pendingReveal = (Int(found.start), 0)
            return
        }
        // 大纲可能尚未落盘返回，优先匹配标题行再兜底正文锚点。
        let ns = document.text as NSString
        let escaped = NSRegularExpression.escapedPattern(for: normalized)
        if let regex = try? NSRegularExpression(pattern: "(?m)^#{1,6}\\s+\(escaped)\\s*$", options: [.caseInsensitive]),
           let match = regex.firstMatch(in: document.text, range: NSRange(location: 0, length: ns.length)) {
            document.pendingReveal = (match.range.location, match.range.length)
            return
        }
        let range = ns.range(of: normalized, options: [.caseInsensitive])
        if range.location != NSNotFound {
            document.pendingReveal = (range.location, range.length)
        }
    }

    func revealHeading(_ heading: DocumentHeading) {
        guard let doc = currentDocument else { return }
        doc.pendingReveal = (Int(heading.start), 0)
        refreshToken &+= 1
        knowledgeSheetPresented = false
    }

    func openLink(target: String, heading: String?) {
        let trimmedTarget = target.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmedTarget.isEmpty, let heading,
           !heading.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
           let doc = currentDocument {
            revealHeadingTitle(heading, in: doc)
            refreshToken &+= 1
            knowledgeSheetPresented = false
            return
        }
        guard let vaultHandle, let currentPath else { return }
        Task {
            let resolved = await Task.detached {
                vaultHandle.resolveLink(sourcePath: currentPath, rawTarget: target)
            }.value
            if let path = resolved.resolvedPath {
                openNote(path, heading: heading)
            } else if !resolved.candidates.isEmpty {
                errorMessage = L10n.string("Several notes match “%@”: %@", target, resolved.candidates.joined(separator: ", "))
            } else {
                errorMessage = L10n.string("No note found for “%@”", target)
            }
        }
    }

    func openBacklink(_ entry: BacklinkInfo) {
        openNote(entry.source, sourceOffset: Int(entry.startUtf16), selectionLength: Int(entry.lengthUtf16))
    }

    func toggleReadingMode() async {
        guard !togglingReadingMode, let doc = currentDocument else { return }
        togglingReadingMode = true
        defer { togglingReadingMode = false }
        if !doc.reading {
            doc.finishEditingSession()
            if doc.dirty {
                await doc.saveNow()
            }
            guard currentDocument === doc else { return }
            doc.reading = true
        } else {
            doc.reading = false
        }
        refreshToken &+= 1
    }

    func beginNewNote() {
        newNoteNameInput = FileNamingRules.suggestedName(
            base: L10n.string("New Note"),
            ext: "md",
            parent: "",
            entries: entries
        )
        newNoteError = ""
        newNoteAlertPresented = true
    }

    func confirmNewNote() {
        let name = newNoteNameInput.trimmingCharacters(in: .whitespacesAndNewlines)
        if let err = FileNamingRules.localError(name) {
            newNoteError = err
            return
        }
        guard let vaultHandle else { return }
        Task {
            do {
                let created = try await Task.detached {
                    try vaultHandle.createNote(parent: "", name: name)
                }.value
                entries[created.path] = created
                treeVersion += 1
                updateIndexForNote(created.path)
                newNoteAlertPresented = false
                openNote(created.path, editing: true)
            } catch {
                newNoteError = L10n.error(error)
            }
        }
    }

    func updateIndexForNote(_ relativePath: String) {
        guard let handle = vaultHandle else { return }
        Task.detached(priority: .utility) { [weak self] in
            handle.updateSearchIndex(upserted: [relativePath], removed: [])
            await MainActor.run {
                self?.knowledgeVersion &+= 1
            }
        }
    }

    func runSearch(query: String, debounce: Duration = .milliseconds(140)) {
        searchQuery = query
        searchTask?.cancel()
        searchCancellation?.cancel()
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let handle = vaultHandle else {
            quickOpenHits = []
            searchResults = []
            return
        }
        let cancellation = SearchCancellation()
        searchCancellation = cancellation
        searchTask = Task {
            if debounce != .zero {
                try? await Task.sleep(for: debounce)
                guard !Task.isCancelled else { return }
            }
            let (quick, full) = await Task.detached(priority: .userInitiated) {
                let q = handle.quickOpen(query: trimmed, limit: 20)
                let f = handle.search(query: trimmed, maxMatchesPerNote: 5, cancellation: cancellation)
                return (q, f)
            }.value
            guard !Task.isCancelled else { return }
            quickOpenHits = quick
            searchResults = full
        }
    }

    func openFocus(ordinal: UInt32) {
        guard let doc = currentDocument, let path = currentPath, let root = vaultRoot else { return }
        let saved = try? core?.diagramFocus(vault: root, note: path, block: ordinal)
        activeFocus = IOSDiagramFocusSession(note: path, vault: root, ordinal: ordinal, document: doc, initial: saved)
    }

    func rememberDiagramCollapsed(ordinal: UInt32, collapsed: [String]) {
        guard let core, let root = vaultRoot, let path = currentPath else { return }
        Task.detached(priority: .utility) {
            try? core.rememberDiagram(vault: root, note: path, block: ordinal, collapsed: collapsed)
        }
    }

    func rememberDiagramFocus(ordinal: UInt32, scale: Double, offsetX: Double, offsetY: Double) {
        guard let core, let root = vaultRoot, let path = currentPath else { return }
        Task.detached(priority: .utility) {
            try? core.rememberDiagramFocus(vault: root, note: path, block: ordinal, scale: scale, offsetX: offsetX, offsetY: offsetY)
        }
    }

    // MARK: - Bookmarks

    private func saveBookmark(for url: URL) {
        guard let data = try? url.bookmarkData(options: .minimalBookmark, includingResourceValuesForKeys: nil, relativeTo: nil) else { return }
        var dict = (try? NSDictionary(contentsOf: bookmarksURL, error: ())) as? [String: Data] ?? [:]
        dict[url.path] = data
        try? (dict as NSDictionary).write(to: bookmarksURL)
    }

    private func removeBookmark(forPath path: String) {
        var dict = (try? NSDictionary(contentsOf: bookmarksURL, error: ())) as? [String: Data] ?? [:]
        if dict.removeValue(forKey: path) != nil {
            try? (dict as NSDictionary).write(to: bookmarksURL)
        }
    }

    private func restoreLastVaultIfAvailable() {
        guard let sessionJSON = try? core?.sessionJson(),
              let data = sessionJSON.data(using: .utf8),
              let parsed = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let recent = parsed["recent"] as? [[String: Any]],
              let firstPath = recent.first?["path"] as? String else {
            return
        }
        if let dict = (try? NSDictionary(contentsOf: bookmarksURL, error: ())) as? [String: Data],
           let bookmarkData = dict[firstPath] {
            var stale = false
            if let resolved = try? URL(resolvingBookmarkData: bookmarkData, options: [], relativeTo: nil, bookmarkDataIsStale: &stale) {
                if stale && resolved.path != firstPath {
                    removeBookmark(forPath: firstPath)
                }
                openVaultImmediate(url: resolved, securityScoped: true)
                return
            }
        }
        if FileManager.default.fileExists(atPath: firstPath) {
            openVaultImmediate(url: URL(fileURLWithPath: firstPath), securityScoped: false)
        }
    }

    #if DEBUG
    private func applyLaunchArgumentsIfNeeded() -> Bool {
        let args = ProcessInfo.processInfo.arguments
        func value(after flag: String) -> String? {
            guard let index = args.firstIndex(of: flag), index + 1 < args.count else { return nil }
            return args[index + 1]
        }
        if let theme = value(after: "--theme") {
            settings.applyOverrides(appearance: AppAppearance(rawValue: theme), fontSize: nil)
        }
        if let sizeText = value(after: "--font-size"), let size = Double(sizeText) {
            settings.applyOverrides(appearance: nil, fontSize: size)
        }
        if let panel = value(after: "--panel"), let tab = KnowledgeTab(rawValue: panel) {
            selectedTab = tab
            knowledgeSheetPresented = true
            preserveKnowledgeSheetOnOpen = true
        }
        if args.contains("--settings") {
            settingsPresented = true
        }
        let requestedNote = value(after: "--note")
        if requestedNote != nil {
            suppressAutoStartNote = true
        }
        if let focusText = value(after: "--focus"), let ordinal = UInt32(focusText) {
            pendingLaunchFocus = ordinal
        }
        if let reveal = value(after: "--reveal") {
            pendingLaunchReveal = reveal
        }
        var opened = false
        if args.contains("--reset-demo"),
           let docs = try? FileManager.default.url(for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true) {
            let demo = docs.appendingPathComponent("HortusDemo", isDirectory: true)
            try? FileManager.default.removeItem(at: demo)
        }
        if args.contains("--demo"), let demoTarget = prepareDemoFolder() {
            openVaultImmediate(url: demoTarget, securityScoped: false)
            opened = true
        } else if let vaultPath = value(after: "--vault"), FileManager.default.fileExists(atPath: vaultPath) {
            openVaultImmediate(url: URL(fileURLWithPath: vaultPath), securityScoped: false)
            opened = true
        }
        if opened, let note = requestedNote {
            let editing = value(after: "--mode") == "editing"
            openNote(note, editing: editing)
        }
        if opened, let query = value(after: "--query") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in
                self?.runSearch(query: query)
            }
        }
        return opened
    }
    #endif
}
