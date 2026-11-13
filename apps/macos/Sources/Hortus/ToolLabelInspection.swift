#if DEBUG
import AppKit
import CoreGraphics
import Darwin
import SwiftUI

/// P1-53 标签文本二态的真实窗口证据：完整标题或只显示图标。
/// 只由显式检查参数启动；覆盖静态/Shell/Agent、中英文、选中/未选中、浅色/深色、
/// 反复改宽、上下文菜单关闭与 + 菜单重加。
@MainActor
enum ToolLabelInspection {
    private static let wideWidth: CGFloat = 320
    private static let narrowWidth: CGFloat = 180

    private static func argument(_ name: String, in arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }.map { String($0.dropFirst(name.count + 3)) }
    }

    private static func wait(seconds: Double, until description: String, _ condition: () -> Bool) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else {
                throw NSError(domain: "Hortus.ToolLabelInspection", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "等待超时（\(seconds) s）：\(description)"])
            }
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    private static func probe(_ name: String, in window: NSWindow) -> NSView? {
        guard let view = ToolDragInspection.find(name, in: window), !view.isHiddenOrHasHiddenAncestor else {
            return nil
        }
        return view
    }

    private static func frame(_ name: String, in window: NSWindow) -> CGRect? {
        guard let view = probe(name, in: window) else { return nil }
        return view.convert(view.bounds, to: nil)
    }

    private static func alive(_ pid: pid_t) -> Bool {
        guard pid > 0 else { return false }
        return kill(pid, 0) == 0
    }

    /// 与 TabStrip 的 `.callout` 和选中字重一致；只用于核对标签没有截断。
    private static func textWidth(_ text: String, selected: Bool) -> CGFloat {
        let size = NSFont.preferredFont(forTextStyle: .callout).pointSize
        let font = NSFont.systemFont(ofSize: size, weight: selected ? .semibold : .regular)
        return (text as NSString).size(withAttributes: [.font: font]).width
    }

    /// 一次布局状态：标签条宽度、标签宽度、文本探针是否存在及其宽度。
    private static func capture(_ tool: ToolID, title: String, selected: Bool, groupID: UUID,
                                window: NSWindow) -> [String: Any] {
        let strip = frame("group-tabs-" + groupID.uuidString, in: window)
        let tab = frame(tool.probeName, in: window)
        let titleView = frame(tool.probeName + "-title", in: window)
        let expected = textWidth(title, selected: selected)
        return [
            "title": title,
            "selected": selected,
            "strip_width": strip?.width ?? 0,
            "tab_width": tab?.width ?? 0,
            "title_visible": titleView != nil,
            "title_width": titleView?.width ?? 0,
            "expected_title_width": expected,
            "title_is_full": titleView.map { abs($0.width - expected) <= 1 } ?? false,
        ]
    }

    private static func screenshot(_ name: String, media: URL, window: NSWindow) async throws -> String {
        let path = media.appendingPathComponent(name).path
        try await AppDelegate.snapshot(to: path, window: window)
        return path
    }

    private static func setAppearance(dark: Bool) async throws {
        NSApp.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        try await Task.sleep(for: .milliseconds(250))
    }

    private static var visibleMenuWindows: [NSWindow] {
        NSApp.windows.filter { $0.isVisible && String(describing: type(of: $0)).lowercased().contains("menu") }
    }

    private static func check(_ row: [String: Any], widthName: String, selected: Bool, key: String,
                              failures: inout [String]) {
        let visible = row["title_visible"] as? Bool == true
        let full = row["title_is_full"] as? Bool == true
        if visible && !full {
            failures.append("\(key) \(widthName) \(selected ? "selected" : "unselected"): 文本宽度与完整标题不符")
        }
        if widthName == "wide" && !visible {
            failures.append("\(key) \(widthName) \(selected ? "selected" : "unselected"): 足够宽度下完整标题未显示")
        }
        if widthName == "narrow" && visible {
            failures.append("\(key) \(widthName) \(selected ? "selected" : "unselected"): 不足宽度下仍显示文本")
        }
    }

    /// 浅色/深色 × 宽/窄 × 选中/未选中，共 8 个真实窗口状态。
    private static func captureStates(_ key: String, tool: ToolID, title: String, companion: ToolID,
                                      groupID: UUID, layout: Layout, media: URL, window: NSWindow,
                                      failures: inout [String]) async throws -> [[String: Any]] {
        var rows: [[String: Any]] = []
        for dark in [false, true] {
            try await setAppearance(dark: dark)
            for (widthName, width) in [("wide", wideWidth), ("narrow", narrowWidth)] {
                layout.size[.right] = width
                try await Task.sleep(for: .milliseconds(220))
                for selected in [true, false] {
                    layout.select(selected ? tool : companion, in: groupID)
                    try await Task.sleep(for: .milliseconds(160))
                    var row = capture(tool, title: title, selected: selected, groupID: groupID, window: window)
                    row["appearance"] = dark ? "dark" : "light"
                    row["width"] = widthName
                    let name = "\(key)-\(dark ? "dark" : "light")-\(widthName)-\(selected ? "selected" : "unselected").png"
                    row["screenshot"] = try await screenshot(name, media: media, window: window)
                    check(row, widthName: widthName, selected: selected, key: key, failures: &failures)
                    rows.append(row)
                }
            }
        }
        try await setAppearance(dark: false)
        return rows
    }

    /// 反复改宽：每次采样文本要么完整、要么不存在，不能出现截断或省略号。
    private static func resizeCycles(tool: ToolID, title: String, selected: Bool, groupID: UUID,
                                     layout: Layout, window: NSWindow, failures: inout [String]) async throws -> [[String: Any]] {
        var rows: [[String: Any]] = []
        layout.select(tool, in: groupID)
        try await Task.sleep(for: .milliseconds(150))
        for index in 0..<8 {
            let widthName = index.isMultiple(of: 2) ? "wide" : "narrow"
            layout.size[.right] = widthName == "wide" ? wideWidth : narrowWidth
            try await Task.sleep(for: .milliseconds(120))
            var row = capture(tool, title: title, selected: selected, groupID: groupID, window: window)
            row["width"] = widthName
            check(row, widthName: widthName, selected: selected, key: "resize", failures: &failures)
            rows.append(row)
        }
        return rows
    }

    private static func rightClick(name: String, window: NSWindow) async throws -> Bool {
        guard CGPreflightPostEventAccess() else { return false }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        guard let rect = frame(name, in: window) else { return false }
        let screen = window.convertToScreen(rect)
        let height = CGDisplayBounds(CGMainDisplayID()).height
        let point = CGPoint(x: screen.midX, y: height - screen.midY)
        await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                post(.mouseMoved, point)
                Thread.sleep(forTimeInterval: 0.1)
                post(.rightMouseDown, point)
                Thread.sleep(forTimeInterval: 0.08)
                post(.rightMouseUp, point)
                continuation.resume()
            }
        }
        return true
    }

    nonisolated private static func post(_ type: CGEventType, _ point: CGPoint) {
        CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .right)?
            .post(tap: .cghidEventTap)
    }

    nonisolated private static func press(_ code: CGKeyCode) {
        CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)?.post(tap: .cghidEventTap)
        CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)?.post(tap: .cghidEventTap)
    }

    /// 右键打开上下文菜单，截图后按 Down×3 + Return 选择“关闭 <标签>”。
    private static func closeViaMenu(tool: ToolID, menuScreenshot: String, layout: Layout,
                                     media: URL, window: NSWindow) async throws -> [String: Any] {
        let clicked = try await rightClick(name: tool.probeName, window: window)
        var menuVisible = false
        let menuDeadline = ProcessInfo.processInfo.systemUptime + 3
        while ProcessInfo.processInfo.systemUptime < menuDeadline {
            if !visibleMenuWindows.isEmpty { menuVisible = true; break }
            try await Task.sleep(for: .milliseconds(50))
        }
        let windowClasses = NSApp.windows.filter(\.isVisible).map { String(describing: type(of: $0)) }
        if menuVisible { try await Task.sleep(for: .milliseconds(300)) }
        let path = media.appendingPathComponent(menuScreenshot).path
        if menuVisible {
            try await AppDelegate.snapshotDisplay(to: path, around: window)
        } else {
            try await AppDelegate.snapshot(to: path, window: window)
        }
        for _ in 0..<3 {
            press(125)
            try await Task.sleep(for: .milliseconds(90))
        }
        press(36)
        var closed = false
        let closeDeadline = ProcessInfo.processInfo.systemUptime + 6
        while ProcessInfo.processInfo.systemUptime < closeDeadline {
            if layout.locateTool(tool) == nil { closed = true; break }
            try await Task.sleep(for: .milliseconds(50))
        }
        return ["clicked": clicked, "menu_visible": menuVisible, "menu_windows": windowClasses,
                "menu_screenshot": path, "closed": closed]
    }

    /// 静态工具关闭后从 + 菜单重加；菜单顺序是 Files、Search、Outline。
    private static func addViaPlus(_ tool: StaticTool, groupID: UUID, layout: Layout,
                                   window: NSWindow) async throws -> Bool {
        guard try await ToolDragInspection.click("group-add-" + groupID.uuidString, window: window) else {
            return false
        }
        try await Task.sleep(for: .milliseconds(500))
        for _ in 0..<3 {
            press(125)
            try await Task.sleep(for: .milliseconds(90))
        }
        press(36)
        let deadline = ProcessInfo.processInfo.systemUptime + 4
        while ProcessInfo.processInfo.systemUptime < deadline {
            if layout.locateTool(.tool(tool)) != nil { return true }
            try await Task.sleep(for: .milliseconds(50))
        }
        return false
    }

    /// 供外部 Accessibility 探针读取的停顿状态：右侧三个图标-only 标签。
    private static func hold(workspace: Workspace, layout: Layout, arguments: [String],
                             window: NSWindow) async throws -> [String: Any] {
        layout.visible[.bottom] = true
        layout.size[.bottom] = 260
        layout.size[.right] = wideWidth
        workspace.showTerminal()
        guard let first = workspace.terminals.allTabs.first(where: { $0.kind == .shell }) else {
            throw CocoaError(.coderValueNotFound)
        }
        try await wait(seconds: 20, until: "Shell 启动") { first.session.inspectionShellPid > 0 }
        guard let rightGroupID = layout.availableGroups(in: .right).first?.id else {
            throw CocoaError(.coderValueNotFound)
        }
        for tab in layout.availableGroups(in: .right).first?.tabs ?? [] where tab != .tool(.outline) {
            layout.remove(tab)
        }
        if layout.locateTool(.tool(.outline)) == nil {
            workspace.addStaticTool(.outline, to: rightGroupID)
        }
        guard workspace.addShell(to: rightGroupID) != nil,
              workspace.addShell(to: rightGroupID) != nil else {
            throw CocoaError(.coderValueNotFound)
        }
        layout.size[.right] = narrowWidth

        try await Task.sleep(for: .milliseconds(700))
        let tabs = layout.availableGroups(in: .right).first?.tabs ?? []
        return [
            "language": Bundle.main.preferredLocalizations.first ?? "",
            "right_tabs": tabs.map(\.id),
            "titles": tabs.map { tool in
                if let staticTool = tool.staticTool { return staticTool.title }
                if let id = tool.terminalID { return workspace.terminals.tab(id)?.title ?? "" }
                return ""
            },
            "right_width": narrowWidth,
            "strip_width": frame("group-tabs-" + rightGroupID.uuidString, in: window)?.width ?? 0,
            "titles_visible": tabs.map { frame($0.probeName + "-title", in: window) != nil },
            "hold": true,
        ]
    }

    static func run(workspace: Workspace, layout: Layout, arguments: [String]) async throws -> [String: Any] {
        guard let mediaPath = argument("tool-label-media", in: arguments),
              let window = workspace.application?.records.first(where: { $0.workspace === workspace })?.window
        else { throw CocoaError(.coderValueNotFound) }
        let media = URL(fileURLWithPath: mediaPath)
        try FileManager.default.createDirectory(at: media, withIntermediateDirectories: true)
        fputs("label: media \(media.path)\n", stderr)
        if arguments.contains(where: { $0.hasPrefix("--tool-label-hold") }) {
            return try await hold(workspace: workspace, layout: layout, arguments: arguments, window: window)
        }

        if let screen = NSScreen.main?.visibleFrame {
            window.setFrame(NSRect(x: screen.minX + 40, y: screen.maxY - 760, width: 1000, height: 720),
                            display: true)
        }
        layout.visible[.bottom] = true
        layout.size[.bottom] = 260
        layout.size[.right] = wideWidth
        try await Task.sleep(for: .milliseconds(300))

        // 三个 Shell：前两个作右侧标签的同伴，第三个证明关闭只结束目标进程。
        fputs("label: shells\n", stderr)
        workspace.showTerminal()
        guard let firstShell = workspace.terminals.allTabs.first(where: { $0.kind == .shell }) else {
            throw CocoaError(.coderValueNotFound)
        }
        try await wait(seconds: 20, until: "第一个 Shell 启动") { firstShell.session.inspectionShellPid > 0 }
        guard let secondShell = workspace.addShell(to: layout.defaultTerminalGroupID) else {
            throw CocoaError(.coderValueNotFound)
        }
        try await wait(seconds: 20, until: "第二个 Shell 启动") { secondShell.session.inspectionShellPid > 0 }
        guard let backgroundShell = workspace.addShell(to: layout.defaultTerminalGroupID) else {
            throw CocoaError(.coderValueNotFound)
        }
        try await wait(seconds: 20, until: "第三个 Shell 启动") { backgroundShell.session.inspectionShellPid > 0 }
        let backgroundPid = backgroundShell.session.inspectionShellPid

        guard let rightGroupID = layout.availableGroups(in: .right).first?.id else {
            throw CocoaError(.coderValueNotFound)
        }
        // 只保留 Outline，避免上次会话的其它标签影响宽度判断。
        for tab in layout.availableGroups(in: .right).first?.tabs ?? [] where tab != .tool(.outline) {
            layout.remove(tab)
        }
        if layout.locateTool(.tool(.outline)) == nil {
            workspace.addStaticTool(.outline, to: rightGroupID)
        }
        layout.moveToRegion(.terminal(firstShell.id), .right)
        layout.moveToRegion(.terminal(secondShell.id), .right)
        try await wait(seconds: 10, until: "两个同伴 Shell 进入右侧") {
            layout.region(containing: .terminal(firstShell.id)) == .right
                && layout.region(containing: .terminal(secondShell.id)) == .right
        }

        var cases: [String: Any] = [:]
        var failures: [String] = []

        // 静态工具：Outline 为目标，第一个 Shell 为未选中同伴。
        fputs("label: static\n", stderr)
        let staticTool = ToolID.tool(.outline)
        let staticTitle = StaticTool.outline.title
        layout.select(staticTool, in: rightGroupID)
        var staticCase: [String: Any] = [:]
        staticCase["states"] = try await captureStates("static", tool: staticTool, title: staticTitle,
                                                       companion: .terminal(firstShell.id), groupID: rightGroupID,
                                                       layout: layout, media: media, window: window,
                                                       failures: &failures)
        staticCase["resize"] = try await resizeCycles(tool: staticTool, title: staticTitle, selected: true,
                                                      groupID: rightGroupID, layout: layout, window: window,
                                                      failures: &failures)
        layout.size[.right] = wideWidth
        layout.select(staticTool, in: rightGroupID)
        try await Task.sleep(for: .milliseconds(200))
        staticCase["context_close"] = try await closeViaMenu(tool: staticTool, menuScreenshot: "static-context-menu.png",
                                                             layout: layout, media: media, window: window)
        staticCase["closed"] = layout.locateTool(staticTool) == nil
        staticCase["readded"] = try await addViaPlus(.outline, groupID: rightGroupID, layout: layout, window: window)
        cases["static"] = staticCase

        // Shell：右侧已有两个 Shell 和 Outline；目标为第一个 Shell，第二个 Shell 作未选中同伴。
        fputs("label: shell\n", stderr)
        let shellTool = ToolID.terminal(firstShell.id)
        let shellPid = firstShell.session.inspectionShellPid
        layout.select(shellTool, in: rightGroupID)
        var shellCase: [String: Any] = [:]
        shellCase["pid"] = Int(shellPid)
        shellCase["states"] = try await captureStates("shell", tool: shellTool, title: firstShell.title,
                                                      companion: .terminal(secondShell.id), groupID: rightGroupID,
                                                      layout: layout, media: media, window: window,
                                                      failures: &failures)
        shellCase["resize"] = try await resizeCycles(tool: shellTool, title: firstShell.title, selected: true,
                                                     groupID: rightGroupID, layout: layout, window: window,
                                                     failures: &failures)
        layout.size[.right] = narrowWidth
        layout.select(shellTool, in: rightGroupID)
        try await Task.sleep(for: .milliseconds(200))
        shellCase["context_close"] = try await closeViaMenu(tool: shellTool, menuScreenshot: "shell-context-menu.png",
                                                            layout: layout, media: media, window: window)
        try await wait(seconds: 10, until: "Shell 进程退出") { !alive(shellPid) }
        shellCase["closed"] = layout.locateTool(shellTool) == nil
        shellCase["target_dead"] = !alive(shellPid)
        shellCase["background_alive"] = alive(backgroundPid)
        shellCase["second_pid"] = Int(backgroundPid)
        cases["shell"] = shellCase

        // Agent：加在右侧，第二个 Shell 仍作未选中同伴。
        fputs("label: agent\n", stderr)
        guard let agent = workspace.addAgent(to: rightGroupID) else { throw CocoaError(.coderValueNotFound) }
        try await wait(seconds: 30, until: "Agent 就绪") {
            switch agent.agentPhase {
            case .running, .failed: true
            case .idle, .preparing: false
            case .exited: true
            }
        }
        let agentTool = ToolID.terminal(agent.id)
        let agentPid = agent.session.inspectionShellPid
        layout.select(agentTool, in: rightGroupID)
        var agentCase: [String: Any] = [:]
        agentCase["pid"] = Int(agentPid)
        agentCase["states"] = try await captureStates("agent", tool: agentTool, title: agent.title,
                                                      companion: .terminal(secondShell.id), groupID: rightGroupID,
                                                      layout: layout, media: media, window: window,
                                                      failures: &failures)
        agentCase["resize"] = try await resizeCycles(tool: agentTool, title: agent.title, selected: true,
                                                     groupID: rightGroupID, layout: layout, window: window,
                                                     failures: &failures)
        layout.size[.right] = narrowWidth
        layout.select(agentTool, in: rightGroupID)
        try await Task.sleep(for: .milliseconds(200))
        agentCase["context_close"] = try await closeViaMenu(tool: agentTool, menuScreenshot: "agent-context-menu.png",
                                                            layout: layout, media: media, window: window)
        try await wait(seconds: 10, until: "Agent 进程退出") { !alive(agentPid) }
        agentCase["closed"] = layout.locateTool(agentTool) == nil
        agentCase["target_dead"] = !alive(agentPid)
        agentCase["background_alive"] = alive(backgroundPid)
        agentCase["agent_phase"] = String(describing: agent.agentPhase)
        cases["agent"] = agentCase

        return [
            "language": Bundle.main.preferredLocalizations.first ?? "",
            "wide_width": wideWidth,
            "narrow_width": narrowWidth,
            "background_pid": Int(backgroundPid),
            "cases": cases,
            "failures": failures,
        ]
    }
}
#endif
