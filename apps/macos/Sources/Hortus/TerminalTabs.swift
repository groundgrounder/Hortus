import Foundation
import SwiftUI

enum TerminalTabKind: Equatable {
    case shell
    case agent
}

/// Agent 标签的启动阶段；进程本身仍由 `TerminalSession` 管理。
enum AgentPhase: Equatable {
    case idle
    /// 正在后台建立全库快照；不可 exec、不可审阅。
    case preparing
    case running
    case exited(Int32?)
    case failed(String)
}

@Observable @MainActor
final class TerminalTab: Identifiable {
    let id = UUID()
    let kind: TerminalTabKind
    let session: TerminalSession
    var title: String
    var agentPhase: AgentPhase = .idle
    var agentSessionID: String?
    /// 每次启动递增；迟到的快照回调据此失效。
    var generation = 0

    init(kind: TerminalTabKind, title: String) {
        self.kind = kind
        self.title = title
        session = TerminalSession(kind: kind == .agent ? .agent : .shell)
    }
}

/// 每个窗口唯一的终端标签控制器：拥有有序标签、选中项和全部会话的生命周期。
@Observable @MainActor
final class TerminalController {
    private(set) var tabs: [TerminalTab] = []
    var selectedID: UUID?
    weak var workspace: Workspace?
    private(set) var root: String?
    private var fontSize = 15.0
    private var shuttingDown = false
    #if DEBUG
    @ObservationIgnored var gateDelayForInspection = 0.0
    #endif

    var current: TerminalTab? {
        tabs.first { $0.id == selectedID } ?? tabs.last
    }

    var currentSession: TerminalSession? { current?.session }

    var shellTabs: [TerminalTab] { tabs.filter { $0.kind == .shell } }

    // MARK: 库与窗口生命周期

    /// 换库或关闭库：覆盖该窗口全部标签，其它窗口不受影响。
    func setRootAndWait(_ newRoot: String?) async {
        if root == newRoot, !tabs.isEmpty { return }
        await stopAll()
        root = newRoot
        tabs.removeAll()
        selectedID = nil
        if let newRoot { addShellTab(root: newRoot) }
    }

    /// 新窗口在第一次关闭可能插入前建立唯一的普通 shell 标签。
    func setInitialRoot(_ newRoot: String) {
        guard root == nil else { return }
        root = newRoot
        if tabs.isEmpty { addShellTab(root: newRoot) }
    }

    func update(fontSize: Double) {
        self.fontSize = fontSize
        for tab in tabs { tab.session.update(fontSize: fontSize) }
    }

    /// 退出流程不等待快照；只回收已有进程组。
    func stopForShutdown(completion: @escaping @Sendable () -> Void) {
        shuttingDown = true
        for tab in tabs {
            tab.generation += 1
            // 快照完成但还没 exec 的会话在退出时立即作废；不让迟到的回调启动 Agent。
            if tab.kind == .agent, neverStarted(tab) {
                markNow(tab.agentSessionID, "cancelled")
            }
        }
        let group = DispatchGroup()
        for tab in tabs {
            group.enter()
            tab.session.stopForShutdown { group.leave() }
        }
        if tabs.isEmpty { completion(); return }
        group.notify(queue: .global(qos: .userInitiated)) { @Sendable in completion() }
    }

    private func stopAll() async {
        let previous = tabs
        tabs.removeAll()
        selectedID = nil
        for tab in previous {
            tab.generation += 1
            if tab.kind == .agent, neverStarted(tab) {
                mark(tab.agentSessionID, "cancelled", nil)
            }
            await tab.session.stopAndWait()
        }
    }

    // MARK: 标签操作

    @discardableResult
    func newShellTab() -> TerminalTab? {
        guard let root else { return nil }
        let tab = addShellTab(root: root)
        selectedID = tab.id
        focusCurrentSoon()
        return tab
    }

    @discardableResult
    private func addShellTab(root: String) -> TerminalTab {
        let tab = TerminalTab(kind: .shell, title: nextTitle(for: .shell))
        configure(tab)
        tab.session.setInitialRoot(root)
        tabs.append(tab)
        if selectedID == nil { selectedID = tab.id }
        return tab
    }

    @discardableResult
    func newAgentTab() -> TerminalTab? {
        guard let root else { return nil }
        let tab = TerminalTab(kind: .agent, title: nextTitle(for: .agent))
        configure(tab)
        tab.session.setInitialRoot(root)
        tabs.append(tab)
        selectedID = tab.id
        launchAgent(tab)
        focusCurrentSoon()
        return tab
    }

    func select(_ tab: TerminalTab) {
        guard tabs.contains(where: { $0 === tab }) else { return }
        selectedID = tab.id
        focusCurrentSoon()
    }

    func close(_ tab: TerminalTab) async {
        guard let index = tabs.firstIndex(where: { $0 === tab }) else { return }
        tab.generation += 1
        if tab.kind == .agent, neverStarted(tab) {
            mark(tab.agentSessionID, "cancelled", nil)
        }
        tabs.remove(at: index)
        if selectedID == tab.id {
            selectedID = tabs.isEmpty ? nil : tabs[min(index, tabs.count - 1)].id
        }
        await tab.session.stopAndWait()
        focusCurrentSoon()
    }

    /// Agent 重新启动总是新会话 ID 与新快照，不复用旧基线。
    func restart(_ tab: TerminalTab) {
        guard tab.kind == .agent, tabs.contains(where: { $0 === tab }) else { return }
        tab.generation += 1
        tab.agentPhase = .preparing
        tab.agentSessionID = nil
        Task { [weak self, weak tab] in
            guard let tab else { return }
            await tab.session.stopAndWait()
            guard let self, self.tabs.contains(where: { $0 === tab }), !self.shuttingDown else { return }
            self.launchAgent(tab)
        }
    }

