#if DEBUG
import AppKit
import CoreGraphics
import Darwin
@testable import SwiftTerm

/// 统一工具标签的真实窗口证据：每组 + 菜单、单行静态/Shell/Agent 标签、
/// 跨区终端和两个窗口各自不同的动态标签。只由显式检查参数启动。
@MainActor
enum ToolTabsInspection {
    private static func argument(_ name: String, in arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }.map { String($0.dropFirst(name.count + 3)) }
    }

    private static func collapsed(_ text: String) -> String { text.filter { !$0.isWhitespace } }

    private static func fullBuffer(_ tab: TerminalTab) -> String {
        guard let view = tab.session.inspectionView else { return "" }
        return view.withTerminal { terminal in
            String(decoding: terminal.getBufferAsData(kind: .normal), as: UTF8.self)
        }
    }

    private static func wait(seconds: Double, until description: String, _ condition: () -> Bool) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else {
                throw NSError(domain: "Hortus.ToolTabsInspection", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "等待超时（\(seconds) s）：\(description)"])
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    static func run(workspace: Workspace, layout: Layout, arguments: [String]) async throws -> [String: Any] {
        guard let mediaPath = argument("tool-tabs-media", in: arguments),
              let window = workspace.application?.records.first(where: { $0.workspace === workspace })?.window
        else { throw CocoaError(.coderValueNotFound) }
        let media = URL(fileURLWithPath: mediaPath)
        try FileManager.default.createDirectory(at: media, withIntermediateDirectories: true)

        fputs("tool-tabs: media \(media.path)\n", stderr)
        layout.size[.bottom] = 300
        fputs("tool-tabs: show terminal\n", stderr)
        workspace.showTerminal()
        guard let firstShell = workspace.currentTerminalTab else { throw CocoaError(.coderValueNotFound) }
        fputs("tool-tabs: first shell\n", stderr)
        try await wait(seconds: 20, until: "第一个 Shell 启动") { firstShell.session.inspectionShellPid > 0 }
        fputs("tool-tabs: second shell\n", stderr)
        guard let secondShell = workspace.addShell(to: layout.defaultTerminalGroupID) else { throw CocoaError(.coderValueNotFound) }
        try await wait(seconds: 20, until: "第二个 Shell 启动") { secondShell.session.inspectionShellPid > 0 }
        fputs("tool-tabs: agent tab\n", stderr)
        guard let agent = workspace.addAgent(to: layout.defaultTerminalGroupID) else {
            throw CocoaError(.coderValueNotFound)
        }
        fputs("tool-tabs: agent ready\n", stderr)
        try await wait(seconds: 30, until: "Agent 标签就绪") {
            switch agent.agentPhase {
            case .running, .failed: true
            case .idle, .preparing: false
            case .exited: true
            }
        }
        try await Task.sleep(for: .milliseconds(400))
        fputs("tool-tabs: single row snapshot\n", stderr)
        try await AppDelegate.snapshot(to: media.appendingPathComponent("single-row-tabs.png").path, window: window)

        // 每个区域标签条尾部的 + 菜单。
        fputs("tool-tabs: plus menus\n", stderr)
        for region in Region.allCases {
            guard let group = layout.availableGroups(in: region).first else { continue }
            let name = "group-add-" + group.id.uuidString
            if try await ToolDragInspection.click(name, window: window) {
                try await Task.sleep(for: .milliseconds(500))
                try await AppDelegate.snapshotDisplay(to: media.appendingPathComponent("plus-\(region.rawValue).png").path, around: window)
                ToolDragInspection.pressEscape()
                try await Task.sleep(for: .milliseconds(350))
            }
        }

        // 静态工具与终端使用同一真实拖放路径：左 → 右 → 底部。
        let staticRight = layout.availableGroups(in: .right).first
        let staticBottom = layout.availableGroups(in: .bottom).first
        var staticToRight = false
        if let staticRight {
            staticToRight = try await ToolDragInspection.drag(
                from: "panel-tab-outline", to: "group-tabs-\(staticRight.id.uuidString)", window: window)
                && layout.region(containing: .tool(.outline)) == .right
        }
        var staticToBottom = false
        if let staticBottom {
            staticToBottom = try await ToolDragInspection.drag(
                from: "panel-tab-outline", to: "group-tabs-\(staticBottom.id.uuidString)", window: window)
                && layout.region(containing: .tool(.outline)) == .bottom
        }
        if layout.region(containing: .tool(.outline)) == .bottom {
            layout.moveToRegion(.tool(.outline), .right)
        }
        try await Task.sleep(for: .milliseconds(300))

        // 拖动终端标签到右侧后仍是同一个进程和会话。
        let secondPid = secondShell.session.inspectionShellPid
        secondShell.session.inspectionSend("printf 'HORTUS_DRAG_MARKER\\n'\r")
        try await wait(seconds: 5, until: "拖动前标记写入终端") {
            fullBuffer(secondShell).contains("HORTUS_DRAG_MARKER")
        }
        let priorBuffer = collapsed(fullBuffer(secondShell))
        let viewBefore = secondShell.session.inspectionView

        if let rightGroup = layout.availableGroups(in: .right).first {
            _ = try await ToolDragInspection.drag(
                from: "panel-tab-terminal-\(secondShell.id.uuidString)",
                to: "group-tabs-\(rightGroup.id.uuidString)", window: window)
        }
        try await Task.sleep(for: .milliseconds(500))
        let afterDrag = collapsed(fullBuffer(secondShell))
        let sameView = secondShell.session.inspectionView === viewBefore
        try await Task.sleep(for: .milliseconds(400))
        fputs("tool-tabs: terminal snapshot\n", stderr)
        try await AppDelegate.snapshot(to: media.appendingPathComponent("terminal-right.png").path, window: window)
        let moved = layout.region(containing: .terminal(secondShell.id)) == .right
            && secondShell.session.inspectionShellPid == secondPid

        // 两个窗口各自不同的动态标签。
        var secondWindowCounts: [Int] = []
        fputs("tool-tabs: second window\n", stderr)
        if let secondVault = argument("tool-tabs-second", in: arguments) {
            guard let resolved = realpath(secondVault, nil) else { throw CocoaError(.fileReadNoSuchFile) }
            let canonical = String(cString: resolved)
            free(resolved)
            await workspace.openFolder(secondVault)
            guard let application = workspace.application else { throw CocoaError(.coderValueNotFound) }
            fputs("tool-tabs: roots \(application.records.map { $0.workspace.root ?? "-" }) canonical \(canonical)\n", stderr)
            guard let secondRecord = application.records.first(where: { $0.workspace.root == canonical }) else {
                throw CocoaError(.coderValueNotFound)
            }
            secondRecord.workspace.showTerminal()
            _ = secondRecord.workspace.addShell(to: secondRecord.layout.defaultTerminalGroupID)
            try await wait(seconds: 20, until: "第二个窗口的 Shell 启动") {
                secondRecord.workspace.terminalPid > 0
            }
            application.activate(secondRecord)
            if let screen = NSScreen.main?.visibleFrame,
               let firstWindow = application.records.first(where: { $0.workspace === workspace })?.window,
               let secondWindow = secondRecord.window {
                firstWindow.setFrame(NSRect(x: screen.minX + 20, y: screen.maxY - 660, width: 900, height: 640), display: true)
                secondWindow.setFrame(NSRect(x: screen.maxX - 920, y: screen.minY + 20, width: 900, height: 640), display: true)
            }
            try await Task.sleep(for: .milliseconds(500))
            secondWindowCounts = [secondRecord.layout.terminalIDs.count]
            fputs("tool-tabs: second window snapshot\n", stderr)
            try await MultiWindowInspection.snapshotWindows(application.records,
                                                           to: media.appendingPathComponent("two-windows.png"))
        }

        return [
            "media": media.path,
            "single_row": ["shells": 2, "agents": 1],
            "terminal_moved_right": moved,
            "terminal_pid_preserved": secondShell.session.inspectionShellPid == secondPid,
            "terminal_buffer_preserved": sameView
                && afterDrag.contains("HORTUS_DRAG_MARKER")
                && secondShell.session.inspectionShellPid == secondPid,
            "terminal_same_view": sameView,
            "buffer_chars": [priorBuffer.count, afterDrag.count],
            "static_drag": ["right": staticToRight, "bottom": staticToBottom],
            "first_window_terminals": layout.terminalIDs.count,
            "second_window_terminals": secondWindowCounts.first ?? 0,
            "plus_buttons": Region.allCases.compactMap { region in
                layout.availableGroups(in: region).first.map { region.rawValue + ":" + $0.id.uuidString }
            },
            "first_pid": Int(firstShell.session.inspectionShellPid),
            "second_pid": Int(secondPid),
        ]
    }
}
#endif
