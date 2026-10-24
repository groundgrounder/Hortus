import AppKit
import Darwin

/// 终端的真实窗口自检，只由显式验收参数启动。命令经 PTY 真正输进 shell，
/// 结果写进库外的临时目录，由 tools/check-terminal.py 在应用退出后再核对一遍。
@MainActor
enum TerminalInspection {
    private static func argument(_ name: String, in arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }.map { String($0.dropFirst(name.count + 3)) }
    }

    private static func quoted(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    private static func wait(seconds: Double, until description: String, _ condition: () -> Bool) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else {
                throw NSError(domain: "Hortus.TerminalInspection", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "等待超时（\(seconds) s）：\(description)"])
            }
            try await Task.sleep(for: .milliseconds(5))
        }
    }

    private static func alive(_ pid: pid_t) -> Bool { pid > 0 && kill(pid, 0) == 0 }

    private static func currentDirectory(_ pid: pid_t) -> String? {
        var info = proc_vnodepathinfo()
        let size = Int32(MemoryLayout<proc_vnodepathinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &info, size) == size else { return nil }
        return withUnsafeBytes(of: info.pvi_cdir.vip_path) { String(cString: $0.bindMemory(to: CChar.self).baseAddress!) }
    }

    private static func pidFile(_ url: URL) async throws -> pid_t {
        var value: pid_t = 0
        try await wait(seconds: 10, until: "shell 写入 \(url.lastPathComponent)") {
            value = (try? String(contentsOf: url, encoding: .utf8)).flatMap { pid_t($0.trimmingCharacters(in: .whitespacesAndNewlines)) } ?? 0
            return value > 0
        }
        return value
    }

    /// 打开终端，等 shell 起来；返回 shell 的 pid
    private static func showTerminal(workspace: Workspace, layout: Layout) async throws -> pid_t {
        layout.setTerminalVisible(true)
        let session = workspace.terminal
        try await wait(seconds: 20, until: "终端视图挂载并启动 shell") { session.state == .running && session.inspectionShellPid > 0 && session.inspectionViewAttached }
        return session.inspectionShellPid
    }

    static func run(workspace: Workspace, layout: Layout, arguments: [String]) async throws -> [String: Any] {
        guard let scratch = argument("terminal-scratch", in: arguments).map({ URL(fileURLWithPath: $0) }),
              let secondPath = argument("terminal-second-vault", in: arguments),
              let noteName = argument("terminal-new-note", in: arguments),
              let noteTitle = argument("terminal-new-title", in: arguments),
              let root = workspace.root, let document = workspace.current?.document, let path = workspace.selected
        else { throw CocoaError(.coderValueNotFound) }
        // Rust 返回规范路径；macOS 临时目录的 /var 是 /private/var 的符号链接。
        guard let resolved = realpath(secondPath, nil) else { throw POSIXError(.ENOENT) }
        let second = String(cString: resolved)
        free(resolved)
        let session = workspace.terminal
        var result: [String: Any] = [:]

        let shell = try await showTerminal(workspace: workspace, layout: layout)
        session.focus()
        result["shell_pid"] = Int(shell)
        result["shell_cwd"] = currentDirectory(shell) ?? ""

        // 1. 环境：HORTUS_VAULT、工作目录、语言
        session.inspectionSend("printf '%s' \"$HORTUS_VAULT\" > \(quoted(scratch.appendingPathComponent("vault.txt").path)); "
            + "pwd -P > \(quoted(scratch.appendingPathComponent("pwd.txt").path)); "
            + "printf '%s' \"$LANG\" > \(quoted(scratch.appendingPathComponent("lang.txt").path))\r")
        try await wait(seconds: 20, until: "shell 写入环境变量") { FileManager.default.fileExists(atPath: scratch.appendingPathComponent("lang.txt").path) }

        // 2. 在终端里新建笔记，文件树里出现的时间
        var started = ProcessInfo.processInfo.systemUptime
        session.inspectionSend("printf '# %s\\n' \(quoted(noteTitle)) > \(quoted(noteName))\r")
        try await wait(seconds: 10, until: "新建笔记出现在文件树") { workspace.entries[noteName] != nil }
        result["new_note_tree_ms"] = (ProcessInfo.processInfo.systemUptime - started) * 1000

        // 3. 终端改写正在阅读的笔记：替换中间一段（sed -i 先写临时文件再改名），再在末尾追加一段
        var view = try await ReadingInspection.settled(true, document: document)
        var refreshes: [[String: Any]] = []
        for (name, command) in [
            ("sed_replace", "sed -i '' 's/^Paragraph 0400:.*/Paragraph 0400: rewritten from the terminal./' \(quoted(path))"),
            ("append", "printf '\\nParagraph 9999: appended from the terminal.\\n' >> \(quoted(path))"),
        ] {
            guard let coordinator = view.readingCoordinator else { throw CocoaError(.coderValueNotFound) }
            coordinator.reveal(source: document.text.utf16.count / 2); view.layoutSubtreeIfNeeded()
            guard let before = coordinator.visibleAnchor() else { throw CocoaError(.coderValueNotFound) }
            let revision = document.revision
            started = ProcessInfo.processInfo.systemUptime
            session.inspectionSend(command + "\r")
            try await wait(seconds: 10, until: "阅读视图刷新：\(name)") {
                document.revision != revision && coordinator.model?.revision == document.revision && !coordinator.pending
            }
            view = try await ReadingInspection.settled(true, document: document)
            guard let after = view.readingCoordinator?.visibleAnchor() else { throw CocoaError(.coderValueNotFound) }
            let disk = try String(contentsOf: URL(fileURLWithPath: root).appendingPathComponent(path), encoding: .utf8)
            refreshes.append([
                "case": name, "display_ms": (coordinator.displayedAt - started) * 1000,
                "same_anchor": before.id == after.id, "anchor_shift_pt": abs(before.offset - after.offset),
                "matches_disk": document.text == disk, "rebuilds": coordinator.model?.rebuilt ?? 0,
            ])
        }
        result["refresh"] = refreshes

        // 4. shell 自己退出：停在"已退出"，界面刷新也不能把它重新拉起来；点"重新启动"才开新 shell
        let exited: () -> Bool = { if case .exited = session.state { true } else { false } }
        session.inspectionSend("exit\r")
        try await wait(seconds: 10, until: "shell 自己退出") { exited() && !alive(shell) }
        // 改字号会让终端面板重新走一遍 SwiftUI 更新，原来的问题就是在这里重新启动了 shell
        workspace.fontSize += 1
        try await Task.sleep(for: .milliseconds(800))
        workspace.fontSize -= 1
        try await Task.sleep(for: .milliseconds(800))
        result["exit_stays_exited"] = exited() && session.inspectionShellPid == 0
        if let shot = argument("terminal-exit-snapshot", in: arguments) { try await AppDelegate.snapshot(to: shot) }
        session.restart()
        try await wait(seconds: 20, until: "重新启动 shell") { session.state == .running && session.inspectionShellPid > 0 && session.inspectionShellPid != shell }
        let restarted = session.inspectionShellPid
        result["restarted_pid"] = Int(restarted)

        // 5. 切换知识库：旧 shell 和它启动的程序都要结束，新 shell 在新库根
        session.inspectionSend("sh -c 'echo $$ > \(scratch.path)/job1.pid; exec sleep 600.101' &\r")
        let job1 = try await pidFile(scratch.appendingPathComponent("job1.pid"))
        started = ProcessInfo.processInfo.systemUptime
        await workspace.openFolder(second)
        try await wait(seconds: 20, until: "新知识库启动 shell") {
            session.root == second && session.state == .running && session.inspectionShellPid > 0 && session.inspectionShellPid != restarted
        }
        try await wait(seconds: 10, until: "切库后旧 shell 和后台程序结束") { !alive(restarted) && !alive(job1) }
        let shell2 = session.inspectionShellPid
        result["switch_old_exit_ms"] = (ProcessInfo.processInfo.systemUptime - started) * 1000
        result["job1_pid"] = Int(job1)
        result["shell2_pid"] = Int(shell2)
        result["shell2_cwd"] = currentDirectory(shell2) ?? ""

        // 6. 新 shell 里留一个后台程序、一个前台程序，退出应用后由脚本检查它们都已结束
        session.inspectionSend("sh -c 'echo $$ > \(scratch.path)/job2.pid; exec sleep 600.202' &\r")
        result["job2_pid"] = Int(try await pidFile(scratch.appendingPathComponent("job2.pid")))
        session.inspectionSend("sh -c 'echo $$ > \(scratch.path)/job3.pid; exec sleep 600.303'\r")
        result["job3_pid"] = Int(try await pidFile(scratch.appendingPathComponent("job3.pid")))
        result["app_pid"] = Int(getpid())
        return result
    }

    /// 空闲 CPU 的三种状态：hidden（终端在后台）、unfocused（窗口失去焦点，由脚本切到别的应用）、focused
    static func idle(workspace: Workspace, layout: Layout, arguments: [String]) async throws -> [String: Any] {
        let mode = argument("terminal-idle-mode", in: arguments) ?? "focused"
        let shell = try await showTerminal(workspace: workspace, layout: layout)
        workspace.terminal.focus()
        if mode == "hidden" {
            layout.setTerminalVisible(false)
            try await Task.sleep(for: .milliseconds(300))
            guard workspace.terminal.state == .running, alive(shell) else { throw CocoaError(.coderValueNotFound) }
        }
        return ["app_pid": Int(getpid()), "shell_pid": Int(shell), "mode": mode,
                "terminal_visible": layout.terminalVisible]
    }
}
