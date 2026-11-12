#if DEBUG
import AppKit
import CoreGraphics
import SwiftUI

/// 连续拖动的结构化诊断：源/目标、坐标、DragSession phase 与落点状态。
/// 只在 Debug 构建里存在，随自检结果或失败转储输出。
@MainActor
enum DragInspection {
    private static var events: [[String: Any]] = []
    private static var stepName = ""
    private static let clock = ProcessInfo.processInfo.systemUptime
    /// 已观察到的拖动会话；键是会话标识，值是按顺序收到的 phase。
    static private(set) var sessionPhases: [String: [String]] = [:]

    static func begin(_ step: String, source: String, target: String, window: NSWindow,
                      sourceView: NSView?, targetView: NSView?) {
        stepName = step
        append(["event": "step_begin", "source": source, "target": target,
                "window": window.windowNumber, "key": window.isKeyWindow, "active": NSApp.isActive,
                "source_found": sourceView != nil, "target_found": targetView != nil])
        record(frame: "source_frame", view: sourceView, window: window)
        record(frame: "target_frame", view: targetView, window: window)
    }

    private static func record(frame name: String, view: NSView?, window: NSWindow) {
        guard let view else { return }
        let rect = window.convertToScreen(view.convert(view.bounds, to: nil))
        let height = CGDisplayBounds(CGMainDisplayID()).height
        append(["event": name,
                "window_rect": [rect.minX, rect.minY, rect.width, rect.height],
                "screen_point": [rect.midX, height - rect.midY],
                "hidden": view.isHiddenOrHasHiddenAncestor])
    }

    static func append(_ value: [String: Any]) {
        var entry = value
        entry["t_ms"] = Int((ProcessInfo.processInfo.systemUptime - clock) * 1000)
        entry["step"] = stepName
        events.append(entry)
    }

    static func recordDrag(_ session: DragSession, tool: ToolID, groupID: UUID,
                           before: ToolID?, after: ToolID?) {
        let key = String(describing: session.id)
        let phase = String(describing: session.phase)
        let previous = sessionPhases[key, default: []]
        // 同一会话的 active 只留第一条；真正要看的是会话身份与 phase 的先后。
        if phase.hasPrefix("active"), previous.contains(where: { $0.hasPrefix("active") }) { return }
        sessionPhases[key] = previous + [phase]
        append(["event": "drag_phase", "session": key, "phase": phase,
                "tool": tool.id, "group": groupID.uuidString,
                "dragging_before": before?.id ?? "nil", "dragging_after": after?.id ?? "nil"])
    }

    static func recordDrop(zone: String, session: DropSession, enabled: Bool, targeted: Bool,
                           dragging: ToolID?) {
        let phase = String(describing: session.phase)
        // 每次移动都会重复 active；只有 enabled 变化时才值得记录。
        if phase.hasPrefix("active"), enabled { return }
        append(["event": "drop_phase", "zone": zone, "phase": phase,
                "enabled": enabled, "targeted": targeted,
                "dragging": dragging?.id ?? "nil"])
    }

    static func recordDropAccepted(zone: String, tool: ToolID) {
        append(["event": "drop_accepted", "zone": zone, "tool": tool.id])
    }

    static func recordDropRejected(zone: String) {
        append(["event": "drop_rejected", "zone": zone])
    }

    static func note(_ event: String, _ value: [String: Any] = [:]) {
        append(["event": event].merging(value) { current, _ in current })
    }

    static func dump() -> [[String: Any]] { events }

    /// 每个拖动会话实际收到的 phase 顺序。
    static func sessionSummary() -> [String: [String]] { sessionPhases }
}

/// 真实鼠标事件的工具标签拖动，只由显式自检参数启动。
/// 用探针名字定位源标签和目标组，不直接调用布局函数。
@MainActor
enum ToolDragInspection {
    static func find(_ name: String, in window: NSWindow) -> NSView? {
        guard let content = window.contentView else { return nil }
        return find(name, in: content)
    }

