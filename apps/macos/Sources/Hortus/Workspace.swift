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
    /// 测试注入的移到废纸篓确认；生产环境使用原生 NSAlert。
    #if DEBUG
    @ObservationIgnored var inspectRenameLinks: ((RenameLinkPlanInfo) -> RenameLinkChoice)?
    #endif
    @ObservationIgnored var promptTrash: (([String], String) -> Bool)?
    /// 测试注入的系统废纸篓移动；生产代码使用 FileManager。
    @ObservationIgnored var moveToTrash: (@Sendable (URL) throws -> URL?)?
    @ObservationIgnored var noteOpenStartedAt = 0.0
    @ObservationIgnored var noteOpenPhases: [String: Double] = [:]
    var diagramCore: WorkspaceCore? { core }
    let id = UUID()
    weak var application: ApplicationWorkspace?
    var context: VaultContext?
    var root: String? { context?.root }
    /// 自检读取当前标签的 shell PID；普通代码走布局选中的终端。
    var terminalPid: pid_t { terminals.currentSession?.inspectionShellPid ?? 0 }
    var currentTerminalTab: TerminalTab? { terminals.current }
    var currentTerminalSession: TerminalSession? { terminals.currentSession }
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
    let changes = ChangesModel()
    var fontSize = 15.0 {
        didSet { terminals.update(fontSize: fontSize, terminalFont: terminalFont) }
    }
    var terminalFont: String? {
        didSet { terminals.update(fontSize: fontSize, terminalFont: terminalFont) }
    }
    var defaultView = "reading"
    var scanMicros: UInt64 { context?.scanMicros ?? 0 }
    var searchIndexReadyMilliseconds: Double? { context?.searchIndexReadyMilliseconds }
    var searchIndexPeakRSSDeltaBytes: Int64? { context?.searchIndexPeakRSSDeltaBytes }
    var restoredViews: [String: String] = [:]
    /// 文件树中的文件夹选中（新建文件夹后展示；打开笔记时清空回到标签选中）。
    var folderSelection: String?
    /// 新建笔记直接进入编辑视图的待处理路径。
    @ObservationIgnored var pendingEditingPaths: Set<String> = []
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
    var tagFilter = ""
    var selectedTag: String?
    var knowledgeMessage: String?
    @ObservationIgnored private var knowledgeNavigation: Task<Void, Never>?
    @ObservationIgnored private var knowledgeMessageTask: Task<Void, Never>?
    @ObservationIgnored private var pendingLinkHeading: (path: String, heading: String)?
    @ObservationIgnored var core: WorkspaceCore?
    @ObservationIgnored private var epoch = UUID()
    @ObservationIgnored private var openingRequest = UUID()
    @ObservationIgnored private var loads: [String: UUID] = [:]
    @ObservationIgnored private var loadTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var releasing = false
    @ObservationIgnored private var openedAt = ContinuousClock.now
    private(set) var layout: Layout?
    @ObservationIgnored private let saves = DispatchQueue(label: "app.hortus.state")
    @ObservationIgnored private var quickOpenTask: Task<Void, Never>?
    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var searchCancellation: SearchCancellation?
    @ObservationIgnored private var quickOpenMarked = false
    @ObservationIgnored private var searchMarked = false
    @ObservationIgnored private var pendingSearchReveals: [String: (start: Int, length: Int)] = [:]

    init() {
        terminals.workspace = self
        changes.workspace = self
    }

    /// 当前窗口最近使用的 Agent 会话；面板默认优先选中它。
    var currentAgentSessionID: String? {
        if let tab = terminals.current, tab.kind == .agent, let id = tab.agentSessionID {
            return id
        }
        return terminals.allTabs.last { $0.kind == .agent }?.agentSessionID
    }

    var current: NoteTab? { tabs.first { $0.path == selected } }
    var title: String { current?.title ?? root.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "Hortus" }
    private var vault: VaultHandle? { context?.handle }

    func configure(application: ApplicationWorkspace, core: WorkspaceCore, layout: Layout) {
        self.application = application
        self.core = core
        self.layout = layout
        fontSize = application.fontSize
        terminalFont = application.terminalFont
        defaultView = application.defaultView
    }

    /// 设置保存后的即时传播：字号与终端字体立即作用于此窗口，默认视图只影响后续打开的笔记。
    func applySettings(fontSize: Double, defaultView: String, terminalFont: String?) {
        self.fontSize = fontSize
        self.terminalFont = terminalFont
        self.defaultView = defaultView
    }

    func attach(_ context: VaultContext, state: VaultState?) async {
        self.context = context
        openedAt = .now
        restoredViews = state?.views ?? [:]
        context.attach(self)
        await terminals.setRootAndWait(context.root)
        await ensureTerminalTools()
        for path in state?.tabs ?? [] {
            if application?.owner(of: path, from: self) != nil {
                application?.recordDuplicate(path, window: self)
            } else { openNote(path) }
        }
        if let selected = state?.selected, tabs.contains(where: { $0.path == selected }) { self.selected = selected }
    }

    // MARK: 统一工具标签

    /// 每个窗口一个普通 Shell；布局里的终端占位 UUID 就位后按它建立会话。
    func ensureTerminalTools() async {
        guard let layout else { return }
        guard let root else {
            layout.removeAllTerminalTools()
            await terminals.trim(to: [])
            return
        }
        if let first = layout.terminalIDs.first {
            layout.keepOnlyTerminal(first)
            terminals.adopt(id: first, kind: .shell, root: root)
            await terminals.trim(to: [first])
        } else if let tab = terminals.createShell() {
            layout.insertTerminal(tab.id)
            await terminals.trim(to: [tab.id])
        }
    }

    /// 工具标签的新增与关闭只改布局；Layout.onChange → ApplicationWorkspace.layoutChanged
    /// 已经统一持久化布局与会话，这里不再重复调度。
    @discardableResult
    func addShell(to groupID: UUID? = nil) -> TerminalTab? {
        guard root != nil, let layout, let tab = terminals.createShell() else { return nil }
        layout.add(.terminal(tab.id), to: groupID ?? layout.defaultTerminalGroupID)
        layout.reveal(.terminal(tab.id))
        terminals.focusSoon()
        return tab
    }

    @discardableResult
    func addAgent(to groupID: UUID? = nil) -> TerminalTab? {
        guard root != nil, let layout, let tab = terminals.createAgent() else { return nil }
        layout.add(.terminal(tab.id), to: groupID ?? layout.defaultTerminalGroupID)
        layout.reveal(.terminal(tab.id))
        terminals.start(tab)
        terminals.focusSoon()
        return tab
    }

    func addStaticTool(_ tool: StaticTool, to groupID: UUID?) {
        layout?.add(.tool(tool), to: groupID)
    }

    func showStaticTool(_ tool: StaticTool, in region: Region = .left) {
        guard let layout else { return }
        if layout.reveal(.tool(tool)) { return }
        if layout.availableGroups(in: region).isEmpty {
            layout.add(.tool(tool), in: region)
        } else {
            layout.add(.tool(tool), to: layout.availableGroups(in: region).last?.id)
        }
    }

    /// 关闭工具标签。终端只终止自己的进程组，其它工具与会话不变。
    func closeTool(_ tool: ToolID) {
        if let id = tool.terminalID {
            Task { await closeTerminal(id) }
        } else {
            layout?.remove(tool)
        }
    }

    func closeTerminal(_ id: UUID) async {
        await terminals.stopAndRemove(id)
        layout?.remove(.terminal(id))
    }

    /// ⌃`：显示并聚焦最近使用的终端；没有终端时创建一个默认 Shell。
    func showTerminal(focus: Bool = true) {
        guard let layout else { return }
        if let id = layout.currentTerminalID ?? layout.terminalIDs.first {
            if terminals.tab(id) == nil, let root { terminals.adopt(id: id, kind: .shell, root: root) }
            layout.reveal(.terminal(id))
        } else {
            _ = addShell(to: nil)
        }
        if focus { terminals.focusSoon() }
    }

    func clearTerminal() { terminals.clearScreen() }

    /// 工具栏显示区域；空区域补一个默认工具，+ 始终可达。
    func toggleRegion(_ region: Region) {
        guard let layout else { return }
        if layout.isShown(region) {
            layout.visible[region] = false
            return
        }
        if layout.availableGroups(in: region).isEmpty {
            if region == .bottom {
                if let tab = addShell(to: nil) {
                    layout.reveal(.terminal(tab.id))
                } else {
                    _ = layout.ensureGroup(in: .bottom)
                }
            } else if let added = layout.ensureGroup(in: region), let id = added.terminalID, let root {
                terminals.adopt(id: id, kind: .shell, root: root)
            }
        }
        layout.visible[region] = true
    }

    func resetLayout() {
        guard let layout else { return }
        layout.reset()
        Task { await ensureTerminalTools() }
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
        focusSession = nil
        syncFocusDiagramMenu()
        application?.cancelTabDrags(in: self)
        // 加载占位在打开完成前保留；库关闭与再次打开也不能重叠创建会话。
        for task in loadTasks.values { await task.value }
        epoch = UUID()
        openingRequest = UUID()
        for tab in tabs { tab.document?.stopForShutdown() }
        await terminals.setRootAndWait(nil)
        changes.reset()
        quickOpenTask?.cancel(); searchTask?.cancel(); searchCancellation?.cancel()
        knowledgeNavigation?.cancel(); knowledgeMessageTask?.cancel()
        knowledgeMessage = nil; selectedTag = nil; tagFilter = ""; pendingLinkHeading = nil
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

    /// 回滚后只刷新受影响的干净标签；脏编辑、组合输入和活动冲突由面板预先阻止。
    func refreshAfterRollback(_ paths: Set<String>) {
        guard !paths.isEmpty else { return }
        for path in paths {
            guard let document = tabs.first(where: { $0.path == path })?.document else { continue }
            guard !document.dirty, !document.marked, document.issue != .conflict else { continue }
            document.refresh()
        }
    }

    /// 未保存输入、组合输入或活动冲突的标签不允许被回滚静默覆盖。
    func hasUnsavedEdits(in paths: Set<String>) -> Bool {
        tabs.contains { tab in
            guard paths.contains(tab.path), let document = tab.document else { return false }
            return document.dirty || document.marked || document.issue == .conflict
        }
    }

    /// 后台线程更新完搜索内容后回到主线程：记录索引就绪，刷新正在显示的搜索结果
    func searchIndexUpdated(changed: Bool, scanFinished: Bool) {
        guard changed || scanFinished, !scanning else { return }
        if quickOpenVisible, !quickOpenMarked { updateQuickOpen(query: quickOpenQuery, marked: false) }
        if !searchMarked, !searchQuery.isEmpty { updateSearch(query: searchQuery, marked: false) }
    }

    func openNote(_ path: String, savePrevious: Bool = true) {
        folderSelection = nil
        guard vault != nil, !releasing, !preparingClose else { return }
        if let owner = application?.owner(of: path, from: self), owner.workspace !== self {
            owner.workspace.openNote(path, savePrevious: savePrevious)
            application?.activate(owner)
            return
        }
        if selected != path || current?.document == nil {
            noteOpenStartedAt = ProcessInfo.processInfo.systemUptime
            noteOpenPhases = [:]
        }
        if selected != path, focusSession != nil { exitFocus(restore: false) }
        if selected != path, savePrevious, let previous = current?.document { Task { await previous.saveNow() } }
        if let pendingLinkHeading, pendingLinkHeading.path != path { self.pendingLinkHeading = nil }
        if !tabs.contains(where: { $0.path == path }) {
            tabs.append(NoteTab(path: path))
            load(path)
        }
        selected = path
        syncFocusDiagramMenu()
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
        showStaticTool(.search, in: .left)
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

    func openSearchResult(path: String, start: Int, length: Int, savePrevious: Bool = true) {
        if let owner = application?.owner(of: path, from: self), owner.workspace !== self {
            owner.workspace.openSearchResult(path: path, start: start, length: length, savePrevious: savePrevious)
            application?.activate(owner)
            return
        }
        // 同篇搜索结果与反链也必须在正文层可见，专注覆盖层不能吞掉这次定位。
        if focusSession != nil { exitFocus(restore: false) }
        pendingSearchReveals[path] = (start, length)
        if selected == path, let document = current?.document {
            revealSearch(document: document, start: start, length: length)
        } else {
            openNote(path, savePrevious: savePrevious)
            if let document = tabs.first(where: { $0.path == path })?.document {
                Task { @MainActor in
                    await Task.yield()
                    self.revealSearch(document: document, start: start, length: length)
                }
            }
        }
    }

    func showTag(_ tag: String) {
        knowledgeMessage = nil
        selectedTag = tag
        tagFilter = ""
        showStaticTool(.tags)
    }

    func openBacklink(_ entry: BacklinkInfo) {
        let path = entry.source
        let owner = application?.owner(of: path, from: self)?.workspace ?? self
        guard let document = owner.tabs.first(where: { $0.path == path })?.document,
              document.dirty else {
            openSearchResult(path: path, start: Int(entry.startUtf16),
                             length: Int(entry.lengthUtf16), savePrevious: false)
            return
        }
        // 索引偏移来自保存时的快照；脏文档必须按实时正文重新定位，避免跳到旧位置。
        let handle = context?.handle
        let target = entry.target, heading = entry.heading, alias = entry.alias
        let embed = entry.embed, inDiagram = entry.inDiagram, line = entry.line
        let original = Int(entry.startUtf16)
        Task { @MainActor [weak self] in
            guard let self, self.context != nil else { return }
            let text = (try? await document.io.perform { try $0.snapshot().text }) ?? document.text
            var start = original
            var length = Int(entry.lengthUtf16)
            if let handle {
                let markup = await Task.detached {
                    handle.knowledgeMarkup(path: path, text: text)
                }.value
                let candidates = markup.links.filter {
                    $0.target == target && $0.heading == heading && $0.alias == alias
                        && $0.embed == embed && $0.inDiagram == inDiagram
                }
                let link = candidates.first(where: { Int($0.line) == Int(line) })
                    ?? candidates.min { abs(Int($0.startUtf16) - original) < abs(Int($1.startUtf16) - original) }
                if let link {
                    start = Int(link.startUtf16)
                    length = Int(link.lengthUtf16)
                } else {
                    start = Self.lineStart(line, in: text)
                    length = 0
                }
            }
            self.openSearchResult(path: path, start: start, length: length, savePrevious: false)
        }
    }

    private static func lineStart(_ line: UInt64, in text: String) -> Int {
        let source = text as NSString
        guard line > 1 else { return 0 }
        var location = 0
        for _ in 1..<line {
            guard location < source.length else { return source.length }
            location = NSMaxRange(source.lineRange(for: NSRange(location: location, length: 0)))
        }
        return min(location, source.length)
    }

    func openLink(source: String, target: String, heading: String?) {
        guard let context else { return }
        let handle = context.handle
        knowledgeNavigation?.cancel()
        knowledgeNavigation = Task { [weak self] in
            let link = await Task.detached { handle.resolveLink(sourcePath: source, rawTarget: target) }.value
            guard !Task.isCancelled, let self, self.context === context, self.selected == source else { return }
            if let path = link.resolvedPath {
                knowledgeMessage = nil
                let owner = application?.owner(of: path, from: self)
                let workspace = owner?.workspace ?? self
                workspace.pendingLinkHeading = heading.map { (path, $0) }
                workspace.openNote(path, savePrevious: false)
                workspace.revealPendingLinkHeading()
                if let owner, workspace !== self { application?.activate(owner) }
            } else if link.resolutionKind == "ambiguous" {
                showKnowledgeMessage(L10n.string("Several notes match “%@”: %@", target,
                                                 link.candidates.prefix(3).joined(separator: ", ")))
            } else if link.resolutionKind == "broken" {
                showKnowledgeMessage(L10n.string("No note found for “%@”", target))
            }
        }
    }

    private func revealPendingLinkHeading() {
        guard let request = pendingLinkHeading, selected == request.path, let document = current?.document,
              document.outlineRevision == document.revision else { return }
        pendingLinkHeading = nil
        let title = request.heading.trimmingCharacters(in: .whitespacesAndNewlines)
        if let heading = document.headings.first(where: {
            $0.title.trimmingCharacters(in: .whitespacesAndNewlines).caseInsensitiveCompare(title) == .orderedSame
        }) {
            openSearchResult(path: request.path, start: Int(heading.start), length: 0, savePrevious: false)
        } else {
            showKnowledgeMessage(L10n.string("No heading found for “%@”", title))
        }
    }

    func showKnowledgeMessage(_ message: String) {
        knowledgeMessageTask?.cancel()
        knowledgeMessage = message
        knowledgeMessageTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(5)) } catch { return }
            self?.knowledgeMessage = nil
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
        guard let view = document.mountedTextView else { return }
        if document.reading {
            view.readingCoordinator?.reveal(source: start, length: length)
        } else {
            let location = max(0, min(start, view.sourceUTF16Length))
            view.revealCharacter(at: location)
            view.setSelectedRange(NSRange(location: location, length: min(length, view.sourceUTF16Length - location)))
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
                let stored = restoredViews[path] ?? recent.first(where: { $0.path == root })?.views?[path] ?? defaultView
                // 新建笔记直接进入编辑视图，不读取历史视图记录。
                let reading = pendingEditingPaths.remove(path) == nil && stored != "editing"
                let document = NoteDocument(io: loaded.0, initial: loaded.1, prepareSourceStyles: !reading)
                if let reveal = pendingSearchReveals.removeValue(forKey: path) {
                    document.sourceAnchor = reveal.start
                    document.sourceSelectionLength = reveal.length
                }
                document.reading = reading
                self.bind(document)
                self.tabs[index].document = document; self.tabs[index].error = nil
                self.revealPendingLinkHeading()
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
        if focusSession?.note == target { exitFocus(restore: false) }
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
                  self.tabs.contains(where: { $0.document === document }) else { return }
            let persistence = await document.prepareToClose()
            // 重命名可能在等待持久化期间改掉路径；关闭请求以文档身份为准。
            guard self.epoch == epoch,
                  let index = self.tabs.firstIndex(where: { $0.document === document })
            else { return }
            let current = self.tabs[index].path
            if persistence != .unsaved {
                self.tabs[index].document?.stopForShutdown()
                self.removeTab(current)
                return
            }
            switch self.decideUnsaved([current], quitting: false) {
            case .retry:
                self.closingTabs.insert(current)
                self.closeTabWhenPersisted(current, document, epoch: epoch)
            case .discard: document.abandon(); self.removeTab(current)
            case .cancel: self.closingTabs.remove(current)
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
    static func noteList(_ notes: [String]) -> String {
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
        syncFocusDiagramMenu()
        scheduleSessionSave()
    }

    /// 重命名成功后的本窗口迁移：标签路径、选中状态、视图记录与文档会话
    /// 一次性搬到新路径；文件树与搜索索引由 VaultContext 同步。
    @discardableResult
    func applyRename(oldPath: String, entry: TreeEntry) -> [Task<Void, Never>] {
        let prefix = entry.directory ? oldPath + "/" : nil
        func remap(_ path: String) -> String {
            if path == oldPath { return entry.path }
            if let prefix, path.hasPrefix(prefix) {
                return entry.path + "/" + path.dropFirst(prefix.count)
            }
            return path
        }
        // 旧路径的专注会话会持续写旧缓存并持有已迁移文档，先退出再搬标签。
        if let session = focusSession,
           session.note == oldPath || prefix.map({ session.note.hasPrefix($0) }) == true {
            exitFocus(restore: false)
        }
        var relocations: [Task<Void, Never>] = []
        for index in tabs.indices {
            let old = tabs[index].path
            let newPath = remap(old)
            guard newPath != old else { continue }
            if tabs[index].loading {
                // 加载中的占位标签作废旧请求，按新路径重新加载。
                loadTasks.removeValue(forKey: old)?.cancel()
                loads.removeValue(forKey: old)
                tabs[index].path = newPath
                load(newPath)
            } else {
                tabs[index].path = newPath
                if let document = tabs[index].document {
                    let root = root ?? ""
                    relocations.append(Task { await document.relocate(root: root, to: newPath) })
                }
            }
            if selected == old { selected = newPath }
        }
        // 未打开标签的视图记录、待定位与关闭中路径也必须按精确/前缀一起搬家。
        func remapKeys<T>(_ values: inout [String: T]) {
            var next: [String: T] = [:]
            next.reserveCapacity(values.count)
            for (key, value) in values { next[remap(key)] = value }
            values = next
        }
        remapKeys(&restoredViews)
        remapKeys(&pendingSearchReveals)
        closingTabs = Set(closingTabs.map(remap))
        if let focus = folderSelection, focus == oldPath || prefix.map({ focus.hasPrefix($0) }) == true {
            folderSelection = remap(focus)
        }
        scheduleSessionSave()
        return relocations
    }

    /// 移到废纸篓流程专用：会话已由调用方停止，直接移除标签，不触发未保存提示。
    func discardTabForTrash(_ path: String) {
        guard let index = tabs.firstIndex(where: { $0.path == path }) else { return }
        if let document = tabs[index].document,
           focusSession?.note == path || focusSession?.document === document {
            exitFocus(restore: false)
        }
        if let document = tabs[index].document { document.abandon() }
        loadTasks.removeValue(forKey: path)?.cancel()
        loads.removeValue(forKey: path)
        tabs.remove(at: index)
        closingTabs.remove(path)
        if selected == path { selected = tabs.isEmpty ? nil : tabs[min(index, tabs.count - 1)].path }
        pendingSearchReveals.removeValue(forKey: path)
        scheduleSessionSave()
    }

    @ObservationIgnored var closingTabs: Set<String> = []
    @ObservationIgnored var preparingClose = false
    @ObservationIgnored private var toggleTask: Task<Void, Never>?
    /// 专注模式覆盖编辑区；正文表面保持原样以便退出时精确恢复。
    var focusSession: DiagramFocusSession?
    /// 专注模式里折叠节点后同步回正文：只递增令牌，由正文视图自行重排。
    var diagramRefreshToken = 0

    func bind(_ document: NoteDocument) {
        document.onSaved = { [weak self, weak document] in
            guard let self, let document else { return }
            if let path = self.tabs.first(where: { $0.document === document })?.path {
                self.context?.refreshSavedNote(path)
            }
        }
        document.onOutlineReady = { [weak self] in self?.revealPendingLinkHeading() }
        document.onCopy = { [weak self] copy in self?.openNote(copy) }
        document.onDiagramAvailabilityChange = { [weak self, weak document] in
            guard let self, document === self.current?.document else { return }
            self.syncFocusDiagramMenu()
        }
    }

    /// SwiftUI 的 CommandMenu 不随图块异步排版或专注会话变化重建；显式同步原生菜单项。
    /// 只有当前活跃窗口能写全局菜单，背景窗口的排版完成不能覆盖前台状态。
    func syncFocusDiagramMenu() {
        if let application, application.active?.workspace !== self { return }
        FocusMenuState.setEnabled(canFocusDiagram)
    }

    /// 转移不停止文档队列，也不按路径重新载入。原生编辑表面携带正文与撤销栈。
    func takeTab(_ path: String) -> NoteTab? {
        guard let tab = tabs.first(where: { $0.path == path }), !tab.loading,
              let document = tab.document, !document.marked, !closingTabs.contains(path), !preparingClose else { return nil }
        if focusSession?.note == path { exitFocus(restore: false) }
        if let coordinator = document.viewOwner as? NoteText.EditorCoordinator {
            document.transferredEditor = coordinator.host?.take()
        } else if let coordinator = document.viewOwner as? ReadingText.Coordinator {
            coordinator.detach()
        }
        pendingSearchReveals.removeValue(forKey: path)
        removeTab(path)
        return tab
    }

    var textView: DiagramTextView? { current?.document?.mountedTextView }

    /// 菜单与快捷键共用一条队列：较早的切换完成后才开始下一次，第二次不会读到过渡中的表面。
    func toggleView() {
        if focusSession != nil { exitFocus(restore: false) }
        enqueueViewSwitch { [weak self] in await self?.toggleCurrentView() }
    }

    /// 阅读面点击图节点/边跳源码：与 ⌘E 共用同一条队列、锚点状态、会话保存与聚焦。
    func switchToEditing(document: NoteDocument, anchor: Int, selectionLength: Int) {
        enqueueViewSwitch { [weak self] in
            await self?.enterEditing(document, anchor: anchor, selectionLength: selectionLength)
        }
    }

    private func enqueueViewSwitch(_ operation: @escaping @MainActor () async -> Void) {
        let previous = toggleTask
        toggleTask = Task {
            _ = await previous?.value
            await operation()
        }
    }

    private func enterEditing(_ document: NoteDocument, anchor: Int, selectionLength: Int) async {
        guard current?.document === document, !document.marked else { return }
        document.sourceAnchor = anchor
        document.sourceSelectionLength = selectionLength
        if document.reading {
            document.reading = false
            scheduleSessionSave()
        }
        focusEditor(document)
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
    func focusEditor(_ document: NoteDocument) {
        guard let app = NSApp, let window = app.keyWindow ?? app.mainWindow else { return }
        Task { @MainActor [weak document] in
            guard let document else { return }
            for _ in 0..<80 {
                guard !document.reading, app.keyWindow === window else { return }
                if let view = document.mountedTextView, !view.isReading,
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

    /// 专注模式折叠后立刻落缓存，正文随后的重排才能读到同一状态。
    func rememberDiagramNow(note: String, block: UInt32, collapsed: [String]) {
        guard let core, let root else { return }
        do { try core.rememberDiagram(vault: root, note: note, block: block, collapsed: collapsed) }
        catch { self.error = L10n.error(error) }
    }

    func rememberDiagramFocus(note: String, block: UInt32, scale: Double, x: Double, y: Double) {
        guard let core, let root else { return }
        saves.async {
            do {
                try core.rememberDiagramFocus(vault: root, note: note, block: block,
                                             scale: scale, offsetX: x, offsetY: y)
            } catch { DispatchQueue.main.async { self.error = L10n.error(error) } }
        }
    }

    /// 光标所在图块优先；否则取阅读视图里视口内的首个图块。
    func focusTarget() -> NoteDiagram? {
        guard let document = current?.document, let view = document.mountedTextView else { return nil }
        let prepared = view.blocks.filter { $0.prepared != nil }
        guard !prepared.isEmpty else { return nil }
        if !document.reading, let caret = view.selectedRanges.first?.rangeValue.location,
           let block = prepared.first(where: { NSLocationInRange(caret, $0.range) }) {
            return block
        }
        if let visible = prepared.first(where: { block in
            view.diagramViews[block.ordinal].map { !$0.isHidden && $0.frame.intersects(view.visibleRect) } ?? false
        }) { return visible }
        return prepared.first
    }

    /// 菜单启用只看可观察状态：图块排版完成或已处于专注模式。
    var canFocusDiagram: Bool { focusSession != nil || current?.document?.hasPreparedDiagrams == true }

    func enterFocus(block ordinal: UInt32) {
        guard focusSession == nil, let document = current?.document, let path = selected,
              let vault = root, let view = document.mountedTextView else { return }
        var initial: DiagramFocus?
        if let core { initial = try? core.diagramFocus(vault: vault, note: path, block: ordinal) }
        focusSession = DiagramFocusSession(
            note: path, vault: vault, ordinal: ordinal, document: document,
            reading: document.reading,
            selection: view.selectedRanges.first?.rangeValue ?? NSRange(location: 0, length: 0),
            scroll: view.enclosingScrollView?.contentView.bounds.origin ?? .zero,
            initial: initial)
        syncFocusDiagramMenu()
    }

    func toggleFocusMode() {
        if focusSession != nil { exitFocus() } else if let target = focusTarget() { enterFocus(block: target.ordinal) }
    }

    /// 退出专注模式；正文从未被改动，只恢复进入前的滚动位置与选区。
    func exitFocus(restore: Bool = true) {
        guard let session = focusSession else { return }
        focusSession = nil
        syncFocusDiagramMenu()
        guard restore, session.document === current?.document else { return }
        let document = session.document
        Task { @MainActor in
            await Task.yield()
            guard let view = document.mountedTextView else { return }
            if let scroll = view.enclosingScrollView {
                scroll.contentView.scroll(to: session.scroll)
                scroll.reflectScrolledClipView(scroll.contentView)
            }
            if !document.reading { view.setSelectedRange(session.selection) }
            view.window?.makeFirstResponder(view)
        }
    }

    /// 专注模式内 ⌘-点击：退出覆盖层并切到编辑视图选中源码。
    func jumpFromFocus(range: NSRange) {
        guard let session = focusSession else { return }
        exitFocus(restore: false)
        if session.document.reading {
            switchToEditing(document: session.document, anchor: range.location, selectionLength: range.length)
        } else {
            revealEditing(document: session.document, range: range)
        }
    }

    private func revealEditing(document: NoteDocument, range: NSRange) {
        Task { @MainActor in
            for _ in 0..<80 {
                if let view = document.mountedTextView, !view.isReading, view.window != nil {
                    view.window?.makeFirstResponder(view)
                    view.setSelectedRange(range)
                    view.scrollRangeToVisible(range)
                    return
                }
                try? await Task.sleep(for: .milliseconds(10))
            }
        }
    }

    func refreshDiagramBodies() { diagramRefreshToken &+= 1 }

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
