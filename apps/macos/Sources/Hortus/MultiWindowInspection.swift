#if DEBUG
import AppKit

/// 先在真实窗口上记录基线，后续检查沿用同一启动入口。
@MainActor
enum MultiWindowInspection {
    static func baseline(workspace: Workspace, output: String) async {
        do {
            guard let argument = ProcessInfo.processInfo.arguments.first(where: { $0.hasPrefix("--multi-window-second=") }) else {
                throw CocoaError(.fileReadInvalidFileName)
            }
            let originalRoot = workspace.root
            let originalTabs = workspace.tabs.map(\.path)
            await workspace.openFolder(String(argument.dropFirst("--multi-window-second=".count)))
            try await Task.sleep(for: .milliseconds(300))
            let result: [String: Any] = [
                "visible_windows": NSApp.windows.filter { $0.isVisible && $0.canBecomeMain }.count,
                "original_root": originalRoot ?? "",
                "root_after_open": workspace.root ?? "",
                "original_tabs": originalTabs,
                "tabs_after_open": workspace.tabs.map(\.path),
                "original_preserved": originalRoot == workspace.root && originalTabs == workspace.tabs.map(\.path),
            ]
            let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: URL(fileURLWithPath: output))
            NSApp.terminate(nil)
        } catch { fputs("Hortus multi-window inspection failed: \(error)\n", stderr); exit(1) }
    }
}
#endif
