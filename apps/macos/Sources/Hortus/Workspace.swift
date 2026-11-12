import AppKit
import SwiftUI
import Darwin
import HortusCore

struct VaultState: Codable, Identifiable, Sendable {
    var path: String
    var tabs: [String]
    var selected: String?
    var views: [String: String]?
    var visible: [String: Bool]?
    var id: String { path }
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
    let id = UUID()
    weak var application: ApplicationWorkspace?
    var context: VaultContext?
    var root: String? { context?.root }
    /// 自检读取当前标签的 shell PID；普通代码走标签控制器。
    var terminalPid: pid_t { terminals.currentSession?.inspectionShellPid ?? 0 }
    private var fixtureEntries: [String: TreeEntry] = [:]
    var entries: [String: TreeEntry] {
        get { context?.entries ?? fixtureEntries }
        set { fixtureEntries = newValue }
    }
    var treeVersion: Int { context?.treeVersion ?? 0 }
    var tabs: [NoteTab] = []
    var selected: String?
    var recent: [VaultState] { application?.recent ?? [] }
    var scanning: Bool { context?.scanning ?? false }
    var opening = false
    var error: String?
    let terminals = TerminalController()
    var fontSize = 15.0 {
        didSet { terminals.update(fontSize: fontSize) }
    }
    var defaultView = "reading"
    var scanMicros: UInt64 { context?.scanMicros ?? 0 }
    var searchIndexReadyMilliseconds: Double? { context?.searchIndexReadyMilliseconds }
    var searchIndexPeakRSSDeltaBytes: Int64? { context?.searchIndexPeakRSSDeltaBytes }
    var restoredViews: [String: String] = [:]
    var firstDrawMilliseconds: Double?
    var scanDisplayMilliseconds: Double?
    var quickOpenVisible = false
    var quickOpenQuery = ""
    var quickOpenResults: [QuickOpenHit] = []
    var quickOpenSelection = 0
    var searchQuery = ""
    var searchFocusRequest = 0
    var searchResults: [SearchResult] = []
    var searchLoading = false
    @ObservationIgnored var core: WorkspaceCore?
    @ObservationIgnored private var epoch = UUID()
    @ObservationIgnored private var openingRequest = UUID()
    @ObservationIgnored private var loads: [String: UUID] = [:]
    @ObservationIgnored private var loadTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var releasing = false
    @ObservationIgnored private var openedAt = ContinuousClock.now
    @ObservationIgnored private var layout: Layout?
    @ObservationIgnored private let saves = DispatchQueue(label: "app.hortus.state")
    @ObservationIgnored private var quickOpenTask: Task<Void, Never>?
    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var searchCancellation: SearchCancellation?
    @ObservationIgnored private var quickOpenMarked = false
    @ObservationIgnored private var searchMarked = false
    @ObservationIgnored private var pendingSearchReveals: [String: (start: Int, length: Int)] = [:]

    init() {
        terminals.workspace = self
    }

    var current: NoteTab? { tabs.first { $0.path == selected } }
    var title: String { current?.title ?? root.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "Hortus" }
    private var vault: VaultHandle? { context?.handle }

    func configure(application: ApplicationWorkspace, core: WorkspaceCore, layout: Layout) {
        self.application = application
        self.core = core
        self.layout = layout
        fontSize = application.fontSize
        defaultView = application.defaultView
    }

