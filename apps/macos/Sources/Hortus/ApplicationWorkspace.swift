import AppKit
import SwiftUI
import HortusCore

@Observable @MainActor
final class WindowRecord: @MainActor Identifiable {
    var id: String
    let workspace: Workspace
    let layout: Layout
    weak var window: NSWindow?
    @ObservationIgnored var delegate: VaultWindowDelegate?
    @ObservationIgnored var focusGuard: ToolbarFocusGuard?
    var closing = false
    var restoredFrame: WindowFrame?
    var lastUsed: UInt64 = 0
    var lastVisible: [String: Bool] = [:]

    init(id: String = UUID().uuidString, layout: Layout) {
        self.id = id
        self.layout = layout
        workspace = Workspace()
    }
}

/// 应用唯一的库注册表、布局排布与完整会话写入者。
@Observable @MainActor
final class ApplicationWorkspace {
    let arrangement = PanelArrangement()
    var records: [WindowRecord] = []
    var contexts: [String: VaultContext] = [:]
    var recent: [VaultState] = []
    var activeID: String?
    var restorationDiagnostics: [String] = []
    @ObservationIgnored private var activation: UInt64 = 0
    @ObservationIgnored var tabDrags: [UUID: (TabDragPayload, NoteDocument)] = [:]
    #if DEBUG
    @ObservationIgnored var tabDragTrace: [String] = []
    #endif
    private var bootstrapID: String = ""
    var ready = false
    private(set) var recoveredSessions = 0
    var fontSize = 15.0
    var defaultView = "reading"
    var defaultVisible: [String: Bool] = ["left": true, "right": true, "bottom": false]
    var latestSize: [Region: CGFloat] = [.left: 260, .right: 250, .bottom: 230]
    @ObservationIgnored var core: WorkspaceCore?
    @ObservationIgnored var showWindow: ((String) -> Void)?
    @ObservationIgnored private var started = false
    @ObservationIgnored private var restoring = false
    @ObservationIgnored private var sessionTask: Task<Void, Never>?
    @ObservationIgnored private var layoutTask: Task<Void, Never>?
    @ObservationIgnored private let saves = DispatchQueue(label: "app.hortus.session")
    @ObservationIgnored private var opening: Task<Void, Never>?
    @ObservationIgnored private var inspectionLaunch = false

    init() {
        records = [WindowRecord(layout: Layout(arrangement: arrangement))]
        activeID = records[0].id
        bootstrapID = records[0].id
        arrangement.onChange = { [weak self] in self?.scheduleLayoutSave() }
    }

    var initial: WindowRecord { records[0] }
    var active: WindowRecord? {
        let focused = activeID
        if let window = NSApp.keyWindow, let record = records.first(where: { $0.window === window }) { return record }
        return records.first { $0.id == focused } ?? records.first
    }

    func record(_ id: String?) -> WindowRecord? {
        records.first { $0.id == (id ?? bootstrapID) }
    }

