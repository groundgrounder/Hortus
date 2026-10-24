import AppKit
import Darwin

/// 仅由命令行验收参数调用，不在正常运行中设置计时器或采样线程。
@MainActor
enum WorkspaceInspection {
    static func checkLifecycle(_ workspace: Workspace, output: String) async {
        do {
            let paths = ProcessInfo.processInfo.arguments.filter { $0.hasPrefix("--cycle-vault=") }
                .map { String($0.dropFirst("--cycle-vault=".count)) }
            guard paths.count >= 2 else { throw CocoaError(.fileReadInvalidFileName) }
            var openCounts: [Int] = []
            var closedCounts: [Int] = []
            var switchCounts: [Int] = []
            for index in 0..<20 {
                await workspace.openFolder(paths[index % paths.count])
                try await Task.sleep(for: .milliseconds(150))
                guard workspace.error == nil, workspace.root != nil, !workspace.scanning else {
                    throw CocoaError(.fileReadUnknown)
                }
                openCounts.append(try threadCount())
                // 同一次打开中再切换库，覆盖旧批次已排入主队列的情形。
                await workspace.openFolder(paths[(index + 1) % paths.count])
                try await Task.sleep(for: .milliseconds(150))
                switchCounts.append(try threadCount())
                await workspace.closeFolder()
                try await Task.sleep(for: .milliseconds(150))
                guard workspace.root == nil, workspace.entries.isEmpty else { throw CocoaError(.fileReadUnknown) }
                closedCounts.append(try threadCount())
            }
            let result: [String: Any] = [
                "cycles": 20, "open_threads": openCounts, "switch_threads": switchCounts,
                "closed_threads": closedCounts,
            ]
            let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            try await Task.detached { try data.write(to: URL(fileURLWithPath: output)) }.value
            NSApp.terminate(nil)
        } catch { fputs("Hortus 库切换自检失败：\(error)\n", stderr); exit(1) }
    }

    private static func threadCount() throws -> Int {
        var info = proc_taskinfo()
        let size = Int32(MemoryLayout.size(ofValue: info))
        guard proc_pidinfo(getpid(), PROC_PIDTASKINFO, 0, &info, size) == size else {
            throw CocoaError(.fileReadUnknown)
        }
        return Int(info.pti_threadnum)
    }
}

/// 从实际 AppKit 视图树取证，不依赖 SwiftUI 声明中的 placement 推断。
@MainActor
enum ToolbarInspection {
    static func exerciseLayout(window: NSWindow, layout: Layout) async throws -> [[String: Any]] {
        let originalWidth = layout.size[.left]
        let originalRight = layout.visible[.right]
        let originalFrame = window.frame
        defer {
            layout.size[.left] = originalWidth
            layout.visible[.right] = originalRight
            window.setFrame(originalFrame, display: true)
        }
        var samples: [[String: Any]] = []
        for width: CGFloat in [180, 260, 380, 480, 260] {
            layout.size[.left] = width
            try await Task.sleep(for: .milliseconds(250))
            samples.append(measure(window: window, layout: layout))
        }
        layout.visible[.right] = false
        var frame = originalFrame
        frame.size.width = 900
        window.setFrame(frame, display: true)
        try await Task.sleep(for: .milliseconds(250))
        samples.append(measure(window: window, layout: layout))
        return samples
    }

    static func measure(window: NSWindow, layout: Layout) -> [String: Any] {
        func descendants(_ view: NSView) -> [NSView] {
            [view] + view.subviews.flatMap { descendants($0) }
        }
        func frame(_ view: NSView) -> NSRect {
            let local = view.convert(view.bounds, to: nil)
            guard let host = view.window else { return local }
            return window.convertFromScreen(host.convertToScreen(local))
        }
        guard let content = window.contentView else { return ["found": false] }
        let contentViews = descendants(content)
        let column = contentViews.first { $0.identifier?.rawValue == "editor-column" }.map(frame)
        let panel = contentViews.first { $0.identifier?.rawValue == "left-panel" }.map(frame)
        var result: [String: Any] = [
            "found": false,
            "fullscreen": window.styleMask.contains(.fullScreen),
            "content_safe_top": content.safeAreaInsets.top,
            "content_frame": NSStringFromRect(frame(content)),
            "column_x": column?.minX ?? -1,
            "column_right": column?.maxX ?? -1,
            "column_top": column?.maxY ?? -1,
            "panel_top": panel?.maxY ?? -1,
            "window_width": window.frame.width,
            "window_height": window.frame.height,
            "presentation_options": NSApp.currentSystemPresentationOptions.rawValue,
            "first_responder": window.firstResponder.map { String(describing: type(of: $0)) } ?? "nil",
            "first_responder_in_content": (window.firstResponder as? NSView)?.isDescendant(of: content) ?? false,
            // 工具栏按钮的焦点环属于工具栏视图层级，不在内容视图中。
            "first_responder_in_toolbar": (window.firstResponder as? NSView).map { view in
                window.toolbar?.items.contains { item in item.view.map { view.isDescendant(of: $0) } ?? false } ?? false
            } ?? false,
            "mouse": NSStringFromPoint(NSEvent.mouseLocation),
        ]
        guard let item = window.toolbar?.items.first(where: { $0.itemIdentifier.rawValue == "editor-tabs" }),
              let itemView = item.view,
              let probe = descendants(itemView).first(where: { $0.identifier?.rawValue == "editor-tabs-probe" }) else { return result }
        var ancestors: [String] = []
        var parent = probe.superview
        while let view = parent {
            ancestors.append(String(describing: type(of: view)))
            parent = view.superview
        }
        let tabFrame = frame(probe)
        let hostFrame = probe.window?.frame ?? .zero
        result.merge([
            "found": true,
            "in_toolbar": probe.isDescendant(of: itemView),
            "in_content": probe.isDescendant(of: content),
            "ancestors": ancestors,
            "item": item.itemIdentifier.rawValue,
            "tabs_x": tabFrame.minX, "tabs_width": tabFrame.width, "tabs_y": tabFrame.minY,
            "toolbar_host_frame": NSStringFromRect(hostFrame),
            "toolbar_intersects_content": hostFrame.intersection(window.frame).height > 0,
            "sidebar_button_right": window.toolbar?.items.first?.view.map { frame($0).maxX } ?? -1,
            "panel_buttons_x": window.toolbar?.items.filter { $0.itemIdentifier.rawValue == "bottom-panel" || $0.itemIdentifier.rawValue == "right-panel" }
                .compactMap(\.view).map { frame($0).minX }.min() ?? -1,
            "panel_buttons_visible": window.toolbar?.visibleItems?.filter { $0.itemIdentifier.rawValue == "bottom-panel" || $0.itemIdentifier.rawValue == "right-panel" }.count == 2,
        ]) { _, value in value }
        return result
    }
}
