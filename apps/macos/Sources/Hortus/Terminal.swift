import AppKit
import Darwin
import SwiftTerm
import SwiftUI

/// 单个 PTY 的执行体：普通 shell 或一个 Agent。
///
/// SwiftTerm 拥有 PTY 和终端仿真；窗口级的标签控制器拥有多个实例。
/// 这个对象只负责一条会话的启动、终止与退出等待。
@Observable @MainActor
final class TerminalSession {
    enum Kind {
        case shell
        case agent
    }

    enum State: Equatable {
        case empty
        /// 库根已设置，视图挂载后启动 shell。
        /// 只有这个状态会自动启动 shell，自己退出的 shell 不因界面刷新被重新拉起。
        case starting
        /// Agent 模板已通过，等待视图挂载后 exec。
        case startingAgent
        case running
        case exited(Int32?)
        case failed
    }

    private(set) var state: State = .empty
    private(set) var root: String?
    private var view: HortusTerminalView?
    private var stopWaiters: [CheckedContinuation<Void, Never>] = []
    private var killTask: Task<Void, Never>?
    private(set) var nerdFonts: TerminalNerdFonts?
    /// 进程结束回调由标签控制器接管，用于记录 Agent 会话终态。
    var onTerminated: ((Int32?) -> Void)?
    var onFailedToStart: (() -> Void)?
    private var pendingAgent: AgentStart?
    #if DEBUG
    private(set) var inspectionFontReadyBeforeShell = false
    #endif

    struct AgentStart {
        let executable: String
        let args: [String]
        let session: String
    }

    private let kind: Kind

    init(kind: Kind = .shell) {
        self.kind = kind
        let installed = TerminalNerdFonts.installed
        Task { [weak self] in
            let catalog = await installed.value
            guard let self else { return }
            self.nerdFonts = catalog
            self.view?.glyphFallbackProvider = catalog.provider
            self.startIfPossible()
        }
    }

    var statusText: String {
        switch state {
        case .empty: return L10n.string("Open a knowledge base to use the terminal")
        case .starting, .startingAgent, .running: return ""
        case .exited: return L10n.string("Process exited")
        case .failed: return L10n.string("Unable to start the shell")
        }
    }

    var isRunning: Bool {
        if case .running = state { return view?.process.running == true }
        return false
    }

    func attach(_ view: HortusTerminalView, fontSize: Double) {
        self.view = view
        view.processDelegate = view.delegateProxy
        view.applyAppearance(fontSize: fontSize)
        view.glyphFallbackProvider = nerdFonts?.provider
        startIfPossible()
        startPendingAgent()
    }

    /// 状态和 Restart 覆盖层只用于已经退出或失败的 shell。
    var showsStatusOverlay: Bool {
        switch state {
        case .exited, .failed: true
        case .empty, .starting, .startingAgent, .running: false
        }
    }

    func detach(_ view: HortusTerminalView) {
        // SwiftUI 移动面板时复用同一个 PTY 视图，不能在这里结束进程。
        guard self.view === view else { return }
    }

    func update(fontSize: Double) {
        view?.applyAppearance(fontSize: fontSize)
    }

    func focus() {
        guard let view, let window = view.window else { return }
        window.makeFirstResponder(view)
        UndoMenuState.scheduleRefresh()
    }

    func clearScreen() {
        guard let view, view.process.running else { return }
        view.send([12])
    }

    func makeView(fontSize: Double) -> HortusTerminalView {
        if let view {
            view.removeFromSuperview()
            attach(view, fontSize: fontSize)
            return view
        }
        let view = HortusTerminalView(session: self)
        attach(view, fontSize: fontSize)
        return view
    }

    func setRoot(_ root: String?) {
        Task { @MainActor [weak self] in await self?.setRootAndWait(root) }
    }

    /// 新窗口没有旧 PTY 需要等待；在第一次关闭可能插入前设置根目录。
    func setInitialRoot(_ root: String) {
        guard self.root == nil, state == .empty else { return }
        self.root = root
        state = .starting
        startIfPossible()
    }

    func setRootAndWait(_ root: String?) async {
        // 再次打开同一个库保留正在运行的 shell。
        if self.root == root, state == .running { return }
        if self.root != root {
            await stopAndWait()
        }
        self.root = root
        state = root == nil ? .empty : .starting
        startIfPossible()
    }