    func start(arguments args: [String] = ProcessInfo.processInfo.arguments, core providedCore: WorkspaceCore? = nil) async {
        guard !started else { return }
        started = true
        restoring = true
        defer { restoring = false; ready = true; scheduleSessionSave() }
        do {
            let service = try await Task.detached { try providedCore ?? WorkspaceCore() }.value
            core = service
            // 上次退出时仍是运行中的会话不能假装进程还受管理。
            recoveredSessions = Int((try? await Task.detached { try service.recoverAgentSessions() }.value) ?? 0)
            let values = try await Task.detached {
                (try service.layoutJson(), try service.sessionJson(), try service.fontSize(), try service.defaultView())
            }.value
            fontSize = values.2; defaultView = values.3
            let session = try JSONDecoder().decode(SessionState.self, from: Data(values.1.utf8))
            recent = session.recent
            if let json = values.0 {
                initial.layout.restore(try JSONDecoder().decode(LayoutState.self, from: Data(json.utf8)))
                latestSize = initial.layout.size
                defaultVisible = Dictionary(uniqueKeysWithValues: initial.layout.visible.map { ($0.key.rawValue, $0.value) })
            }
            if let saved = session.default_visible, !saved.isEmpty { defaultVisible = saved }
            configure(initial)
            inspectionLaunch = !Self.shouldPersistSession(arguments: args, cacheOverride: ProcessInfo.processInfo.environment["HORTUS_CACHE_DIR"] != nil)
            if let arg = args.first(where: { $0.hasPrefix("--vault=") }) {
                await openNow(String(arg.dropFirst("--vault=".count)), source: initial.workspace)
            } else if !args.contains("--empty") {
                let windows = session.windows ?? session.last_vault.map { path in
                    let old = session.recent.first { $0.path == path }
                    return [WindowState(id: UUID().uuidString, path: path, tabs: old?.tabs ?? [],
                                        selected: old?.selected, views: old?.views ?? [:], frame: nil)]
                } ?? []
                for (index, window) in windows.enumerated() {
                    await openNow(window.path, source: index == 0 ? initial.workspace : nil, restoring: window)
                }
            }
        } catch { initial.workspace.error = L10n.error(error) }
    }

    func configure(_ record: WindowRecord) {
        guard let core else { return }
        record.layout.size = latestSize
        for region in Region.allCases { record.layout.visible[region] = defaultVisible[region.rawValue] ?? (region != .bottom) }
        record.lastVisible = visibility(record)
        record.workspace.configure(application: self, core: core, layout: record.layout)
        record.layout.onSizeChange = { [weak self, weak record] in
            guard let self, let record else { return }
            self.latestSize = record.layout.size
        }
        record.layout.onChange = { [weak self, weak record] in
            guard let self, let record else { return }
            self.layoutChanged(record.workspace)
        }
    }

    /// 显式检查启动不能覆盖真实会话；隔离缓存内仍允许检查保存与恢复。
    static func shouldPersistSession(arguments: [String], cacheOverride: Bool) -> Bool {
        cacheOverride || !arguments.contains { $0 == "--empty" || $0.hasPrefix("--vault=") }
    }

    /// 所有打开请求串行化，规范化路径在创建句柄前去重，返回 root 后再确认。
    func openFolder(_ path: String, source: Workspace? = nil) async {
        let previous = opening
        let task = Task { [weak self] in
            await previous?.value
            // 打开或激活另一窗口前完成原窗口的失焦保存，正文仍由原窗口持有。
            await source?.saveAllDocuments()
            await self?.openNow(path, source: source)
        }
        opening = task
        await task.value
    }

    private func openNow(_ path: String, source: Workspace?, restoring state: WindowState? = nil) async {
        guard let core else { return }
        do {
            let canonical = try await Task.detached {
                let url = URL(fileURLWithPath: path).resolvingSymlinksInPath().standardizedFileURL
                guard try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else {
                    throw CocoaError(.fileReadInvalidFileName)
                }
                return url.path
            }.value
            if state == nil, let record = mostRecentWindow(root: canonical) {
                activate(record)
                return
            }
            let context: VaultContext
            if let old = contexts[canonical] { context = old }
            else {
                let handle = try await Task.detached { try core.openVault(path: canonical) }.value
                let root = handle.root()
                if let old = contexts[root] {
                    await Task.detached { handle.close() }.value
                    context = old
                } else {
                    context = VaultContext(handle: handle)
                    contexts[root] = context
                }
            }
            if state == nil, let existing = mostRecentWindow(root: context.root) { activate(existing); return }
            let record: WindowRecord
            if let source, source.root == nil, let empty = records.first(where: { $0.workspace === source }) {
                record = empty
            } else {
                record = WindowRecord(id: state?.id ?? UUID().uuidString, layout: Layout(arrangement: arrangement))
                configure(record)
                records.append(record)
            }
            record.restoredFrame = state?.frame
            if let state {
                if record.id == bootstrapID { bootstrapID = state.id }
                record.id = state.id
            }
            let old = recent.first { $0.path == context.root }
            let saved = state.map { VaultState(path: context.root, tabs: $0.tabs, selected: $0.selected,
                                               views: $0.views, visible: $0.visible ?? old?.visible) } ?? old
            for region in Region.allCases {
                record.layout.visible[region] = saved?.visible?[region.rawValue] ?? defaultVisible[region.rawValue] ?? (region != .bottom)
            }
            record.lastVisible = visibility(record)
            await record.workspace.attach(context, state: saved)
            if record.window == nil {
                // 首个 Scene 已由 SwiftUI 创建；注册探针稍晚到达时不能再打开同一窗口。
                if record !== initial { showWindow?(record.id) }
            }
            else { restoreFrame(record); activate(record) }
            remember(record)
            scheduleSessionSave()
        } catch {
            // 每个失败库有自己的可见错误，后续有效窗口仍可恢复。
            let target = source.flatMap { source in records.first { $0.workspace === source } } ?? {
                let record = WindowRecord(layout: Layout(arrangement: arrangement))
                configure(record); records.append(record); showWindow?(record.id)
                return record
            }()
            target.workspace.error = "\(path): \(L10n.error(error))"
        }
    }

