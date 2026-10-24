import AppKit

/// 开启“键盘导航”时，SwiftUI 在工具栏内容变化后（例如打开知识库）会把键盘焦点自动放到第一个工具栏按钮上，
/// 按钮带一圈焦点环。工具栏只在用户主动导航（Tab、⌃ 功能键或点击）时获得焦点；其余情况交回正文或窗口。
@MainActor
final class ToolbarFocusGuard {
    private weak var window: NSWindow?
    private var observation: NSKeyValueObservation?
    private var pending = false

    init(window: NSWindow) {
        self.window = window
        observation = window.observe(\.firstResponder, options: [.old, .new]) { [weak self] _, change in
            // 焦点只在主线程变化。
            let old = change.oldValue ?? nil
            MainActor.assumeIsolated { self?.firstResponderChanged(from: old) }
        }
        firstResponderChanged(from: nil)
    }

    private func firstResponderChanged(from old: NSResponder?) {
        guard let window, isInToolbar(window.firstResponder, of: window), !userNavigated(), !pending else { return }
        let previous = (old as? NSView).flatMap { view in
            view.window === window && !isInToolbar(view, of: window) ? view : nil
        }
        // 不在 KVO 回调里改变响应者链，避免重入。
        pending = true
        DispatchQueue.main.async { [weak self] in
            guard let self, let window = self.window else { return }
            self.pending = false
            guard self.isInToolbar(window.firstResponder, of: window) else { return }
            let target = previous ?? Self.editor(in: window)
            if !(target.map { window.makeFirstResponder($0) } ?? false) { window.makeFirstResponder(nil) }
        }
    }

    private func userNavigated() -> Bool {
        guard let event = NSApp.currentEvent else { return false }
        switch event.type {
        case .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp, .otherMouseDown, .otherMouseUp:
            return true
        case .keyDown:
            // Tab / ⇧Tab，以及 ⌃F2、⌃F5 等系统的焦点移动快捷键。
            return event.keyCode == 48 || event.modifierFlags.contains(.control)
        default:
            return false
        }
    }

    private func isInToolbar(_ responder: NSResponder?, of window: NSWindow) -> Bool {
        guard let view = responder as? NSView else { return false }
        return window.toolbar?.items.contains { item in item.view.map { view.isDescendant(of: $0) } ?? false } ?? false
    }

    private static func editor(in window: NSWindow) -> NSView? {
        func find(_ view: NSView) -> DiagramTextView? {
            if let text = view as? DiagramTextView, !text.isHiddenOrHasHiddenAncestor { return text }
            return view.subviews.lazy.compactMap(find).first
        }
        return window.contentView.flatMap(find)
    }
}