    func restart() {
        guard kind == .shell, root != nil else { return }
        switch state {
        case .exited, .failed: break
        default: return
        }
        guard let view, !view.process.running, !view.process.windingDown else { return }
        state = .starting
        startIfPossible()
    }

    /// Agent 的 exec 在模板校验和快照完成后才开始；视图尚未挂载时排队。
    @discardableResult
    func startAgent(executable: String, args: [String], session: String) -> Bool {
        guard root != nil else { return false }
        pendingAgent = AgentStart(executable: executable, args: args, session: session)
        state = .startingAgent
        startPendingAgent()
        return true
    }

    func stopAndWait() async {
        pendingAgent = nil
        if state == .startingAgent {
            state = .empty
            return
        }
        guard let view, view.process.running || view.process.windingDown else {
            killTask?.cancel()
            return
        }
        await withCheckedContinuation { continuation in
            stopWaiters.append(continuation)
            sendTerminationSignals(for: view)
        }
    }

    /// 在 AppKit 进入 terminateLater 的嵌套循环前开始关闭。
    /// 那里不能依赖 MainActor 任务或 SwiftTerm 的主队列退出监视。
    func stopForShutdown(completion: @escaping @Sendable () -> Void) {
        killTask?.cancel()
        killTask = nil
        pendingAgent = nil
        guard let pid = view?.process.shellPid, pid > 0 else {
            state = .empty
            completion()
            return
        }
        Self.signalProcessGroup(pid, signal: SIGHUP)
        DispatchQueue.global(qos: .userInitiated).async {
            let deadline = ProcessInfo.processInfo.systemUptime + 1
            while !Self.hasExited(pid), ProcessInfo.processInfo.systemUptime < deadline {
                Thread.sleep(forTimeInterval: 0.01)
            }
            if !Self.hasExited(pid) { Self.signalProcessGroup(pid, signal: SIGKILL) }
            while !Self.hasExited(pid) { Thread.sleep(forTimeInterval: 0.01) }
            completion()
        }
    }

    nonisolated private static func hasExited(_ pid: pid_t) -> Bool {
        var info = siginfo_t()
        // kill(pid, 0) 对僵尸进程也成功；WNOWAIT 把状态留给 SwiftTerm 的主队列监视。
        let result = waitid(P_PID, id_t(pid), &info, WEXITED | WNOHANG | WNOWAIT)
        return result == 0 ? info.si_pid == pid : errno == ECHILD
    }

    private func startIfPossible() {
        guard kind == .shell, nerdFonts != nil, state == .starting, let root, let view,
              !view.process.running, !view.process.windingDown else { return }
        let shell = ProcessInfo.processInfo.environment["SHELL"].flatMap { $0.isEmpty ? nil : $0 } ?? "/bin/zsh"
        var environment = ProcessInfo.processInfo.environment
        environment["TERM"] = "xterm-256color"
        environment["HORTUS_VAULT"] = root
        // 普通 shell 不是 Agent 会话，不能继承可能残留的会话标识。
        environment.removeValue(forKey: "HORTUS_AGENT_SESSION")
        // 从 Finder 或程序坞启动时没有 locale；缺省会把中文文件名和输出当成 ASCII。
        if ["LANG", "LC_ALL", "LC_CTYPE"].allSatisfy({ environment[$0]?.isEmpty ?? true }) {
            environment["LANG"] = Self.utf8Locale()
        }
        let variables = environment.map { "\($0.key)=\($0.value)" }.sorted()
        #if DEBUG
        inspectionFontReadyBeforeShell = view.glyphFallbackProvider === nerdFonts?.provider
        #endif
        state = .running
        view.startProcess(executable: shell, args: ["-l"], environment: variables, currentDirectory: root)
    }

