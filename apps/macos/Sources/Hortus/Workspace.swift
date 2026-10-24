import AppKit
import SwiftUI
import Darwin
import HortusCore

struct VaultState: Codable, Identifiable {
    var path: String
    var tabs: [String]
    var selected: String?
    var views: [String: String]?
    var id: String { path }
}

private struct SessionState: Codable {
    var last_vault: String?
    var recent: [VaultState]
}

struct NoteTab: Identifiable {
    var path: String
    var document: NoteDocument?
    @MainActor var text: String { document?.text ?? "" }
    var loading = true
    var error: String?
    var id: String { path }
    var title: String { URL(fileURLWithPath: path).lastPathComponent }
}

/// 未持久化文档的原生提示决策；测试注入回调代替真实弹窗。
enum UnsavedDecision {
    case retry
    case discard
    case cancel
}

/// 多份文档的关闭结果只在主线程使用；批量准备时由队列写入。
private final class UnsavedNotes: @unchecked Sendable {
    private let lock = NSLock()
    private var paths: [String] = []
    func append(_ path: String) {
        lock.lock(); defer { lock.unlock() }
        paths.append(path)
    }
    var values: [String] {
        lock.lock(); defer { lock.unlock() }
        return paths
    }
}

/// 每个库只有一个阻塞消费者；关闭后等待它真正退出，避免切库积累线程。
private final class VaultSubscription: Sendable {
    let handle: VaultHandle
    private let finished = DispatchGroup()

    init(handle: VaultHandle, receive: @escaping @Sendable ([TreeBatch]) -> Void) {
        self.handle = handle
        let finished = finished
        finished.enter()
        let thread = Thread {
            defer { finished.leave() }
            while true {
                let batches = handle.waitForBatches()
                guard !batches.isEmpty else { return }
                receive(batches)
            }
        }
        thread.name = "hortus-batches"
        thread.start()
    }

    func close() {
        handle.close()
        finished.wait()
    }
}

/// 主线程仅维护界面状态；文件、配置和绑定初始化在后台执行。
@Observable @MainActor
final class Workspace {
    @ObservationIgnored let diagrams = DiagramService()
    @ObservationIgnored var shutdownDelayForInspection = 0.0
    /// 测试注入的未保存提示；生产环境使用原生 NSAlert。
    @ObservationIgnored var promptUnsaved: (([String], Bool) -> UnsavedDecision)?
    @ObservationIgnored var noteOpenStartedAt = 0.0
    @ObservationIgnored var noteOpenPhases: [String: Double] = [:]
    var diagramCore: WorkspaceCore? { core }
    var root: String?
    var entries: [String: TreeEntry] = [:]
    var treeVersion = 0
    var tabs: [NoteTab] = []
    var selected: String?
    var recent: [VaultState] = []
    var scanning = false
    var opening = false
    var error: String?
    var terminal = TerminalSession()
    var fontSize = 15.0 {
        didSet { terminal.update(fontSize: fontSize) }
    }
    var defaultView = "reading"
    var scanMicros: UInt64 = 0
    var searchIndexReadyMilliseconds: Double?
    var searchIndexPeakRSSDeltaBytes: Int64?
    var firstDrawMilliseconds: Double?
    var scanDisplayMilliseconds: Double?
    var quickOpenVisible = false
    var quickOpenQuery = ""
    var quickOpenResults: [QuickOpenHit] = []
    var quickOpenSelection = 0
    var searchQuery = ""
    var searchResults: [SearchResult] = []
    var searchLoading = false
    @ObservationIgnored private var core: WorkspaceCore?
    @ObservationIgnored private var subscription: VaultSubscription?
    @ObservationIgnored private var layoutTask: Task<Void, Never>?
    @ObservationIgnored private var sessionTask: Task<Void, Never>?
    @ObservationIgnored private var epoch = UUID()
    @ObservationIgnored private var openingRequest = UUID()
    @ObservationIgnored private var loads: [String: UUID] = [:]
    @ObservationIgnored private var started = false
    @ObservationIgnored private var openedAt = ContinuousClock.now
    @ObservationIgnored private var searchIndexBaselineRSS: Int64 = 0
    @ObservationIgnored private var layout: Layout?
    @ObservationIgnored private let saves = DispatchQueue(label: "app.hortus.state")
    @ObservationIgnored private var quickOpenTask: Task<Void, Never>?
    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var searchCancellation: SearchCancellation?
    @ObservationIgnored private var quickOpenMarked = false
    @ObservationIgnored private var searchMarked = false
    @ObservationIgnored private var pendingSearchReveals: [String: (start: Int, length: Int)] = [:]