    func attach(_ context: VaultContext, state: VaultState?) async {
        self.context = context
        openedAt = .now
        restoredViews = state?.views ?? [:]
        context.attach(self)
        await terminals.setRootAndWait(context.root)
        for path in state?.tabs ?? [] {
            if application?.owner(of: path, from: self) != nil {
                application?.recordDuplicate(path, window: self)
            } else { openNote(path) }
        }
        if let selected = state?.selected, tabs.contains(where: { $0.path == selected }) { self.selected = selected }
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

    func openFolder(_ path: String) async { await application?.openFolder(path, source: self) }

    func closeFolder() async {
        guard await prepareDocumentsForClose() == .ready else { return }
        await application?.detach(self)
    }

    /// 非最后窗口关闭时先持久化，再释放本窗口文档和 PTY。
    func releaseDocuments() async {
        releasing = true
        defer { releasing = false }
        application?.cancelTabDrags(in: self)
        // 加载占位在打开完成前保留；库关闭与再次打开也不能重叠创建会话。
        for task in loadTasks.values { await task.value }
        epoch = UUID()
        openingRequest = UUID()
        for tab in tabs { tab.document?.stopForShutdown() }
        await terminals.setRootAndWait(nil)
        quickOpenTask?.cancel(); searchTask?.cancel(); searchCancellation?.cancel()
        context?.detach(self)
        context = nil
        tabs = []; selected = nil; loads = [:]
        closingTabs = []; pendingSearchReveals = [:]; restoredViews = [:]
        quickOpenVisible = false; quickOpenResults = []; searchResults = []; searchLoading = false
    }

    func applyVaultEvents(_ batches: [TreeBatch]) {
        var refresh = Set<String>()
        for batch in batches {
            for path in batch.removed where tabs.contains(where: { $0.path == path }) { refresh.insert(path) }
            for item in batch.upserted where tabs.contains(where: { $0.path == item.path }) { refresh.insert(item.path) }
            if let issue = batch.errors.first { error = L10n.errorKey(issue) }
        }
        for path in refresh {
            if let document = tabs.first(where: { $0.path == path })?.document { document.refresh() }
            else { load(path) }
        }
    }

    /// 后台线程更新完搜索内容后回到主线程：记录索引就绪，刷新正在显示的搜索结果
    func searchIndexUpdated(changed: Bool, scanFinished: Bool) {
        guard changed || scanFinished, !scanning else { return }
        if quickOpenVisible, !quickOpenMarked { updateQuickOpen(query: quickOpenQuery, marked: false) }
        if !searchMarked, !searchQuery.isEmpty { updateSearch(query: searchQuery, marked: false) }
    }

    func openNote(_ path: String) {
        guard vault != nil, !releasing, !preparingClose else { return }
        if let owner = application?.owner(of: path, from: self), owner.workspace !== self {
            owner.workspace.openNote(path)
            application?.activate(owner)
            return
        }
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
        searchFocusRequest += 1
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
        if let owner = application?.owner(of: path, from: self), owner.workspace !== self {
            owner.workspace.openSearchResult(path: path, start: start, length: length)
            application?.activate(owner)
            return
        }
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

    func revealHeading(_ heading: DocumentHeading, path: String? = nil) {
        let path = path ?? current?.path
        guard let path else { return }
        let owner = application?.owner(of: path, from: self)?.workspace ?? self
        guard let document = owner.tabs.first(where: { $0.path == path })?.document,
              document.outlineRevision == document.revision else { return }
        openSearchResult(path: path, start: Int(heading.start), length: 0)
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
        guard let core, let root, loads[path] == nil, !releasing else { return }
        let request = epoch
        let revision = UUID()
        let queuedAt = ProcessInfo.processInfo.systemUptime
        loads[path] = revision
        loadTasks[path] = Task { [weak self] in
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
            self.loads[path] = nil; self.loadTasks[path] = nil
            self.tabs[index].loading = false
            switch result {
            case .success(let loaded):
                let reading = (restoredViews[path] ?? recent.first(where: { $0.path == root })?.views?[path] ?? defaultView) != "editing"
                let document = NoteDocument(io: loaded.0, initial: loaded.1, prepareSourceStyles: !reading)
                if let reveal = pendingSearchReveals.removeValue(forKey: path) {
                    document.sourceAnchor = reveal.start
                    document.sourceSelectionLength = reveal.length
                }
                document.reading = reading
                self.bind(document)
                self.tabs[index].document = document; self.tabs[index].error = nil
                self.scheduleSessionSave()
                self.noteOpenPhases = ["load_task_wait_ms": max(0, loaded.4 - queuedAt - loaded.2 - loaded.3) * 1000,
                                       "open_document_ms": loaded.2 * 1000,
                                       "initial_snapshot_ms": loaded.3 * 1000,
                                       "main_actor_handoff_ms": max(0, ProcessInfo.processInfo.systemUptime - loaded.4) * 1000]
            case .failure(let error): self.tabs[index].error = L10n.error(error)
            }
            if self.closingTabs.contains(path) {
                if let document = self.tabs.first(where: { $0.path == path })?.document {
                    self.closeTabWhenPersisted(path, document, epoch: request)
                } else { self.removeTab(path) }
            }
        }
    }

    func closeTab(_ path: String? = nil) {
        guard let target = path ?? selected, let index = tabs.firstIndex(where: { $0.path == target }) else { return }
        application?.cancelTabDrags(in: self, path: target)
        if tabs[index].loading { closingTabs.insert(target); return }
        guard let document = tabs[index].document else { removeTab(target); return }
        guard !closingTabs.contains(target) else { return }
        closingTabs.insert(target)
        closeTabWhenPersisted(target, document, epoch: epoch)
    }

    /// 只有最新正文至少落盘一处后才移除标签；失败时保留标签和可编辑状态。
    private func closeTabWhenPersisted(_ target: String, _ document: NoteDocument, epoch: UUID) {
        Task { [weak self] in
            guard let self, self.epoch == epoch,
                  self.tabs.contains(where: { $0.path == target && $0.document === document }) else { return }
            let persistence = await document.prepareToClose()
            guard self.epoch == epoch,
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
            case .cancel: self.closingTabs.remove(target)
            }
        }
    }

    /// 批量准备关闭或切换知识库；全部文档都持久化后才返回 `.ready`。
    enum DocumentsCloseResult: Equatable { case ready, cancelled }
    func prepareDocumentsForClose() async -> DocumentsCloseResult {
        guard !preparingClose else { return .cancelled }
        preparingClose = true
        defer { preparingClose = false }
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
        closingTabs.remove(target)
        if selected == target { selected = tabs.isEmpty ? nil : tabs[min(index, tabs.count - 1)].path }
        scheduleSessionSave()
    }

    @ObservationIgnored var closingTabs: Set<String> = []
    @ObservationIgnored var preparingClose = false
    @ObservationIgnored private var toggleTask: Task<Void, Never>?

    func bind(_ document: NoteDocument) {
        document.onCopy = { [weak self] copy in self?.openNote(copy) }
    }

    /// 转移不停止文档队列，也不按路径重新载入。原生编辑表面携带正文与撤销栈。
    func takeTab(_ path: String) -> NoteTab? {
        guard let tab = tabs.first(where: { $0.path == path }), !tab.loading,
              let document = tab.document, !document.marked, !closingTabs.contains(path), !preparingClose else { return nil }
        if let coordinator = document.viewOwner as? NoteText.EditorCoordinator {
            document.transferredEditor = coordinator.host?.take()
        } else if let coordinator = document.viewOwner as? ReadingText.Coordinator {
            coordinator.detach()
        }
        pendingSearchReveals.removeValue(forKey: path)
        removeTab(path)
        return tab
    }

    var textView: DiagramTextView? {
        guard let document = current?.document else { return nil }
        return NSApp.windows.compactMap(\.contentView).flatMap { DiagramInspection.textViews(in: $0) }
            .first { $0.representedDocument == ObjectIdentifier(document) }
    }

    /// 菜单与快捷键共用一条队列：较早的切换完成后才开始下一次，第二次不会读到过渡中的表面。
    func toggleView() {
        let previous = toggleTask
        toggleTask = Task { [weak self] in
            _ = await previous?.value
            await self?.toggleCurrentView()
        }
    }
    func toggleCurrentView() async {
        guard let document = current?.document, !document.marked else { return }
        let entryView = AnchorInspection.isActive ? textView : nil
        if let entryView {
            AnchorInspection.record("toggle_enter", AnchorInspection.snapshot(entryView, document: document))
        }
        if document.reading {
            if let view = textView, let coordinator = view.readingCoordinator {
                let selected = view.selectedRange()
                let position = selected.length > 0 ? selected.location : coordinator.visibleInsertionPosition()
                var mapped: Int?
                if selected.length == 0, coordinator.atDocumentBottom() {
                    // 无选区且视口在文末：保留 EOF；显式选区仍按选区映射。
                    mapped = coordinator.sourceLength
                } else if let position, let source = coordinator.sourcePosition(at: position) {
                    mapped = source
                } else if let fallback = coordinator.visibleInsertionPosition() {
                    mapped = coordinator.sourcePosition(at: fallback)
                }
                if let mapped { document.sourceAnchor = mapped }
                if AnchorInspection.isActive {
                    var details = AnchorInspection.snapshot(view, document: document)
                    details["sample_position"] = position ?? NSNotFound
                    details["selected_location"] = selected.location
                    details["selected_length"] = selected.length
                    details["mapped_source"] = mapped ?? NSNotFound
                    details["at_bottom"] = coordinator.atDocumentBottom()
                    AnchorInspection.record("toggle_reading_anchor", details)
                }
            } else if AnchorInspection.isActive {
                AnchorInspection.record("toggle_reading_unavailable",
                                        AnchorInspection.snapshot(textView, document: document))
            }
        } else {
            // 只接受真正的编辑表面；过渡中仍挂载的阅读面使用显示坐标，不能写进源码锚点。
            let editing = textView.flatMap { $0.isReading ? nil : $0 }
            if let editing {
                let selected = editing.selectedRange()
                if selected.location != NSNotFound, selected.location >= 0,
                   selected.location <= editing.sourceUTF16Length,
                   editing.sourceUTF16Length > 0 || document.text.isEmpty {
                    document.sourceAnchor = selected.location
                }
            }
            if AnchorInspection.isActive {
                var details = AnchorInspection.snapshot(editing ?? entryView ?? textView, document: document)
                details["chosen_source"] = document.sourceAnchor
                details["editing_surface"] = editing != nil
                AnchorInspection.record("toggle_editing_anchor", details)
            }
            await document.saveNow()
            guard !document.dirty, document.issue != .saveFailed, document.issue != .conflict else { return }
        }
        document.reading.toggle(); scheduleSessionSave()
        if AnchorInspection.isActive {
            AnchorInspection.record("toggle_exit", AnchorInspection.snapshot(textView, document: document))
        }
        if !document.reading { focusEditor(document) }
    }
    /// 视图切换后编辑表面在下一轮渲染才出现；只在窗口仍为 key 时把焦点交给当前正文。
    private func focusEditor(_ document: NoteDocument) {
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else { return }
        Task { @MainActor [weak document] in
            guard let document else { return }
            for _ in 0..<80 {
                guard !document.reading, NSApp.keyWindow === window else { return }
                if let view = DiagramInspection.current(document: document), !view.isReading,
                   view.window === window, view.isEditable, window.makeFirstResponder(view) { return }
                try? await Task.sleep(for: .milliseconds(10))
            }
        }
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

    func scheduleLayoutSave() { application?.layoutChanged(self) }
    func scheduleSessionSave() { application?.scheduleSessionSave() }

    func sessionViews() -> [String: String] {
        Dictionary(uniqueKeysWithValues: tabs.map { tab in
            (tab.path, tab.document.map { $0.reading ? "reading" : "editing" } ?? restoredViews[tab.path] ?? defaultView)
        })
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
        epoch = UUID(); openingRequest = UUID()
        quickOpenTask?.cancel(); searchTask?.cancel(); searchCancellation?.cancel()
        for document in tabs.compactMap(\.document) { document.stopForShutdown() }
        let group = DispatchGroup()
        group.enter()
        terminals.stopForShutdown { group.leave() }
        Self.drainStateWrites(queue: saves, group: group, completion: completion)
    }

    nonisolated private static func drainStateWrites(queue: DispatchQueue, group: DispatchGroup,
                                                     completion: @escaping @Sendable () -> Void) {
        queue.async { group.wait(); completion() }
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