    private func startPendingAgent() {
        guard state == .startingAgent, let pending = pendingAgent, let root, let view,
              !view.process.running, !view.process.windingDown else { return }
        var environment = ProcessInfo.processInfo.environment
        environment["TERM"] = "xterm-256color"
        environment["HORTUS_VAULT"] = root
        environment["HORTUS_AGENT_SESSION"] = pending.session
        if ["LANG", "LC_ALL", "LC_CTYPE"].allSatisfy({ environment[$0]?.isEmpty ?? true }) {
            environment["LANG"] = Self.utf8Locale()
        }
        let variables = environment.map { "\($0.key)=\($0.value)" }.sorted()
        state = .running
        pendingAgent = nil
        view.startProcess(executable: pending.executable, args: pending.args,
                          environment: variables, currentDirectory: root)
    }

    private func sendTerminationSignals(for view: HortusTerminalView) {
        let pid = view.process.shellPid
        guard pid > 0 else {
            view.terminate()
            return
        }
        killTask?.cancel()
        Self.signalProcessGroup(pid, signal: SIGHUP)
        killTask = Task { [weak self, weak view] in
            do { try await Task.sleep(for: .seconds(1)) } catch { return }
            guard let self, let view else { return }
            if view.process.running || view.process.windingDown {
                Self.signalProcessGroup(pid, signal: SIGKILL)
                view.terminate()
            }
            // SwiftUI 移除视图后 SwiftTerm 可能不再回调；停止等待必须显式完成。
            self.processTerminated(exitCode: nil)
        }
    }

    private func processTerminated(exitCode: Int32?) {
        killTask?.cancel()
        killTask = nil
        state = root == nil ? .empty : .exited(exitCode)
        let waiters = stopWaiters
        stopWaiters.removeAll()
        waiters.forEach { $0.resume() }
        onTerminated?(exitCode)
    }

    private func processFailed() {
        killTask?.cancel()
        killTask = nil
        state = .failed
        let waiters = stopWaiters
        stopWaiters.removeAll()
        waiters.forEach { $0.resume() }
        onFailedToStart?()
    }

    /// 第一个已安装、符合用户语言顺序的 UTF-8 locale；形式与 Terminal.app 相同，例如 zh_CN.UTF-8。
    nonisolated static func utf8Locale(preferred: [String] = Locale.preferredLanguages,
                                       region: String? = Locale.current.region?.identifier,
                                       installed: (String) -> Bool = {
                                           FileManager.default.fileExists(atPath: "/usr/share/locale/\($0)")
                                       }) -> String {
        for identifier in preferred {
            let language = Locale.Language(identifier: identifier)
            guard let code = language.languageCode?.identifier else { continue }
            let script = language.script?.identifier
            var regions = [language.region?.identifier, region].compactMap { $0 }
            if code == "zh" { regions.append(script == "Hant" ? "TW" : "CN") }
            for candidate in regions.map({ "\(code)_\($0).UTF-8" }) where installed(candidate) {
                return candidate
            }
        }
        return "en_US.UTF-8"
    }

    fileprivate func didTerminate(exitCode: Int32?) { processTerminated(exitCode: exitCode) }
    fileprivate func didFailToStart() { processFailed() }

    nonisolated private static func signalProcessGroup(_ pid: pid_t, signal: Int32) {
        // forkpty 在 macOS 上为子进程建立会话；先发进程组，避免 shell 启动的程序变成孤儿。
        _ = kill(-pid, signal)
        _ = kill(pid, signal)
    }
}

// Used only by inspections that run behind explicit check arguments.
extension TerminalSession {
    var inspectionShellPid: pid_t { view?.process.running == true ? view?.process.shellPid ?? 0 : 0 }
    var inspectionView: HortusTerminalView? { view }
    var inspectionViewAttached: Bool { view?.window != nil }
    func inspectionSend(_ text: String) {
        view?.send(Array(text.utf8))
    }
}

@MainActor
final class HortusTerminalView: LocalProcessTerminalView {
    let delegateProxy: DelegateProxy