    func focus() {
        guard let view = currentSession?.inspectionView, let window = view.window else {
            return
        }
        window.makeFirstResponder(view)
        UndoMenuState.scheduleRefresh()
    }

    func clearScreen() {
        currentSession?.clearScreen()
    }

    // MARK: Agent 启动闸门

    /// 固定顺序：模板与可执行文件 → 后台完整快照并发布元数据 → 复核标签/库根/代次 → exec。
    private func launchAgent(_ tab: TerminalTab) {
        guard let workspace, let core = workspace.core, let root = workspace.root else {
            tab.agentPhase = .failed(L10n.string("Open a knowledge base to use the terminal"))
            return
        }
        tab.generation += 1
        let generation = tab.generation
        tab.agentPhase = .preparing
        tab.agentSessionID = nil
        let session = tab.session
        Task { [weak self, weak tab] in
            do {
                let spec = try await Task.detached { try core.agentSpec() }.value
                guard let self, let tab, self.valid(tab, generation, root: root) else { return }
                let record = try await Task.detached {
                    try core.beginAgentSession(vault: root)
                }.value
                if !self.valid(tab, generation, root: root) {
                    // 迟到的快照不能启动 Agent；快照本身保留，不自动删除。
                    self.mark(record.id, "cancelled", nil)
                    return
                }
                tab.agentSessionID = record.id
                #if DEBUG
                if self.gateDelayForInspection > 0 {
                    try? await Task.sleep(for: .seconds(self.gateDelayForInspection))
                }
                #endif
                if !self.valid(tab, generation, root: root) {
                    self.mark(record.id, "cancelled", nil)
                    return
                }
                guard session.startAgent(executable: spec.executable, args: spec.args, session: record.id) else {
                    tab.agentPhase = .failed(L10n.string("Unable to start the shell"))
                    self.mark(record.id, "failed", nil)
                    return
                }
                tab.agentPhase = .running
                self.mark(record.id, "running", nil)
            } catch {
                guard let self, let tab, self.valid(tab, generation, root: root) else { return }
                tab.agentPhase = .failed(L10n.error(error))
            }
        }
    }

    /// 进程从未启动（模板/快照/视图挂载阶段被取消）才写 `cancelled`；
    /// 已经退出或失败的结果不能被退出流程覆盖。
    private func neverStarted(_ tab: TerminalTab) -> Bool {
        switch tab.session.state {
        case .empty, .starting, .startingAgent: true
        default: false
        }
    }

    private func valid(_ tab: TerminalTab, _ generation: Int, root: String) -> Bool {
        !shuttingDown
            && tabs.contains(where: { $0 === tab })
            && tab.generation == generation
            && workspace?.root == root
    }

    /// 退出路径同步落一次终态；只写一个小 JSON，不能等待后台任务。
    private func markNow(_ id: String?, _ state: String) {
        guard let id, let core = workspace?.core else { return }
        try? core.markAgentSession(id: id, state: state, exitCode: nil)
    }

    private func mark(_ id: String?, _ state: String, _ exitCode: Int32?) {
        guard let id, let core = workspace?.core else { return }
        Task.detached {
            try? core.markAgentSession(id: id, state: state, exitCode: exitCode)
        }
    }

    private func configure(_ tab: TerminalTab) {
        tab.session.update(fontSize: fontSize)
        tab.session.onTerminated = { [weak self, weak tab] exitCode in
            guard let self, let tab, tab.kind == .agent else { return }
            tab.agentPhase = .exited(exitCode)
            self.mark(tab.agentSessionID, "exited", exitCode)
        }
        tab.session.onFailedToStart = { [weak self, weak tab] in
            guard let self, let tab, tab.kind == .agent else { return }
            tab.agentPhase = .failed(L10n.string("Unable to start the shell"))
            self.mark(tab.agentSessionID, "failed", nil)
        }
    }

    private func focusCurrentSoon() {
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(20))
            self?.focus()
        }
    }

    private func nextTitle(for kind: TerminalTabKind) -> String {
        let count = tabs.filter { $0.kind == kind }.count
        let base = kind == .shell ? L10n.string("Shell") : L10n.string("Agent")
        return count == 0 ? base : "\(base) \(count + 1)"
    }
}

#if DEBUG
extension TerminalController {
    /// 自检读取真实标签状态；生产可执行文件不包含这些字段。
    func inspectionTabStates() -> [[String: Any]] {
        tabs.enumerated().map { index, tab in
            var state: [String: Any] = [
                "index": index,
                "kind": tab.kind == .shell ? "shell" : "agent",
                "title": tab.title,
                "selected": tab.id == selectedID,
                "pid": Int(tab.session.inspectionShellPid),
                "session_state": String(describing: tab.session.state),
                "generation": tab.generation,
            ]
            switch tab.agentPhase {
            case .idle: state["agent_phase"] = "idle"
            case .preparing: state["agent_phase"] = "preparing"
            case .running: state["agent_phase"] = "running"
            case .exited(let code): state["agent_phase"] = "exited"; state["exit_code"] = code.map(Int.init) ?? -1
            case .failed(let message): state["agent_phase"] = "failed"; state["message"] = message
            }
            if let id = tab.agentSessionID { state["agent_session"] = id }
            return state
        }
    }

    func inspectionSetGateDelay(_ seconds: Double) {
        gateDelayForInspection = seconds
    }
}
#endif