    func register(_ window: NSWindow, record: WindowRecord) {
        guard record.window !== window else { return }
        record.window = window
        let delegate = VaultWindowDelegate(application: self, record: record, forwarding: window.delegate)
        record.delegate = delegate
        window.delegate = delegate
        restoreFrame(record)
        if activeID == record.id { activate(record) }
        if window.toolbar != nil { record.focusGuard = ToolbarFocusGuard(window: window) }
    }

    private func restoreFrame(_ record: WindowRecord) {
        guard let window = record.window else { return }
        if let frame = record.restoredFrame {
            window.setFrame(frame.restored(in: NSScreen.screens.map(\.visibleFrame), cascade: records.firstIndex(where: { $0 === record }) ?? 0), display: true)
            record.restoredFrame = nil
        } else if records.count > 1 {
            window.cascadeTopLeft(from: NSPoint(x: 48 + records.count * 24, y: Int(NSScreen.main?.visibleFrame.maxY ?? 900) - records.count * 24))
        }
    }

    func activate(_ record: WindowRecord) {
        becameActive(record)
        record.window?.makeKeyAndOrderFront(nil)
        NSApp.activate()
    }

    func becameActive(_ record: WindowRecord) {
        activeID = record.id
        activation += 1; record.lastUsed = activation
        remember(record, promote: true)
    }

    private func mostRecentWindow(root: String) -> WindowRecord? {
        records.filter { !$0.closing && $0.workspace.root == root }.max { $0.lastUsed < $1.lastUsed }
    }

    /// 占位加载标签也预留唯一性；不能在异步打开完成前再创建一份会话。
    func owner(of path: String, from workspace: Workspace) -> WindowRecord? {
        guard let root = workspace.root else { return nil }
        return records.first { $0.workspace.root == root && $0.workspace.tabs.contains { $0.path == path } }
    }

    func recordDuplicate(_ path: String, window: Workspace) {
        let message = "Hortus session: duplicate note skipped in window \(records.first { $0.workspace === window }?.id ?? "?"): \(path)"
        restorationDiagnostics.append(message)
        fputs(message + "\n", stderr)
    }

    private func visibility(_ record: WindowRecord) -> [String: Bool] {
        Dictionary(uniqueKeysWithValues: record.layout.visible.map { ($0.key.rawValue, $0.value) })
    }

    func layoutChanged(_ workspace: Workspace) {
        guard let record = records.first(where: { $0.workspace === workspace }) else { return }
        let visible = visibility(record)
        let changed = visible != record.lastVisible
        record.lastVisible = visible
        remember(record, visibleWrite: changed)
        scheduleLayoutSave()
        scheduleSessionSave()
    }