    var current: NoteTab? { tabs.first { $0.path == selected } }
    var title: String { current?.title ?? root.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "Hortus" }
    private var vault: VaultHandle? { subscription?.handle }

    func start(layout: Layout) async {
        guard !started else { return }
        started = true
        self.layout = layout
        do {
            let service = try await Task.detached { try WorkspaceCore() }.value
            core = service
            let (layoutJSON, sessionJSON, font) = try await Task.detached {
                (try service.layoutJson(), try service.sessionJson(), try service.fontSize())
            }.value
            if let layoutJSON {
                layout.restore(try JSONDecoder().decode(LayoutState.self, from: Data(layoutJSON.utf8)))
            }
            fontSize = font
            defaultView = try await Task.detached { try service.defaultView() }.value
            let session = try JSONDecoder().decode(SessionState.self, from: Data(sessionJSON.utf8))
            recent = session.recent
            layout.onChange = { [weak self] in self?.scheduleLayoutSave() }
            let args = ProcessInfo.processInfo.arguments
            if let arg = args.first(where: { $0.hasPrefix("--vault=") }) {
                await openFolder(String(arg.dropFirst("--vault=".count)))
            } else if !args.contains("--empty"), let path = session.last_vault {
                await openFolder(path)
            }
        } catch { self.error = L10n.error(error) }
    }