    init(frame: CGRect = .zero, session: TerminalSession) {
        delegateProxy = DelegateProxy(session: session)
        super.init(frame: frame)
        processDelegate = delegateProxy
        applyNativeColors()
        caretViewTracksFocus = true
        cursorBlinkResetsOnInput = true
        suspendsRenderingWhenNotVisible = true
        translatesAutoresizingMaskIntoConstraints = false
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func applyAppearance(fontSize: Double) {
        if font.pointSize != CGFloat(fontSize) {
            font = NSFont.monospacedSystemFont(ofSize: CGFloat(fontSize), weight: .regular)
        }
        applyNativeColors()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyNativeColors()
    }

    private func applyNativeColors() {
        // SwiftTerm 在 AppKit 绘制之外换算前景色；用本视图的外观解析。
        // 背景必须是面板的真实颜色：SwiftTerm 用它回答 OSC 11、反显并从它派生 256 色。
        // `.clear` 是零 alpha 的黑色，曾让程序看到黑色终端。只保留 alpha 让玻璃透出。
        effectiveAppearance.performAsCurrentDrawingAppearance {
            nativeForegroundColor = NSColor.labelColor.usingColorSpace(.sRGB) ?? .labelColor
            nativeBackgroundColor = NSColor.textBackgroundColor.usingColorSpace(.sRGB) ?? .textBackgroundColor
        }
        backgroundOpacity = 0
    }

    final class DelegateProxy: NSObject, LocalProcessTerminalViewDelegate {
        weak var session: TerminalSession?

        init(session: TerminalSession) { self.session = session }

        func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) { }
        func setTerminalTitle(source: LocalProcessTerminalView, title: String) { }
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) { }
        func processTerminated(source: TerminalView, exitCode: Int32?) {
            session?.didTerminate(exitCode: exitCode)
        }
        func processFailedToStart(source: TerminalView, error: LocalProcessError) {
            session?.didFailToStart()
        }
    }
}

private struct TerminalRepresentable: NSViewRepresentable {
    let session: TerminalSession
    let fontSize: Double

    func makeNSView(context: Context) -> HortusTerminalView {
        return session.makeView(fontSize: fontSize)
    }

    func updateNSView(_ nsView: HortusTerminalView, context: Context) {
        session.update(fontSize: fontSize)
        session.attach(nsView, fontSize: fontSize)
    }

    static func dismantleNSView(_ nsView: HortusTerminalView, coordinator: ()) {
        // 会话由窗口控制器拥有并在库或窗口关闭时停止；重绘不能杀掉进程。
    }
}

struct TerminalPanel: View {
    @Environment(Workspace.self) private var workspace
    let tab: TerminalTab
    let fontSize: Double

    var body: some View {
        ZStack {
            TerminalRepresentable(session: tab.session, fontSize: fontSize)
                .id(tab.id)
                .clipShape(.rect(cornerRadius: 12, style: .continuous))
            statusOverlay
        }
        .padding(8)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(tab.title)
    }

    @ViewBuilder private var statusOverlay: some View {
        switch tab.kind {
        case .shell:
            if tab.session.showsStatusOverlay {
                VStack(spacing: 10) {
                    Text(tab.session.statusText).font(.callout).foregroundStyle(.secondary)
                    Button(L10n.string("Restart")) { tab.session.restart() }
                        .buttonStyle(.borderedProminent)
                }
                .padding(18)
                .glassEffect(.regular, in: .rect(cornerRadius: 14, style: .continuous))
            }
        case .agent:
            switch tab.agentPhase {
            case .preparing:
                VStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(L10n.string("Preparing snapshot…")).font(.callout).foregroundStyle(.secondary)
                }
                .padding(18)
                .glassEffect(.regular, in: .rect(cornerRadius: 14, style: .continuous))
            case .failed(let message):
                VStack(spacing: 8) {
                    Text(L10n.string("Agent failed to start")).font(.callout.weight(.medium))
                    Text(message).font(.caption).foregroundStyle(.secondary)
                        .multilineTextAlignment(.center).frame(maxWidth: 420)
                    Button(L10n.string("Restart Agent")) { workspace.terminals.restart(tab) }
                        .buttonStyle(.borderedProminent)
                }
                .padding(18)
                .glassEffect(.regular, in: .rect(cornerRadius: 14, style: .continuous))
            case .exited:
                VStack(spacing: 8) {
                    Text(L10n.string("Agent exited")).font(.callout).foregroundStyle(.secondary)
                    Button(L10n.string("Restart Agent")) { workspace.terminals.restart(tab) }
                        .buttonStyle(.borderedProminent)
                }
                .padding(18)
                .glassEffect(.regular, in: .rect(cornerRadius: 14, style: .continuous))
            case .idle, .running:
                EmptyView()
            }
        }
    }
}