    private func remember(_ record: WindowRecord, visibleWrite: Bool = false, promote: Bool = false) {
        guard let root = record.workspace.root else { return }
        let previous = recent.first { $0.path == root }
        let state = VaultState(path: root, tabs: record.workspace.tabs.map(\.path), selected: record.workspace.selected,
                               views: record.workspace.sessionViews(), visible: visibleWrite ? visibility(record) : previous?.visible ?? visibility(record))
        if let index = recent.firstIndex(where: { $0.path == root }) {
            recent.remove(at: index)
            recent.insert(state, at: promote ? 0 : index)
        } else { recent.insert(state, at: 0) }
        recent = Array(recent.prefix(20))
    }

    func snapshot() -> SessionState {
        for record in records { remember(record) }
        let windows = records.compactMap { record -> WindowState? in
            guard let root = record.workspace.root else { return nil }
            return WindowState(id: record.id, path: root, tabs: record.workspace.tabs.map(\.path),
                               selected: record.workspace.selected, views: record.workspace.sessionViews(),
                               frame: record.window.map { WindowFrame(window: $0) }, visible: visibility(record))
        }
        return SessionState(last_vault: active?.workspace.root ?? recent.first?.path, recent: recent,
                            windows: windows, default_visible: defaultVisible)
    }

    func scheduleSessionSave() {
        guard !restoring, !inspectionLaunch else { return }
        sessionTask?.cancel()
        sessionTask = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(250)) } catch { return }
            await self?.persistSession()
        }
    }

    func persistSession() async {
        guard !inspectionLaunch, let core else { return }
        do {
            let json = String(decoding: try JSONEncoder().encode(snapshot()), as: UTF8.self)
            try await save { try core.saveWindowSession(json: json) }
        } catch { active?.workspace.error = L10n.error(error) }
    }

    func scheduleLayoutSave() {
        guard !restoring else { return }
        layoutTask?.cancel()
        layoutTask = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(250)) } catch { return }
            guard let self, let core = self.core else { return }
            do {
                let state = self.layoutState()
                let json = String(decoding: try JSONEncoder().encode(state), as: UTF8.self)
                try await self.save { try core.saveLayout(json: json) }
            } catch { self.active?.workspace.error = L10n.error(error) }
        }
    }

    private func layoutState() -> LayoutState {
        let layout = Layout(arrangement: arrangement)
        layout.size = latestSize
        return layout.persistedState
    }

    private func save<T: Sendable>(_ operation: @escaping @Sendable () throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            saves.async { continuation.resume(with: Result(catching: operation)) }
        }
    }

    func requestClose(_ record: WindowRecord) {
        guard !record.closing else { return }
        if records.count == 1 { NSApp.terminate(nil); return }
        record.closing = true
        cancelTabDrags(in: record.workspace)
        Task {
            guard await record.workspace.prepareDocumentsForClose() == .ready else { record.closing = false; return }
            remember(record)
            await detach(record.workspace)
            records.removeAll { $0 === record }
            record.window?.delegate = record.delegate?.forwarding
            record.window?.close()
            record.delegate = nil; record.focusGuard = nil
            activeID = records.first?.id
            await persistSession()
        }
    }

    func detach(_ workspace: Workspace) async {
        if let record = records.first(where: { $0.workspace === workspace }) { remember(record) }
        let context = workspace.context
        await workspace.releaseDocuments()
        if let context, context.windowCount == 0 {
            contexts.removeValue(forKey: context.root)
            await context.close()
        }
        scheduleSessionSave()
    }

    /// 嵌套退出循环只依赖 DispatchGroup 与 RunLoop 源，不依赖 MainActor Task。
    func prepareShutdown(completion: @escaping @Sendable ([String]) -> Void) {
        let group = DispatchGroup()
        let collector = ShutdownFailures()
        for record in records {
            group.enter()
            let root = record.workspace.root ?? record.id
            record.workspace.prepareShutdown { paths in
                collector.append(paths.map { root + "/" + $0 }); group.leave()
            }
        }
        Self.finishDocuments(group: group, collector: collector, completion: completion)
    }

    nonisolated private static func finishDocuments(group: DispatchGroup, collector: ShutdownFailures,
                                                    completion: @escaping @Sendable ([String]) -> Void) {
        group.notify(queue: .global(qos: .userInitiated)) { completion(collector.values) }
    }

    func beginShutdownCleanup(completion: @escaping @Sendable () -> Void) {
        sessionTask?.cancel(); layoutTask?.cancel()
        tabDrags = [:]
        let core = core
        let session = inspectionLaunch ? nil : try? JSONEncoder().encode(snapshot())
        let layout = try? JSONEncoder().encode(layoutState())
        let group = DispatchGroup()
        for record in records { group.enter(); record.workspace.beginShutdownCleanup { group.leave() } }
        for context in contexts.values { group.enter(); context.close { group.leave() } }
        let delay = records.map { $0.workspace.shutdownDelayForInspection }.max() ?? 0
        Self.finishCleanup(group: group, queue: saves, core: core, session: session, layout: layout,
                           delay: delay, completion: completion)
    }

    nonisolated private static func finishCleanup(group: DispatchGroup, queue: DispatchQueue, core: WorkspaceCore?,
                                                 session: Data?, layout: Data?, delay: Double,
                                                 completion: @escaping @Sendable () -> Void) {
        queue.async {
            group.wait()
            do {
                if let core, let layout { try core.saveLayout(json: String(decoding: layout, as: UTF8.self)) }
                if let core, let session { try core.saveWindowSession(json: String(decoding: session, as: UTF8.self)) }
            } catch { fputs("Hortus session save failed: \(error)\n", stderr) }
            if delay > 0 { Thread.sleep(forTimeInterval: delay) }
            completion()
        }
    }
}

