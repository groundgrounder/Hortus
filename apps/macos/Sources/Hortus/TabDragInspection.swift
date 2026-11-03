#if DEBUG
import AppKit

/// 同库专项通过真实鼠标事件走标签的拖拽入口。
@MainActor
enum TabDragInspection {
    static func run(_ application: ApplicationWorkspace, output: String) async throws {
        let first = application.initial
        for _ in 0..<500 {
            if first.window != nil, !first.workspace.scanning, first.workspace.current?.document != nil { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        guard let window = first.window,
              let source = window.toolbar?.items.first(where: { $0.itemIdentifier.rawValue == "editor-tabs" })?.view,
              let screen = NSScreen.main?.visibleFrame else { throw CocoaError(.coderValueNotFound) }
        window.setFrame(NSRect(x: screen.minX, y: screen.maxY - 650, width: 900, height: 650), display: true)
        application.activate(first)
        try await Task.sleep(for: .milliseconds(300))
        let rect = window.convertToScreen(source.convert(source.bounds, to: nil))
        let from = CGPoint(x: rect.minX + 40, y: rect.midY)
        let to = CGPoint(x: screen.maxX - 60, y: screen.minY + 60)
        await withCheckedContinuation { continuation in
            sendDrag(from: from, to: to) { continuation.resume() }
        }
        try await Task.sleep(for: .milliseconds(700))
        let result: [String: Any] = ["real_cgevent_drag": true, "window_count": application.records.count,
                                    "source_point": NSStringFromPoint(from), "release_point": NSStringFromPoint(to),
                                    "tabs": application.records.map { $0.workspace.tabs.map(\.path) }]
        try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            .write(to: URL(fileURLWithPath: output))
        guard application.records.count == 2 else { throw CocoaError(.coderValueNotFound) }
        NSApp.terminate(nil)
    }

    nonisolated private static func sendDrag(from: CGPoint, to: CGPoint, completion: @escaping @Sendable () -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            let top = CGDisplayBounds(CGMainDisplayID()).height
            func post(_ type: CGEventType, _ point: CGPoint) {
                CGEvent(mouseEventSource: nil, mouseType: type,
                        mouseCursorPosition: CGPoint(x: point.x, y: top - point.y), mouseButton: .left)?.post(tap: .cghidEventTap)
            }
            post(.mouseMoved, from); Thread.sleep(forTimeInterval: 0.1)
            post(.leftMouseDown, from); Thread.sleep(forTimeInterval: 0.15)
            for step in 1...30 {
                let fraction = CGFloat(step) / 30
                post(.leftMouseDragged, CGPoint(x: from.x + (to.x - from.x) * fraction, y: from.y + (to.y - from.y) * fraction))
                Thread.sleep(forTimeInterval: 0.025)
            }
            post(.leftMouseUp, to)
            completion()
        }
    }
}
#endif