    private static func find(_ name: String, in view: NSView) -> NSView? {
        if view.identifier?.rawValue == name { return view }
        return view.subviews.lazy.compactMap { find(name, in: $0) }.first
    }

    private static func wait(_ condition: () -> Bool) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + 10
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else { throw CocoaError(.coderValueNotFound) }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    private static func screenPoint(_ view: NSView, window: NSWindow) -> CGPoint {
        let rect = window.convertToScreen(view.convert(view.bounds, to: nil))
        let height = CGDisplayBounds(CGMainDisplayID()).height
        return CGPoint(x: rect.midX, y: height - rect.midY)
    }

    /// 落点动画会改变 frame；等到连续两次采样一致再计算坐标。
    private static func waitStable(_ view: NSView?, in window: NSWindow) async throws {
        guard let view else { return }
        var previous = view.convert(view.bounds, to: nil)
        for _ in 0..<20 {
            try await Task.sleep(for: .milliseconds(16))
            let current = view.convert(view.bounds, to: nil)
            if current == previous { return }
            previous = current
        }
    }

    /// 刚跨区的标签会先保留旧位置；等它真正进入当前组标签条并稳定后再按真实坐标拖动。
    private static func waitSettled(_ sourceName: String, in groupName: String,
                                    window: NSWindow) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + 10
        while ProcessInfo.processInfo.systemUptime < deadline {
            if let source = find(sourceName, in: window), let group = find(groupName, in: window) {
                let sourceRect = source.convert(source.bounds, to: nil)
                let groupRect = group.convert(group.bounds, to: nil)
                let center = NSPoint(x: sourceRect.midX, y: sourceRect.midY)
                if groupRect.contains(center) && !source.isHiddenOrHasHiddenAncestor {
                    try await Task.sleep(for: .milliseconds(32))
                    if let again = find(sourceName, in: window),
                       again.convert(again.bounds, to: nil) == sourceRect {
                        return
                    }
                }
            }
            try await Task.sleep(for: .milliseconds(16))
        }
        throw NSError(domain: "Hortus.ToolDragInspection", code: 1,
                      userInfo: [NSLocalizedDescriptionKey: "等待超时（10 s）：\(sourceName) 未进入 \(groupName)"])
    }

    @discardableResult
    static func drag(from sourceName: String, to targetName: String, containedIn groupName: String? = nil,
                     window: NSWindow, step: String = "drag") async throws -> Bool {
        guard CGPreflightPostEventAccess() else { return false }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        try await wait { NSApp.isActive && window.isKeyWindow }
        if let groupName {
            try await waitSettled(sourceName, in: groupName, window: window)
        }
        let source = find(sourceName, in: window)
        let target = find(targetName, in: window)
        DragInspection.begin(step, source: sourceName, target: targetName, window: window,
                             sourceView: source, targetView: target)
        guard let source, let target else { return false }
        try await waitStable(source, in: window)
        try await waitStable(target, in: window)
        let from = screenPoint(source, window: window)
        let to = screenPoint(target, window: window)
        try await Task.sleep(for: .milliseconds(100))
        await withCheckedContinuation { continuation in
            postDrag(from: from, to: to) { continuation.resume() }
        }
        DragInspection.note("drag_posted")
        try await Task.sleep(for: .milliseconds(250))
        return true
    }

    /// 拖到窗口里没有放置目标的位置（标题栏），用于验证无效落点。
    @discardableResult
    static func drag(from sourceName: String, to point: CGPoint, containedIn groupName: String? = nil,
                     window: NSWindow, step: String = "invalid-drop") async throws -> Bool {
        guard CGPreflightPostEventAccess() else { return false }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        try await wait { NSApp.isActive && window.isKeyWindow }
        if let groupName {
            try await waitSettled(sourceName, in: groupName, window: window)
        }
        let source = find(sourceName, in: window)
        DragInspection.begin(step, source: sourceName, target: "screen-point", window: window,
                             sourceView: source, targetView: nil)
        guard let source else { return false }
        try await waitStable(source, in: window)
        let from = screenPoint(source, window: window)
        DragInspection.note("target_point", ["x": point.x, "y": point.y])
        try await Task.sleep(for: .milliseconds(100))
        await withCheckedContinuation { continuation in
            postDrag(from: from, to: point) { continuation.resume() }
        }
        DragInspection.note("drag_posted")
        try await Task.sleep(for: .milliseconds(250))
        return true
    }

    /// 点击某个探针（用于打开标签条尾部的 + 菜单）。
    @discardableResult
    static func click(_ name: String, window: NSWindow) async throws -> Bool {
        guard CGPreflightPostEventAccess() else { return false }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        try await wait { NSApp.isActive && window.isKeyWindow }
        guard let view = find(name, in: window) else { return false }
        let point = screenPoint(view, window: window)
        try await Task.sleep(for: .milliseconds(100))
        await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                post(.mouseMoved, point)
                Thread.sleep(forTimeInterval: 0.1)
                post(.leftMouseDown, point)
                Thread.sleep(forTimeInterval: 0.08)
                post(.leftMouseUp, point)
                continuation.resume()
            }
        }
        return true
    }

    static func pressEscape() {
        for down in [true, false] {
            CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: down)?.post(tap: .cghidEventTap)
        }
    }

    /// 拖到无效落点后按 Escape，布局不应变化，标签也不能丢。
    @discardableResult
    static func cancel(from sourceName: String, containedIn groupName: String? = nil, window: NSWindow,
                       step: String = "cancel") async throws -> Bool {
        guard CGPreflightPostEventAccess() else { return false }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        try await wait { NSApp.isActive && window.isKeyWindow }
        if let groupName {
            try await waitSettled(sourceName, in: groupName, window: window)
        }
        let source = find(sourceName, in: window)
        DragInspection.begin(step, source: sourceName, target: "", window: window,
                             sourceView: source, targetView: nil)
        guard let source else { return false }
        let from = screenPoint(source, window: window)
        try await Task.sleep(for: .milliseconds(100))
        await withCheckedContinuation { continuation in
            postCancel(from: from) { continuation.resume() }
        }
        DragInspection.note("cancel_posted")
        try await Task.sleep(for: .milliseconds(800))
        return true
    }

    nonisolated private static func postDrag(from: CGPoint, to: CGPoint, completion: @escaping @Sendable () -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            post(.mouseMoved, from)
            Thread.sleep(forTimeInterval: 0.1)
            post(.leftMouseDown, from)
            Thread.sleep(forTimeInterval: 0.15)
            for step in 1...30 {
                let fraction = CGFloat(step) / 30
                post(.leftMouseDragged, CGPoint(x: from.x + (to.x - from.x) * fraction,
                                                y: from.y + (to.y - from.y) * fraction))
                Thread.sleep(forTimeInterval: 0.025)
            }
            Thread.sleep(forTimeInterval: 0.3)
            post(.leftMouseUp, to)
            completion()
        }
    }

    nonisolated private static func postCancel(from: CGPoint, completion: @escaping @Sendable () -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            post(.mouseMoved, from)
            Thread.sleep(forTimeInterval: 0.1)
            post(.leftMouseDown, from)
            Thread.sleep(forTimeInterval: 0.1)
            for step in 1...12 {
                post(.leftMouseDragged, CGPoint(x: from.x + CGFloat(step) * 2, y: from.y + CGFloat(step) * 2))
                Thread.sleep(forTimeInterval: 0.025)
            }
            if let escape = CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: true) {
                escape.post(tap: .cghidEventTap)
            }
            Thread.sleep(forTimeInterval: 0.2)
            post(.leftMouseUp, CGPoint(x: from.x + 24, y: from.y + 24))
            completion()
        }
    }

    nonisolated private static func post(_ type: CGEventType, _ point: CGPoint) {
        guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else { return }
        event.post(tap: .cghidEventTap)
    }
}
#endif
