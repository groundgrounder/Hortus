import Foundation

#if DEBUG
import AppKit
import Darwin
import Foundation

/// Agent 启动、快照闭锁、多标签与恢复的真实窗口自检。
/// 只在 Debug 构建里存在；Release 可执行文件不包含这些参数。
@MainActor
enum AgentInspection {
    /// 另一窗口写入检查使用的完整布局与会话 JSON。
    private nonisolated static let persistedLayoutJSON = """
    {"regions":{"left":{"groups":[{"id":"1","tabs":["files","search","tags"],"selected":"files"}],"size":260.0},"right":{"groups":[{"id":"2","tabs":["outline","backlinks"],"selected":"outline"}],"size":250.0},"bottom":{"groups":[{"id":"3","tabs":["terminal","changes"],"selected":"terminal"}],"size":230.0}}}
    """
    private nonisolated static let emptySessionJSON = """
    {"diagrams":[],"last_vault":null,"recent":[],"windows":[],"default_visible":{}}
    """
    private static func argument(_ name: String, in arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }.map { String($0.dropFirst(name.count + 3)) }
    }

    private static func wait(seconds: Double, until description: String, _ condition: () -> Bool) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else {
                throw NSError(domain: "Hortus.AgentInspection", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "等待超时（\(seconds) s）：\(description)"])
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    private static func screen(_ tab: TerminalTab) -> String {
        guard let view = tab.session.inspectionView else { return "" }
        return view.visibleRowsText(0..<view.terminalDimensions.rows).joined(separator: "\n")
    }

    /// 跨区后终端宽度变化会重排尾随空格；比较实际文字而不是列宽。
    private static func normalizedText(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.replacingOccurrences(of: " +$", with: "", options: .regularExpression) }
            .joined(separator: "\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func normalizedScreen(_ tab: TerminalTab) -> String { normalizedText(screen(tab)) }

    /// 去掉所有空白后比较：跨区重排只改变折行，不改变终端里已有的文字。
    private static func collapsedText(_ text: String) -> String {
        text.filter { !$0.isWhitespace }
    }

    private static func collapsedScreen(_ tab: TerminalTab) -> String { collapsedText(screen(tab)) }

    private static func waitShell(_ tab: TerminalTab, seconds: Double = 20) async throws {
        try await wait(seconds: seconds, until: "shell \(tab.title) 启动") {
            tab.session.state == .running && tab.session.inspectionShellPid > 0 && tab.session.inspectionViewAttached
        }
    }

    private static func currentDirectory(_ pid: pid_t) -> String? {
        var info = proc_vnodepathinfo()
        let size = Int32(MemoryLayout<proc_vnodepathinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &info, size) == size else { return nil }
        return withUnsafeBytes(of: info.pvi_cdir.vip_path) {
            String(cString: $0.bindMemory(to: CChar.self).baseAddress!)
        }
    }

    #if DEBUG
    private static func dragTerminal(_ tab: TerminalTab, to region: Region, layout: Layout, window: NSWindow) async throws -> Bool {
        guard let group = layout.availableGroups(in: region).first else { return false }
        return try await ToolDragInspection.drag(
            from: "panel-tab-terminal-\(tab.id.uuidString)",
            to: "group-tabs-\(group.id.uuidString)", window: window)
    }
    #endif

    private static func alive(_ pid: pid_t) -> Bool { pid > 0 && kill(pid, 0) == 0 }

    private static func scratch(_ directory: String?) -> [String: Any] {
        guard let directory else { return [:] }
        let url = URL(fileURLWithPath: directory)
        var result: [String: Any] = [:]
        let names = (try? FileManager.default.contentsOfDirectory(atPath: directory)) ?? []
        for name in names.sorted() {
            if let value = try? String(contentsOf: url.appendingPathComponent(name), encoding: .utf8) {
                result[name] = value
            }
        }
        return result
    }

    private static func sessions(_ workspace: Workspace, all: Bool = false) -> [[String: Any]] {
        guard let core = workspace.core else { return [] }
        let records = (try? core.listAgentSessions(vault: all ? nil : workspace.root)) ?? []
        return records.map { record in
            [
                "id": record.id,
                "vault": record.vault,
                "state": record.state,
                "snapshot_path": record.snapshotPath,
                "created_at_ms": record.createdAtMs,
                "started_at_ms": record.startedAtMs ?? -1,
                "ended_at_ms": record.endedAtMs ?? -1,
                "entries": record.entries,
                "bytes": record.bytes,
                "method": record.method,
                "error": record.error ?? "",
            ]
        }
    }

    private static func tabStates(_ controller: TerminalController) -> [[String: Any]] {
        controller.inspectionTabStates()
    }

    private static func send(_ tab: TerminalTab, _ text: String) {
        tab.session.inspectionSend(text + "\r")
    }

    /// 假 Agent 的写入相对 exec 是异步的；读到 argv 之前不能判定证据缺失。
    private static func waitScratch(_ directory: String?, seconds: Double = 10) async throws {
        guard let directory else { return }
        let marker = URL(fileURLWithPath: directory).appendingPathComponent("argc")
        try await wait(seconds: seconds, until: "假 Agent 写入 argv") {
            FileManager.default.fileExists(atPath: marker.path)
        }
    }

    private static func ensureShell(_ controller: TerminalController) async throws -> TerminalTab {
        if controller.shellTabs.isEmpty { controller.newShellTab() }
        guard let tab = controller.shellTabs.first else { throw CocoaError(.coderValueNotFound) }
        try await waitShell(tab)
        return tab
    }

    private static func ensureAgent(_ controller: TerminalController) throws -> TerminalTab {
        guard let tab = controller.newAgentTab() else { throw CocoaError(.coderValueNotFound) }
        return tab
    }

    /// 外部检查用来判断时机的标记目录；应用只写标记，不阻塞快照。
    private static func markerDirectory(_ arguments: [String]) throws -> URL {
        guard let path = argument("agent-marker", in: arguments) else {
            throw CocoaError(.coderValueNotFound)
        }
        return URL(fileURLWithPath: path)
    }

    private static func writeMarker(_ marker: URL, _ name: String) {
        _ = FileManager.default.createFile(
            atPath: marker.appendingPathComponent(name).path, contents: Data())
    }

    /// 快照记录在扫描前先写 `preparing`；检查据此在快照进行中操作。
    private static func waitPreparing(_ workspace: Workspace, seconds: Double = 120) async throws {
        let directory = URL(fileURLWithPath: workspace.core?.dataDirectory() ?? "")
            .appendingPathComponent("agent-sessions")
        try await wait(seconds: seconds, until: "出现 preparing 会话") {
            let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
            for name in names {
                let metadata = directory.appendingPathComponent(name).appendingPathComponent("session.json")
                if let text = try? String(contentsOf: metadata, encoding: .utf8),
                   text.contains("\"preparing\"") {
                    return true
                }
            }
            return false
        }
    }

    private static func waitSessionState(
        _ workspace: Workspace, _ state: String, seconds: Double
    ) async throws -> [[String: Any]] {
        var latest: [[String: Any]] = []
        try await wait(seconds: seconds, until: "会话进入 \(state)") {
            latest = sessions(workspace)
            return latest.contains { $0["state"] as? String == state }
        }
        return latest
    }

    static func run(workspace: Workspace, layout: Layout, arguments: [String]) async throws -> [String: Any] {
        let mode = argument("agent-mode", in: arguments) ?? "launch"
        let scratchDirectory = argument("agent-scratch", in: arguments)
        let controller = workspace.terminals
        workspace.showTerminal()
        var result: [String: Any] = ["mode": mode, "root": workspace.root ?? ""]
        if let delay = argument("agent-gate-delay", in: arguments).flatMap(Double.init) {
            controller.inspectionSetGateDelay(delay)
        }
        if mode.hasPrefix("late-") || mode == "preparing" {
            controller.inspectionSetGateDelay(
                argument("agent-gate-delay", in: arguments).flatMap(Double.init) ?? 30)
        }

        switch mode {
        case "history":
            result["app_start_recovered"] = workspace.application?.recoveredSessions ?? 0
            result["recovered"] = (try? workspace.core?.recoverAgentSessions()) ?? 0
            result["sessions"] = sessions(workspace, all: true)
            result["tabs"] = tabStates(controller)

        case "launch", "fail", "preparing", "exit":
            let firstShell = try await ensureShell(controller)
            send(firstShell, "printf 'HORTUS_SHELL_READY\\n'")
            try await wait(seconds: 10, until: "普通 shell 响应") {
                screen(firstShell).contains("HORTUS_SHELL_READY")
            }
            let tab = try ensureAgent(controller)
            if mode == "launch" {
                send(firstShell, "printf 'HORTUS_AFTER_PREPARE\\n'")
            }
            let gap: GapRecorder? = mode == "preparing" || mode == "launch" ? GapRecorder() : nil
            gap?.start()
            if mode == "preparing" {
                try await wait(seconds: 60, until: "快照完成并等待 exec") {
                    tab.agentSessionID != nil && tab.agentPhase == .preparing
                }
            } else {
                try await wait(seconds: 90, until: "Agent 进入运行、退出或失败") {
                    switch tab.agentPhase {
                    case .running: tab.session.inspectionShellPid > 0
                    case .exited, .failed: true
                    case .idle, .preparing: false
                    }
                }
            }
            gap?.stop()
            if mode == "exit" {
                try await wait(seconds: 60, until: "Agent 进程退出") {
                    if case .exited = tab.agentPhase { return true }
                    return false
                }
            }
            if case .running = tab.agentPhase { try await waitScratch(scratchDirectory) }
            if mode == "launch" {
                // 快照期间主线程仍处理 shell 的 PTY 输入与输出。
                try await wait(seconds: 10, until: "准备期间 shell 仍可执行") {
                    screen(firstShell).contains("HORTUS_AFTER_PREPARE")
                }
                result["max_main_gap_ms"] = gap?.maxGapMilliseconds ?? 0
            }
            result["tabs"] = tabStates(controller)
            result["sessions"] = sessions(workspace)
            result["scratch"] = scratch(scratchDirectory)
            result["shell_alive"] = alive(firstShell.session.inspectionShellPid)
            result["agent_tab"] = tab.id.uuidString
            if case .failed(let message) = tab.agentPhase { result["failure"] = message }
            if case .exited(let code) = tab.agentPhase { result["exit_code"] = code.map { Int($0) } ?? -1 }
            if mode == "preparing" {
                result["snapshot_ready_before_exec"] = tab.agentSessionID != nil
                if let window = workspace.application?.records.first(where: { $0.workspace === workspace })?.window {
                    let generation = tab.generation, session = tab.agentSessionID
                    let moved = try await dragTerminal(tab, to: .right, layout: layout, window: window)
                    let right = layout.region(containing: .terminal(tab.id)) == .right
                        && tab.generation == generation && tab.agentSessionID == session
                        && tab.agentPhase == .preparing
                    _ = try await dragTerminal(tab, to: .bottom, layout: layout, window: window)
                    result["preparing_drag_valid"] = moved && right && tab.generation == generation
                        && tab.agentSessionID == session && tab.agentPhase == .preparing
                }
            }

        case "tabs":
            let first = try await ensureShell(controller)
            guard let second = controller.newShellTab() else { throw CocoaError(.coderValueNotFound) }
            try await waitShell(second)
            send(first, "printf 'HORTUS_SESSION_ENV=%s\\n' \"${HORTUS_AGENT_SESSION-unset}\"")
            try await wait(seconds: 10, until: "普通 shell 不继承 Agent 会话标识") {
                screen(first).contains("HORTUS_SESSION_ENV=unset")
            }
            send(first, "printf 'HORTUS_TAB_ONE\\n'")
            send(second, "printf 'HORTUS_TAB_TWO\\n'")
            try await wait(seconds: 10, until: "两个 shell 输出各自标记") {
                screen(first).contains("HORTUS_TAB_ONE") && screen(second).contains("HORTUS_TAB_TWO")
            }
            let firstPid = first.session.inspectionShellPid
            let secondPid = second.session.inspectionShellPid
            let firstBuffer = screen(first)
            let secondBuffer = screen(second)
            result["shell_pids"] = [Int(firstPid), Int(secondPid)]
            result["shell_cwds"] = [currentDirectory(firstPid) ?? "", currentDirectory(secondPid) ?? ""]
            result["buffers_distinct"] = firstBuffer != secondBuffer
                && firstBuffer.contains("HORTUS_TAB_ONE") && !firstBuffer.contains("HORTUS_TAB_TWO")

            let gap = GapRecorder()
            gap.start()
            guard let agentTab = controller.newAgentTab() else { throw CocoaError(.coderValueNotFound) }
            try await wait(seconds: 90, until: "Agent 运行") {
                agentTab.agentPhase == .running && agentTab.session.inspectionShellPid > 0
            }
            try await waitScratch(scratchDirectory)
            gap.stop()
            result["max_main_gap_ms"] = gap.maxGapMilliseconds

            #if DEBUG
            // 真实拖动：Agent 和 Shell 跨区后 PID、cwd、scrollback 与 Agent 会话不变。
            if let window = workspace.application?.records.first(where: { $0.workspace === workspace })?.window {
                let agentPidBefore = agentTab.session.inspectionShellPid
                let agentSessionBefore = agentTab.agentSessionID
                let agentCwdBefore = currentDirectory(agentPidBefore) ?? ""
                let agentBufferBefore = collapsedScreen(agentTab)
                let agentViewBefore = agentTab.session.inspectionView
                let agentToRight = try await dragTerminal(agentTab, to: .right, layout: layout, window: window)
                try await wait(seconds: 3, until: "Agent 拖到右侧") {
                    layout.region(containing: .terminal(agentTab.id)) == .right
                }
                let agentRegionRight = layout.region(containing: .terminal(agentTab.id)) == .right
                let agentPidRight = agentTab.session.inspectionShellPid == agentPidBefore
                let agentSessionRight = agentTab.agentSessionID == agentSessionBefore
                let agentCwdRight = (currentDirectory(agentPidBefore) ?? "") == agentCwdBefore
                let agentBufferRight = collapsedScreen(agentTab) == agentBufferBefore
                let agentRightOK = agentRegionRight && controller.tab(agentTab.id) === agentTab
                    && agentPidRight && agentSessionRight && agentCwdRight && agentBufferRight
                    && agentTab.session.inspectionView === agentViewBefore
                let agentToBottom = try await dragTerminal(agentTab, to: .bottom, layout: layout, window: window)
                try await wait(seconds: 3, until: "Agent 拖回底部") {
                    layout.region(containing: .terminal(agentTab.id)) == .bottom
                }
                let agentRegionBottom = layout.region(containing: .terminal(agentTab.id)) == .bottom
                let agentPidBottom = agentTab.session.inspectionShellPid == agentPidBefore
                let agentSessionBottom = agentTab.agentSessionID == agentSessionBefore
                let agentBufferBottom = collapsedScreen(agentTab) == agentBufferBefore
                let agentBottomOK = agentRegionBottom && agentPidBottom && agentSessionBottom && agentBufferBottom
                let shellCwdBefore = currentDirectory(firstPid) ?? ""
                let shellViewBefore = first.session.inspectionView
                let shellToLeft = try await dragTerminal(first, to: .left, layout: layout, window: window)
                let shellLeftOK = layout.region(containing: .terminal(first.id)) == .left
                    && first.session.inspectionShellPid == firstPid
                    && (currentDirectory(firstPid) ?? "") == shellCwdBefore
                    && screen(first).contains("HORTUS_TAB_ONE")
                    && first.session.inspectionView === shellViewBefore
                let shellToBottom = try await dragTerminal(first, to: .bottom, layout: layout, window: window)
                let shellBottomOK = layout.region(containing: .terminal(first.id)) == .bottom
                    && first.session.inspectionShellPid == firstPid
                    && screen(first).contains("HORTUS_TAB_ONE")
                let cancelled = try await ToolDragInspection.cancel(
                    from: "panel-tab-terminal-\(first.id.uuidString)", window: window)
                let cancelOK = layout.region(containing: .terminal(first.id)) == .bottom
                    && layout.dragging == nil
                    && first.session.inspectionShellPid == firstPid
                result["real_drag"] = [
                    "agent_to_right": agentToRight && agentRightOK,
                    "agent_to_bottom": agentToBottom && agentBottomOK,
                    "shell_to_left": shellToLeft && shellLeftOK,
                    "shell_to_bottom": shellToBottom && shellBottomOK,
                    "cancel": cancelled && cancelOK,
                ]
                result["real_drag_detail"] = [
                    "agent_event": agentToRight,
                    "agent_region_right": agentRegionRight,
                    "agent_pid_right": agentPidRight,
                    "agent_session_right": agentSessionRight,
                    "agent_cwd_right": agentCwdRight,
                    "agent_buffer_right": agentBufferRight,
                    "agent_bottom": agentBottomOK,
                    "agent_region_bottom": agentRegionBottom,
                    "agent_pid_bottom": agentPidBottom,
                    "agent_session_bottom": agentSessionBottom,
                    "agent_buffer_bottom": agentBufferBottom,
                    "shell_left_event": shellToLeft,
                    "shell_left_ok": shellLeftOK,
                    "cancel_event": cancelled,
                    "cancel_dragging": layout.dragging == nil,
                    "cancel_pid_same": first.session.inspectionShellPid == firstPid,
                ]
            }
            #endif

            // 切换标签不重启进程、不清空 scrollback。
            controller.select(first)
            try await Task.sleep(for: .milliseconds(150))
            controller.select(second)
            try await Task.sleep(for: .milliseconds(150))
            controller.select(agentTab)
            try await Task.sleep(for: .milliseconds(150))
            result["switch_pids"] = [Int(first.session.inspectionShellPid), Int(second.session.inspectionShellPid),
                                     Int(agentTab.session.inspectionShellPid)]
            result["switch_markers"] = screen(first).contains("HORTUS_TAB_ONE")
                && screen(second).contains("HORTUS_TAB_TWO")
                && screen(first).count >= firstBuffer.count * 8 / 10
                && screen(second).count >= secondBuffer.count * 8 / 10

            // 停靠移动只复用视图，不重启进程。
            layout.moveToRegion(.terminal(first.id), .right)
            try await Task.sleep(for: .milliseconds(300))
            layout.moveToRegion(.terminal(first.id), .bottom)
            try await Task.sleep(for: .milliseconds(300))
            result["dock_pids"] = [Int(first.session.inspectionShellPid), Int(second.session.inspectionShellPid),
                                   Int(agentTab.session.inspectionShellPid)]
            result["dock_markers"] = screen(first).contains("HORTUS_TAB_ONE")
                && screen(second).contains("HORTUS_TAB_TWO")
                && screen(first).count >= firstBuffer.count * 8 / 10
                && screen(second).count >= secondBuffer.count * 8 / 10

            // 焦点与清屏只作用于当前标签。
            controller.select(first)
            try await wait(seconds: 3, until: "当前终端切到第一个 Shell") { controller.current === first }
            controller.focus()
            try await wait(seconds: 3, until: "第一个 Shell 获得焦点") {
                first.session.inspectionView?.window?.firstResponder === first.session.inspectionView
            }
            let focusFirst = first.session.inspectionView?.window?.firstResponder === first.session.inspectionView
            controller.select(second)
            try await wait(seconds: 3, until: "当前终端切到第二个 Shell") { controller.current === second }
            controller.focus()
            try await wait(seconds: 3, until: "第二个 Shell 获得焦点") {
                second.session.inspectionView?.window?.firstResponder === second.session.inspectionView
            }
            let focusSecond = second.session.inspectionView?.window?.firstResponder === second.session.inspectionView
            result["focus"] = [focusFirst, focusSecond,
                               first.session.inspectionView?.window != nil,
                               second.session.inspectionView?.window != nil]
            controller.clearScreen()
            try await wait(seconds: 3, until: "清屏移除第二个 Shell 的标记") {
                !screen(second).contains("HORTUS_TAB_TWO")
            }
            result["focus_isolated"] = focusFirst && focusSecond
            result["focus_detail"] = [
                "first_view_attached": first.session.inspectionView?.window != nil,
                "second_view_attached": second.session.inspectionView?.window != nil,
                "current_is_first": controller.current === first,
                "first_is_first_responder": first.session.inspectionView?.window?.firstResponder === first.session.inspectionView,
                "second_is_first_responder": second.session.inspectionView?.window?.firstResponder === second.session.inspectionView,
            ]
            result["clear_isolated"] = !screen(second).contains("HORTUS_TAB_TWO")
                && screen(first).contains("HORTUS_SESSION_ENV=unset")
                && screen(first).contains("HORTUS_TAB_ONE")
                && first.session.inspectionShellPid == firstPid

            // 关闭一个标签只终止它自己的进程组。
            let agentPid = agentTab.session.inspectionShellPid
            await controller.close(second)
            try await Task.sleep(for: .milliseconds(300))
            result["closed_tab_pid"] = Int(secondPid)
            result["closed_dead"] = !alive(secondPid)
            result["other_shell_alive"] = alive(firstPid) && first.session.inspectionShellPid == firstPid
            result["agent_alive"] = alive(agentPid) && agentTab.session.inspectionShellPid == agentPid
            result["tabs"] = tabStates(controller)
            result["sessions"] = sessions(workspace)
            result["scratch"] = scratch(scratchDirectory)
            result["shell_cwds_after"] = [currentDirectory(firstPid) ?? ""]
            result["buffers_after_close"] = screen(first).contains("HORTUS_TAB_ONE")
            // 截图停在仍有输出的 shell 标签，而不是安静的 Agent。
            controller.select(first)
            try await Task.sleep(for: .milliseconds(150))

        case "restart":
            _ = try await ensureShell(controller)
            let tab = try ensureAgent(controller)
            try await wait(seconds: 90, until: "第一次 Agent 结束") {
                if case .exited = tab.agentPhase { return true }
                return false
            }
            try await waitScratch(scratchDirectory)
            let firstID = tab.agentSessionID ?? ""
            result["first_session"] = firstID
            controller.restart(tab)
            try await wait(seconds: 90, until: "新会话 ID 与新快照") {
                guard let id = tab.agentSessionID, id != firstID else { return false }
                switch tab.agentPhase {
                case .running, .exited: return true
                case .idle, .preparing, .failed: return false
                }
            }
            result["second_session"] = tab.agentSessionID ?? ""
            try await Task.sleep(for: .milliseconds(300))
            result["tabs"] = tabStates(controller)
            result["sessions"] = sessions(workspace)
            result["scratch"] = scratch(scratchDirectory)

        case "two-windows":
            guard let secondVault = argument("agent-second-vault", in: arguments),
                  let application = workspace.application,
                  let firstRecord = application.records.first(where: { $0.workspace === workspace })
            else { throw CocoaError(.coderValueNotFound) }
            let firstTab = try await ensureShell(controller)
            guard let firstExtra = controller.newShellTab() else { throw CocoaError(.coderValueNotFound) }
            try await waitShell(firstExtra)
            await workspace.openFolder(secondVault)
            guard let secondRecord = application.records.first(where: { $0.workspace !== workspace && $0.workspace.root != nil })
            else { throw CocoaError(.coderValueNotFound) }
            let secondWorkspace = secondRecord.workspace
            secondWorkspace.showTerminal()
            application.activate(secondRecord)
            try await Task.sleep(for: .milliseconds(300))
            let secondFirst = try await ensureShell(secondWorkspace.terminals)
            guard let secondExtra = secondWorkspace.terminals.newShellTab() else { throw CocoaError(.coderValueNotFound) }
            try await waitShell(secondExtra)
            let tabs = [firstTab, firstExtra, secondFirst, secondExtra]
            let pids = tabs.map(\.session.inspectionShellPid)
            result["pids"] = pids.map(Int.init)
            result["cwds"] = pids.map { currentDirectory($0) ?? "" }
            result["roots"] = [workspace.root ?? "", secondWorkspace.root ?? ""]
            result["distinct_pids"] = Set(pids).count == 4
            application.activate(firstRecord)
            try await Task.sleep(for: .milliseconds(200))
            await controller.close(firstExtra)
            try await Task.sleep(for: .milliseconds(300))
            result["closed_pid"] = Int(pids[1])
            result["closed_dead"] = !alive(pids[1])
            result["remaining_alive"] = [alive(pids[0]), alive(pids[2]), alive(pids[3])]
            result["tabs"] = tabStates(controller)

        case "late-close-tab":
            let first = try await ensureShell(controller)
            let tab = try ensureAgent(controller)
            try await wait(seconds: 90, until: "快照完成") { tab.agentSessionID != nil }
            result["session_before"] = tab.agentSessionID ?? ""
            await controller.close(tab)
            try await Task.sleep(for: .milliseconds(1500))
            result["sessions"] = sessions(workspace)
            result["tabs"] = tabStates(controller)
            result["scratch"] = scratch(scratchDirectory)
            result["shell_alive"] = alive(first.session.inspectionShellPid)

        case "late-vault":
            let tab = try ensureAgent(controller)
            try await wait(seconds: 90, until: "快照完成") { tab.agentSessionID != nil }
            result["session_before"] = tab.agentSessionID ?? ""
            await workspace.releaseDocuments()
            try await Task.sleep(for: .milliseconds(1500))
            result["sessions"] = sessions(workspace)
            result["tabs"] = tabStates(controller)
            result["scratch"] = scratch(scratchDirectory)

        case "late-window":
            guard let secondVault = argument("agent-second-vault", in: arguments) else {
                throw CocoaError(.coderValueNotFound)
            }
            let tab = try ensureAgent(controller)
            try await wait(seconds: 90, until: "快照完成") { tab.agentSessionID != nil }
            result["session_before"] = tab.agentSessionID ?? ""
            await workspace.openFolder(secondVault)
            try await Task.sleep(for: .milliseconds(500))
            guard let record = workspace.application?.records.first(where: { $0.workspace === workspace }) else {
                throw CocoaError(.coderValueNotFound)
            }
            record.window?.performClose(nil)
            try await Task.sleep(for: .milliseconds(1500))
            result["sessions"] = sessions(workspace)
            result["scratch"] = scratch(scratchDirectory)

        case "late-quit":
            let tab = try ensureAgent(controller)
            try await wait(seconds: 90, until: "快照完成") { tab.agentSessionID != nil }
            result["session_before"] = tab.agentSessionID ?? ""
            result["sessions"] = sessions(workspace)
            result["scratch"] = scratch(scratchDirectory)

        case "during-close-tab":
            let first = try await ensureShell(controller)
            let tab = try ensureAgent(controller)
            try await waitPreparing(workspace)
            await controller.close(tab)
            result["sessions"] = try await waitSessionState(workspace, "cancelled", seconds: 30)
            result["tabs"] = tabStates(controller)
            result["scratch"] = scratch(scratchDirectory)
            result["shell_alive"] = alive(first.session.inspectionShellPid)

        case "during-vault":
            _ = try await ensureShell(controller)
            _ = try ensureAgent(controller)
            try await waitPreparing(workspace)
            await workspace.releaseDocuments()
            result["sessions"] = try await waitSessionState(workspace, "cancelled", seconds: 30)
            result["scratch"] = scratch(scratchDirectory)

        case "during-window":
            guard let secondVault = argument("agent-second-vault", in: arguments),
                  let application = workspace.application else {
                throw CocoaError(.coderValueNotFound)
            }
            _ = try await ensureShell(controller)
            _ = try ensureAgent(controller)
            try await waitPreparing(workspace)
            await workspace.openFolder(secondVault)
            try await Task.sleep(for: .milliseconds(300))
            guard let record = application.records.first(where: { $0.workspace === workspace }) else {
                throw CocoaError(.coderValueNotFound)
            }
            let started = ProcessInfo.processInfo.systemUptime
            record.window?.performClose(nil)
            try await Task.sleep(for: .milliseconds(300))
            result["window_close_ms"] = (ProcessInfo.processInfo.systemUptime - started) * 1000
            result["sessions"] = try await waitSessionState(workspace, "cancelled", seconds: 30)
            result["scratch"] = scratch(scratchDirectory)

        case "during-quit":
            _ = try await ensureShell(controller)
            _ = try ensureAgent(controller)
            try await waitPreparing(workspace)
            // 快照仍在进行时请求退出；结果写出后应用立即终止。
            writeMarker(try markerDirectory(arguments), "quit")
            result["quit"] = true

        case "during-hold":
            _ = try ensureAgent(controller)
            try await waitPreparing(workspace)
            writeMarker(try markerDirectory(arguments), "holding")
            try await Task.sleep(for: .seconds(120))

        case "during-saves":
            guard let core = workspace.core,
                  let root = workspace.root else { throw CocoaError(.coderValueNotFound) }
            _ = try await ensureShell(controller)
            // 先建立一个 ready 会话，作为状态写入对象。
            let baseline = try await Task.detached { try core.beginAgentSession(vault: root) }.value
            let tab = try ensureAgent(controller)
            try await waitPreparing(workspace)
            let measurement = Measurement()
            let gap = GapRecorder()
            gap.start()
            Task.detached {
                let before = ProcessInfo.processInfo.systemUptime
                _ = try? core.saveLayout(json: Self.persistedLayoutJSON)
                _ = try? core.saveWindowSession(json: Self.emptySessionJSON)
                _ = try? core.markAgentSession(id: baseline.id, state: "running", exitCode: nil)
                measurement.finish(ProcessInfo.processInfo.systemUptime - before)
            }
            let deadline = ProcessInfo.processInfo.systemUptime + 1.2
            while ProcessInfo.processInfo.systemUptime < deadline,
                  !measurement.snapshot.completed {
                try await Task.sleep(for: .milliseconds(20))
            }
            let sampled = measurement.snapshot
            gap.stop()
            result["save_within_2s"] = sampled.completed
            result["save_ms"] = sampled.elapsed * 1000
            result["max_main_gap_ms"] = gap.maxGapMilliseconds
            try await wait(seconds: 30, until: "快照完成并尝试启动") {
                switch tab.agentPhase {
                case .running, .exited, .failed: return true
                case .idle, .preparing: return false
                }
            }
            result["tabs"] = tabStates(controller)
            result["sessions"] = sessions(workspace)
            result["scratch"] = scratch(scratchDirectory)
            result["baseline"] = baseline.id

        case "data-inside-vault":
            guard let blocked = argument("agent-blocked-vault", in: arguments) else {
                throw CocoaError(.coderValueNotFound)
            }
            let first = try await ensureShell(controller)
            send(first, "printf 'HORTUS_SHELL_READY\\n'")
            try await wait(seconds: 10, until: "普通 shell 响应") {
                screen(first).contains("HORTUS_SHELL_READY")
            }
            await workspace.openFolder(blocked)
            try await Task.sleep(for: .milliseconds(500))
            result["error"] = workspace.error ?? ""
            result["sessions"] = sessions(workspace)
            result["tabs"] = tabStates(controller)
            result["shell_alive"] = alive(first.session.inspectionShellPid)
            result["shell_ran"] = screen(first).contains("HORTUS_SHELL_READY")

        default:
            throw NSError(domain: "Hortus.AgentInspection", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: "未知 agent-mode \(mode)"])
        }
        return result
    }
}

/// 主线程响应间隔：快照/复制在后台时不应出现长阻塞。
@MainActor
private final class GapRecorder {
    private var task: Task<Void, Never>?
    private var last = 0.0
    private(set) var maxGapMilliseconds = 0.0

    func start() {
        last = ProcessInfo.processInfo.systemUptime
        task = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(5))
                guard let self else { return }
                let now = ProcessInfo.processInfo.systemUptime
                self.maxGapMilliseconds = max(self.maxGapMilliseconds, (now - self.last) * 1000)
                self.last = now
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
    }
}
#endif

/// 另一窗口写入的完成状态；轮询在主线程，实际写入在后台任务。
private final class Measurement: @unchecked Sendable {
    struct Sample {
        let completed: Bool
        let elapsed: Double
    }

    private let lock = NSLock()
    private var done = false
    private var duration = 0.0

    func finish(_ elapsed: Double) {
        lock.lock()
        duration = elapsed
        done = true
        lock.unlock()
    }

    var snapshot: Sample {
        lock.lock()
        defer { lock.unlock() }
        return Sample(completed: done, elapsed: duration)
    }
}