    func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = L10n.string("Open folder")
        panel.begin { [weak self] response in
            guard response == .OK, let path = panel.url?.path else { return }
            Task { await self?.openFolder(path) }
        }
    }

    func openFolder(_ path: String) async {
        guard let core else { return }
        // 切换知识库前先把当前库的文档持久化；任一失败就保持当前界面。
        if root != nil, await prepareDocumentsForClose() == .cancelled { return }
        let request = UUID()
        openingRequest = request
        opening = true
        let requestStarted = ContinuousClock.now
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        searchIndexBaselineRSS = Int64(usage.ru_maxrss)
        searchIndexReadyMilliseconds = nil
        searchIndexPeakRSSDeltaBytes = nil
        defer { if openingRequest == request { opening = false } }
        do {
            let handle = try await Task.detached { try core.openVault(path: path) }.value
            guard openingRequest == request else { await Task.detached { handle.close() }.value; return }
            await persistSession()
            guard openingRequest == request else { await Task.detached { handle.close() }.value; return }
            epoch = request
            // 旧文档已经确认持久化；进入新库后不再接收它们的自动保存。
            for tab in tabs { tab.document?.stopForShutdown() }
            await terminal.setRootAndWait(nil)
            // 切换状态期间不挂起主线程任务，避免后来的打开请求留下已关闭的旧库。
            if let old = subscription { Task.detached { old.close() } }
            subscription = VaultSubscription(handle: handle) { [weak self] batches in
                // 先把树交给主线程显示，再在这条后台线程读正文更新搜索；读文件不能挡住文件树
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.epoch == request else { return }
                    self.apply(batches)
                }
                let upserted = batches.flatMap { batch in
                    batch.upserted.filter { !$0.directory }.map(\.path)
                }
                let removed = batches.flatMap(\.removed)
                handle.updateSearchIndex(upserted: upserted, removed: removed)
                let changed = !upserted.isEmpty || !removed.isEmpty
                let scanFinished = batches.contains(where: \.scanComplete)
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.epoch == request else { return }
                    self.searchIndexUpdated(changed: changed, scanFinished: scanFinished)
                }
            }
            root = handle.root()
            await terminal.setRootAndWait(root)
            entries = [:]
            treeVersion += 1
            tabs = []
            selected = nil
            scanning = true
            scanMicros = 0
            firstDrawMilliseconds = nil
            scanDisplayMilliseconds = nil
            openedAt = requestStarted
            loads = [:]
            quickOpenTask?.cancel()
            searchTask?.cancel()
            searchCancellation?.cancel()
            quickOpenResults = []
            searchResults = []
            searchQuery = ""
            searchLoading = false
            searchCancellation?.cancel()
            if let saved = recent.first(where: { $0.path == root }) {
                for note in saved.tabs { openNote(note) }
                if let selection = saved.selected, tabs.contains(where: { $0.path == selection }) { selected = selection }
            }
            scheduleSessionSave()
        } catch { if openingRequest == request { self.error = L10n.error(error) } }
    }

    func closeFolder() async {
        // 任一文档没有持久化就中止关闭，界面保持原状。
        guard await prepareDocumentsForClose() == .ready else { return }
        for tab in tabs { tab.document?.stopForShutdown() }
        await terminal.setRootAndWait(nil)
        let request = UUID()
        openingRequest = request
        epoch = request
        sessionTask?.cancel()
        let old = subscription
        subscription = nil
        await persistSession()
        guard openingRequest == request else {
            await Task.detached { old?.close() }.value
            return
        }
        root = nil
        entries = [:]
        treeVersion += 1
        tabs = []
        selected = nil
        loads = [:]
        quickOpenTask?.cancel()
        searchTask?.cancel()
        searchCancellation?.cancel()
        quickOpenResults = []
        searchResults = []
        searchLoading = false
        quickOpenVisible = false
        scanning = false
        opening = false
        await Task.detached { old?.close() }.value
    }

    private func apply(_ batches: [TreeBatch]) {
        var refresh = Set<String>()
        for batch in batches {
            for path in batch.removed {
                entries.removeValue(forKey: path)
                if tabs.contains(where: { $0.path == path }) { refresh.insert(path) }
            }
            for item in batch.upserted {
                entries[item.path] = item
                if tabs.contains(where: { $0.path == item.path }) { refresh.insert(item.path) }
            }
            if batch.scanComplete {
                scanning = false
                scanMicros = batch.scanMicros
            }
            if let issue = batch.errors.first { error = L10n.errorKey(issue) }
        }
        treeVersion += 1
        for path in refresh {
            if let document = tabs.first(where: { $0.path == path })?.document { document.refresh() }
            else { load(path) }
        }
    }

    /// 后台线程更新完搜索内容后回到主线程：记录索引就绪，刷新正在显示的搜索结果
    private func searchIndexUpdated(changed: Bool, scanFinished: Bool) {
        if scanFinished {
            let elapsed = openedAt.duration(to: .now).components
            searchIndexReadyMilliseconds = Double(elapsed.seconds) * 1000
                + Double(elapsed.attoseconds) / 1_000_000_000_000_000
            var usage = rusage()
            getrusage(RUSAGE_SELF, &usage)
            searchIndexPeakRSSDeltaBytes = max(0, Int64(usage.ru_maxrss) - searchIndexBaselineRSS)
        }
        guard changed || scanFinished, !scanning else { return }
        if quickOpenVisible, !quickOpenMarked { updateQuickOpen(query: quickOpenQuery, marked: false) }
        if !searchMarked, !searchQuery.isEmpty { updateSearch(query: searchQuery, marked: false) }
    }

    func openNote(_ path: String) {
        guard vault != nil else { return }
        if selected != path || current?.document == nil {
            noteOpenStartedAt = ProcessInfo.processInfo.systemUptime
            noteOpenPhases = [:]
        }
        if selected != path, let previous = current?.document { Task { await previous.saveNow() } }
        if !tabs.contains(where: { $0.path == path }) {
            tabs.append(NoteTab(path: path))
            load(path)
        }
        selected = path
        scheduleSessionSave()
    }

    func presentQuickOpen() {
        guard vault != nil else { return }
        quickOpenVisible = true
        quickOpenQuery = ""
        quickOpenSelection = 0
        updateQuickOpen(query: "", marked: false)
    }

    func dismissQuickOpen() {
        quickOpenVisible = false
        quickOpenTask?.cancel()
    }

    func moveQuickOpenSelection(_ delta: Int) {
        guard !quickOpenResults.isEmpty else { return }
        quickOpenSelection = (quickOpenSelection + delta + quickOpenResults.count) % quickOpenResults.count
    }

    func activateQuickOpenSelection() {
        guard quickOpenResults.indices.contains(quickOpenSelection) else { return }
        let path = quickOpenResults[quickOpenSelection].path
        dismissQuickOpen()
        openNote(path)
    }

    func updateQuickOpen(query: String, marked: Bool) {
        quickOpenQuery = query
        quickOpenMarked = marked
        quickOpenTask?.cancel()
        guard !marked, let handle = vault else { return }
        quickOpenTask = Task { [weak self] in
            let results = await Task.detached {
                handle.quickOpen(query: query, limit: 50)
            }.value
            guard let self, !Task.isCancelled, self.quickOpenVisible else { return }
            self.quickOpenResults = results
            self.quickOpenSelection = min(self.quickOpenSelection, max(0, results.count - 1))
        }
    }

    func showSearchPanel() {
        guard root != nil else { return }
        layout?.moveToRegion(.search, .left)
        searchTask?.cancel()
        searchCancellation?.cancel()
        searchQuery = ""
        searchResults = []
        searchLoading = false
        searchMarked = false
    }

    func updateSearch(query: String, marked: Bool) {
        searchQuery = query
        searchMarked = marked
        searchTask?.cancel()
        searchCancellation?.cancel()
        guard !marked, let handle = vault else {
            searchLoading = false
            return
        }
        guard !query.isEmpty else {
            searchResults = []
            searchLoading = false
            return
        }
        let cancellation = SearchCancellation()
        searchCancellation = cancellation
        searchLoading = true
        searchTask = Task { [weak self] in
            let results = await Task.detached {
                handle.search(query: query, maxMatchesPerNote: 5, cancellation: cancellation)
            }.value
            guard let self, !Task.isCancelled, self.searchQuery == query else { return }
            self.searchResults = results
            self.searchLoading = false
        }
    }

    func waitForSearchTasks() async {
        await quickOpenTask?.value
        await searchTask?.value
    }

    func openSearchResult(path: String, start: Int, length: Int) {
        pendingSearchReveals[path] = (start, length)
        if selected == path, let document = current?.document {
            revealSearch(document: document, start: start, length: length)
        } else {
            openNote(path)
            if let document = tabs.first(where: { $0.path == path })?.document {
                Task { @MainActor in
                    await Task.yield()
                    self.revealSearch(document: document, start: start, length: length)
                }
            }
        }
    }

    func revealHeading(_ heading: DocumentHeading) {
        guard let document = current?.document, document.outlineRevision == document.revision else { return }
        revealSearch(document: document, start: Int(heading.start), length: 0)
    }

    private func revealSearch(document: NoteDocument, start: Int, length: Int) {
        if let path = tabs.first(where: { $0.document === document })?.path {
            pendingSearchReveals.removeValue(forKey: path)
        }
        document.sourceAnchor = start
        document.sourceSelectionLength = length
        guard let view = NSApp.windows.compactMap(\.contentView).flatMap({ DiagramInspection.textViews(in: $0) })
            .first(where: { $0.representedDocument == ObjectIdentifier(document) }) else { return }
        if document.reading {
            view.readingCoordinator?.reveal(source: start)
        } else {
            view.revealCharacter(at: start)
            view.setSelectedRange(NSRange(location: start, length: min(length, max(0, view.sourceUTF16Length - start))))
        }
    }

    /// 自检用：全文搜索词由检查脚本传入，界面代码里不写死文字
    func searchPerformanceSamples(fullTextQuery: String) async -> (quickOpen: [Double], fullText: [Double])? {
        guard vault != nil, searchIndexReadyMilliseconds != nil else { return nil }
        var quickOpen: [Double] = []
        quickOpenVisible = true
        for _ in 0..<20 {
            for query in ["n", "no", "not", "note"] {
                let start = DispatchTime.now().uptimeNanoseconds
                updateQuickOpen(query: query, marked: false)
                await quickOpenTask?.value
                quickOpen.append(Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000)
            }
        }
        quickOpenVisible = false
        var fullText: [Double] = []
        for _ in 0..<20 {
            let start = DispatchTime.now().uptimeNanoseconds
            updateSearch(query: fullTextQuery, marked: false)
            await searchTask?.value
            fullText.append(Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000)
        }
        return (quickOpen, fullText)
    }

    private func load(_ path: String) {
        guard let core, let root else { return }
        let request = epoch
        let revision = UUID()
        let queuedAt = ProcessInfo.processInfo.systemUptime
        loads[path] = revision
        Task { [weak self] in
            let result = await Task.detached { Result {
                let openStarted = ProcessInfo.processInfo.systemUptime
                let session = try core.openDocument(vault: root, path: path)
                let openFinished = ProcessInfo.processInfo.systemUptime
                let initial = try session.initial()
                let initialFinished = ProcessInfo.processInfo.systemUptime
                return (DocumentIO(session: session, url: URL(fileURLWithPath: root).appendingPathComponent(path)), initial,
                        openFinished - openStarted, initialFinished - openFinished, initialFinished)
            } }.value
            guard let self, self.epoch == request, self.loads[path] == revision, let index = self.tabs.firstIndex(where: { $0.path == path }) else { return }
            self.tabs[index].loading = false
            switch result {
            case .success(let loaded):
                let document = NoteDocument(io: loaded.0, initial: loaded.1)
                if let reveal = pendingSearchReveals.removeValue(forKey: path) {
                    document.sourceAnchor = reveal.start
                    document.sourceSelectionLength = reveal.length
                }
                document.reading = (recent.first(where: { $0.path == root })?.views?[path] ?? defaultView) != "editing"
                document.onCopy = { [weak self] copy in self?.openNote(copy) }
                self.tabs[index].document = document; self.tabs[index].error = nil
                self.noteOpenPhases = ["load_task_wait_ms": max(0, loaded.4 - queuedAt - loaded.2 - loaded.3) * 1000,
                                       "open_document_ms": loaded.2 * 1000,
                                       "initial_snapshot_ms": loaded.3 * 1000,
                                       "main_actor_handoff_ms": max(0, ProcessInfo.processInfo.systemUptime - loaded.4) * 1000]
            case .failure(let error): self.tabs[index].error = L10n.error(error)
            }
        }
    }

    func closeTab(_ path: String? = nil) {
        guard let target = path ?? selected, let index = tabs.firstIndex(where: { $0.path == target }) else { return }
        guard let document = tabs[index].document else { removeTab(target); return }
        closeTabWhenPersisted(target, document, epoch: epoch)
    }

    /// 只有最新正文至少落盘一处后才移除标签；失败时保留标签和可编辑状态。
    private func closeTabWhenPersisted(_ target: String, _ document: NoteDocument, epoch: UUID) {
        Task { [weak self] in
            let persistence = await document.prepareToClose()
            guard let self, self.epoch == epoch,
                  let index = self.tabs.firstIndex(where: { $0.path == target && $0.document === document })
            else { return }
            if persistence != .unsaved {
                self.tabs[index].document?.stopForShutdown()
                self.removeTab(target)
                return
            }
            switch self.decideUnsaved([target], quitting: false) {
            case .retry: self.closeTabWhenPersisted(target, document, epoch: epoch)
            case .discard: document.abandon(); self.removeTab(target)
            case .cancel: break
            }
        }
    }

    /// 批量准备关闭或切换知识库；全部文档都持久化后才返回 `.ready`。
    private enum DocumentsCloseResult: Equatable { case ready, cancelled }
    private func prepareDocumentsForClose() async -> DocumentsCloseResult {
        let requestEpoch = epoch
        while true {
            var unsaved: [(String, NoteDocument)] = []
            for tab in tabs {
                guard let document = tab.document else { continue }
                let persistence = await document.prepareToClose()
                guard self.epoch == requestEpoch,
                      tabs.contains(where: { $0.path == tab.path && $0.document === document })
                else { return .cancelled }
                if persistence == .unsaved { unsaved.append((tab.path, document)) }
            }
            if unsaved.isEmpty { return .ready }
            switch decideUnsaved(unsaved.map(\.0), quitting: false) {
            case .retry: continue
            case .discard:
                for (_, document) in unsaved { document.abandon() }
                return .ready
            case .cancel: return .cancelled
            }
        }
    }

    /// 未保存文档的原生提示。测试通过 promptUnsaved 注入决策，避免弹窗阻塞。
    func decideUnsaved(_ notes: [String], quitting: Bool) -> UnsavedDecision {
        if let promptUnsaved { return promptUnsaved(notes, quitting) }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = L10n.string("Some notes could not be saved")
        alert.informativeText = L10n.string("These notes could not be written to the note file or to a recovery draft. Retry after fixing the problem, or discard their changes.")
            + "\n\n" + Self.noteList(notes)
        alert.addButton(withTitle: L10n.string("Retry Save"))
        alert.addButton(withTitle: L10n.string(quitting ? "Discard Changes and Quit" : "Discard Changes and Close"))
        alert.buttons[1].hasDestructiveAction = true
        alert.addButton(withTitle: L10n.string("Cancel"))
        switch alert.runModal() {
        case .alertFirstButtonReturn: return .retry
        case .alertSecondButtonReturn: return .discard
        default: return .cancel
        }
    }
    private static func noteList(_ notes: [String]) -> String {
        let limit = 5
        var lines = notes.prefix(limit).map { "• \($0)" }
        if notes.count > limit { lines.append(L10n.string("…and %@ more", notes.count - limit)) }
        return lines.joined(separator: "\n")
    }

    private func removeTab(_ target: String) {
        guard let index = tabs.firstIndex(where: { $0.path == target }) else { return }
        tabs.remove(at: index)
        if selected == target { selected = tabs.isEmpty ? nil : tabs[min(index, tabs.count - 1)].path }
        scheduleSessionSave()
    }

    func toggleView() { Task { await toggleCurrentView() } }
    func toggleCurrentView() async {
        guard let document = current?.document, !document.marked else { return }
        if document.reading {
            if let view = DiagramInspection.current(), let coordinator = view.readingCoordinator {
                let position = view.selectedRange().length > 0 ? view.selectedRange().location
                    : view.characterIndexForInsertion(at: CGPoint(x: 31, y: view.visibleRect.minY + 1))
                document.sourceAnchor = coordinator.sourcePosition(at: position)
            }
        } else {
            document.sourceAnchor = DiagramInspection.current()?.selectedRange().location ?? 0
            await document.saveNow()
            guard !document.dirty, document.issue != .saveFailed, document.issue != .conflict else { return }
        }
        document.reading.toggle(); scheduleSessionSave()
    }
    func saveCurrent() { if let document = current?.document { Task { await document.saveNow() } } }
    func saveAllDocuments() async {
        for document in tabs.compactMap(\.document) { await document.saveNow() }
    }

    func rememberDiagram(note: String, block: UInt32, collapsed: [String]) {
        guard let core, let root else { return }
        saves.async {
            do { try core.rememberDiagram(vault: root, note: note, block: block, collapsed: collapsed) }
            catch { DispatchQueue.main.async { self.error = L10n.error(error) } }
        }
    }

    func scheduleLayoutSave() {
        layoutTask?.cancel()
        guard let core, let state = layout?.state else { return }
        layoutTask = Task { [weak self] in
            do {
                try await Task.sleep(for: .milliseconds(250))
                let json = String(decoding: try JSONEncoder().encode(state), as: UTF8.self)
                guard let self else { return }
                try await self.save { try core.saveLayout(json: json) }
            } catch is CancellationError {} catch { self?.error = L10n.error(error) }
        }
    }

    func scheduleSessionSave() {
        sessionTask?.cancel()
        sessionTask = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(250)) } catch { return }
            await self?.persistSession()
        }
    }

    private func persistSession() async {
        guard let core, let root else { return }
        let paths = tabs.map(\.path)
        let selection = selected
        let views = sessionViews()
        do {
            let json = try await save {
                try core.rememberSession(path: root, tabs: paths, selected: selection, views: views)
                return try core.sessionJson()
            }
            recent = try JSONDecoder().decode(SessionState.self, from: Data(json.utf8)).recent
        } catch { self.error = L10n.error(error) }
    }

    private func sessionViews() -> [String: String] {
        Dictionary(uniqueKeysWithValues: tabs.compactMap { tab in
            tab.document.map { (tab.path, $0.reading ? "reading" : "editing") }
        })
    }

    /// 同一队列串行写入，退出保存也排在所有已提交的后台保存之后。
    private func save<T: Sendable>(_ operation: @escaping @Sendable () throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            saves.async { continuation.resume(with: Result(catching: operation)) }
        }
    }

    /// 退出第一阶段：等待所有文档把最新正文写到笔记或恢复稿。
    /// 只报告未持久化的路径，不停止文档；取消退出后界面仍可继续工作。
    func prepareShutdown(completion: @escaping @Sendable ([String]) -> Void) {
        let documents = tabs.compactMap { tab in tab.document.map { (path: tab.path, io: $0.io, noteWrite: !$0.marked) } }
        guard !documents.isEmpty else { completion([]); return }
        Self.settleDocuments(documents, completion: completion)
    }

    /// 在非 MainActor 上下文收集结果：块本身带隔离信息时，进入保存队列就会触发运行时检查。
    nonisolated private static func settleDocuments(
        _ documents: [(path: String, io: DocumentIO, noteWrite: Bool)],
        completion: @escaping @Sendable ([String]) -> Void
    ) {
        let unsaved = UnsavedNotes()
        let group = DispatchGroup()
        for document in documents {
            group.enter()
            document.io.queue.async {
                if document.io.prepareForCloseOnQueue(noteWrite: document.noteWrite).persistence == .unsaved {
                    unsaved.append(document.path)
                }
                group.leave()
            }
        }
        // terminateLater 的嵌套循环不调度 MainActor 任务；RunLoop 源仍会执行。
        group.notify(queue: .global(qos: .userInitiated)) {
            let paths = unsaved.values
            RunLoop.main.perform(inModes: [.common]) {
                MainActor.assumeIsolated { completion(paths) }
            }
        }
    }

    /// 退出第二阶段：文档已确认持久化后才做不可逆清理。
    /// 布局与会话用与常规保存相同的完整入口一次写入。
    func beginShutdownCleanup(completion: @escaping @Sendable () -> Void) {
        openingRequest = UUID()
        epoch = UUID()
        layoutTask?.cancel(); sessionTask?.cancel()
        let state = layout?.state
        let core = core
        let root = root
        let paths = tabs.map(\.path)
        let selection = selected
        let views = sessionViews()
        let old = subscription
        subscription = nil
        let delay = shutdownDelayForInspection
        for document in tabs.compactMap(\.document) { document.stopForShutdown() }
        let group = DispatchGroup()
        group.enter()
        terminal.stopForShutdown { group.leave() }
        Self.finishShutdown(group: group, queue: saves, core: core, state: state, root: root,
                            paths: paths, selection: selection, views: views, old: old,
                            delay: delay, completion: completion)
    }

    nonisolated private static func finishShutdown(
        group: DispatchGroup, queue: DispatchQueue, core: WorkspaceCore?, state: LayoutState?,
        root: String?, paths: [String], selection: String?, views: [String: String],
        old: VaultSubscription?, delay: TimeInterval,
        completion: @escaping @Sendable () -> Void
    ) {
        queue.async {
            group.wait()
            if delay > 0 { Thread.sleep(forTimeInterval: delay) }
            do {
                if let core, let state { try core.saveLayout(json: String(decoding: JSONEncoder().encode(state), as: UTF8.self)) }
                if let core, let root { try core.rememberSession(path: root, tabs: paths, selected: selection, views: views) }
            } catch { fputs("Hortus session save failed: \(error)\n", stderr) }
            old?.close()
            RunLoop.main.perform(inModes: [.common]) {
                MainActor.assumeIsolated { completion() }
            }
        }
    }

    /// 用户明确放弃退出：清理这些文档的恢复稿后再让进程结束。
    func abandonDocuments(_ paths: [String], completion: @escaping @MainActor @Sendable () -> Void) {
        let documents = tabs.filter { paths.contains($0.path) }.compactMap(\.document)
        guard !documents.isEmpty else { completion(); return }
        let group = DispatchGroup()
        for document in documents {
            document.stopForShutdown()
            let session = document.io.session
            group.enter()
            document.io.queue.async {
                _ = try? session.recover(restore: false)
                group.leave()
            }
        }
        group.notify(queue: .main) { MainActor.assumeIsolated { completion() } }
    }

    func treeDidDraw() {
        guard !entries.isEmpty else { return }
        let elapsed = openedAt.duration(to: .now).components
        let milliseconds = Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15
        if firstDrawMilliseconds == nil { firstDrawMilliseconds = milliseconds }
        if !scanning, scanDisplayMilliseconds == nil { scanDisplayMilliseconds = milliseconds }
    }
}
