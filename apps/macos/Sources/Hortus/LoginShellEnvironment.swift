import Darwin
import Foundation

/// 登录 shell PATH 的一次性只读探测。
///
/// 从 Finder 或程序坞启动时，`launchd` 传入的 PATH 只有系统目录；
/// 用户在终端里能运行的 Homebrew、`~/.local/bin`、`~/.cargo/bin` 等命令不可见。
/// 这里在后台运行一次 `$SHELL -lc 'printf "%s" "$PATH"'`，最多等 1.5 秒；
/// 超时、失败、空值或非零退出都返回 nil，由调用方使用进程 PATH 与常见目录。
enum LoginShellEnvironment {
    private static let cache = Cache()

    /// 进程内只探测一次；重复打开设置或启动 Agent 不再运行 shell。
    static func resolvedPath() async -> String? {
        await cache.value()
    }

    #if DEBUG
    /// 自检注入的三种状态：未提供、提供有效值、显式提供空值（不运行 shell）。
    enum InjectedPath: Equatable {
        case absent
        case path(String?)
    }

    /// `--settings-login-path=<PATH>` 只存在于 Debug 构建。
    nonisolated static func injectedPath(arguments: [String]) -> InjectedPath {
        let prefix = "--settings-login-path="
        guard let raw = arguments.first(where: { $0.hasPrefix(prefix) }) else {
            return .absent
        }
        let value = String(raw.dropFirst(prefix.count))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return .path(value.isEmpty ? nil : value)
    }
    #endif

    private actor Cache {
        private var resolved: String??

        func value() async -> String? {
            if let resolved { return resolved }
            let path = await Task.detached(priority: .utility) { probe() }.value
            resolved = path
            return path
        }
    }

    /// 在后台线程同步运行；调用方负责不占用主线程。
    nonisolated static func probe() -> String? {
        #if DEBUG
        if case let .path(path) = injectedPath(arguments: ProcessInfo.processInfo.arguments) {
            return path
        }
        #endif
        let environment = ProcessInfo.processInfo.environment
        let shell = environment["SHELL"].flatMap { $0.isEmpty ? nil : $0 } ?? "/bin/zsh"
        guard FileManager.default.isExecutableFile(atPath: shell) else { return nil }

        // 输出走临时文件而不是管道：profile 留下的后台进程即使继承 stdout，
        // 也不会让读取端一直等到 EOF。
        let output = FileManager.default.temporaryDirectory
            .appendingPathComponent("hortus-login-path-\(UUID().uuidString)")
        guard FileManager.default.createFile(atPath: output.path, contents: nil),
              let handle = try? FileHandle(forWritingTo: output) else { return nil }
        defer { try? FileManager.default.removeItem(at: output) }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: shell)
        process.arguments = ["-lc", "printf '%s' \"$PATH\""]
        process.environment = environment
        process.standardOutput = handle
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            try? handle.close()
            return nil
        }

        let deadline = Date().addingTimeInterval(1.5)
        while process.isRunning, Date() < deadline {
            Thread.sleep(forTimeInterval: 0.01)
        }
        if process.isRunning {
            process.terminate()
            let grace = Date().addingTimeInterval(0.2)
            while process.isRunning, Date() < grace {
                Thread.sleep(forTimeInterval: 0.01)
            }
            if process.isRunning {
                kill(process.processIdentifier, SIGKILL)
            }
        }
        process.waitUntilExit()
        try? handle.close()
        guard process.terminationStatus == 0,
              let data = try? Data(contentsOf: output) else { return nil }
        let text = String(decoding: data.prefix(16 * 1024), as: UTF8.self)
        // 登录脚本可能向 stdout 打印提示；PATH 是最后一行。
        let line = text.split(separator: "\n").last.map(String.init) ?? ""
        let path = line.trimmingCharacters(in: .whitespacesAndNewlines)
        return path.isEmpty ? nil : path
    }
}