private final class ShutdownFailures: @unchecked Sendable {
    private let lock = NSLock()
    private var paths: [String] = []
    func append(_ paths: [String]) { lock.lock(); self.paths += paths; lock.unlock() }
    var values: [String] { lock.lock(); defer { lock.unlock() }; return paths }
}

@MainActor
final class VaultWindowDelegate: NSObject, NSWindowDelegate {
    weak var application: ApplicationWorkspace?
    weak var record: WindowRecord?
    nonisolated(unsafe) weak var forwarding: (any NSWindowDelegate)?
    init(application: ApplicationWorkspace, record: WindowRecord, forwarding: (any NSWindowDelegate)?) {
        self.application = application; self.record = record; self.forwarding = forwarding
    }
    override func responds(to selector: Selector!) -> Bool {
        if super.responds(to: selector) { return true }
        return MainActor.assumeIsolated { forwarding?.responds(to: selector) ?? false }
    }
    override func forwardingTarget(for selector: Selector!) -> Any? {
        forwarding
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if let record { application?.requestClose(record) }
        return false
    }
    func windowDidBecomeKey(_ notification: Notification) {
        if let record { application?.becameActive(record) }
        forwarding?.windowDidBecomeKey?(notification)
    }
    func windowDidMove(_ notification: Notification) { application?.scheduleSessionSave(); forwarding?.windowDidMove?(notification) }
    func windowDidResize(_ notification: Notification) { application?.scheduleSessionSave(); forwarding?.windowDidResize?(notification) }
}

struct WindowRegistration: NSViewRepresentable {
    let application: ApplicationWorkspace
    let record: WindowRecord
    func makeNSView(context: Context) -> Probe { Probe(application: application, record: record) }
    func updateNSView(_ view: Probe, context: Context) { view.register() }
    final class Probe: NSView {
        weak var application: ApplicationWorkspace?
        weak var record: WindowRecord?
        init(application: ApplicationWorkspace, record: WindowRecord) {
            self.application = application; self.record = record; super.init(frame: .zero)
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
        override func viewDidMoveToWindow() { super.viewDidMoveToWindow(); register() }
        func register() {
            guard let window, let record else { return }
            DispatchQueue.main.async { [weak self, weak window] in
                guard let self, let window else { return }
                self.application?.register(window, record: record)
            }
        }
    }
}
