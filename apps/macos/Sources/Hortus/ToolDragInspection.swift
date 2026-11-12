#if DEBUG
import AppKit
import CoreGraphics

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

    @discardableResult
    static func drag(from sourceName: String, to targetName: String, window: NSWindow) async throws -> Bool {
        guard CGPreflightPostEventAccess() else { return false }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        try await wait { NSApp.isActive && window.isKeyWindow }
        guard let source = find(sourceName, in: window), let target = find(targetName, in: window) else { return false }
        let from = screenPoint(source, window: window)
        let to = screenPoint(target, window: window)
        try await Task.sleep(for: .milliseconds(100))
        await withCheckedContinuation { continuation in
            postDrag(from: from, to: to) { continuation.resume() }
        }
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
    static func cancel(from sourceName: String, window: NSWindow) async throws -> Bool {
        guard CGPreflightPostEventAccess() else { return false }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        try await wait { NSApp.isActive && window.isKeyWindow }
        guard let source = find(sourceName, in: window) else { return false }
        let from = screenPoint(source, window: window)
        try await Task.sleep(for: .milliseconds(100))
        await withCheckedContinuation { continuation in
            postCancel(from: from) { continuation.resume() }
        }
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
