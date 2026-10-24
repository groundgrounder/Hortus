import AppKit
import SwiftUI

@Observable @MainActor
final class ToolbarGeometry {
    var alignedOrigin: CGFloat = 0
    var compactOrigin: CGFloat = 0
    var trailingClearance: CGFloat = 0
}

/// 从工具栏的实际布局求水平留白，不假定红绿灯、按钮或系统间距的宽度。
/// 只响应视图布局；全屏显示、拖动和双击完全交给系统标题栏。
struct ToolbarProbe: NSViewRepresentable {
    let geometry: ToolbarGeometry
    func makeNSView(context: Context) -> Probe { Probe(geometry: geometry) }
    func updateNSView(_ nsView: Probe, context: Context) { nsView.scheduleMeasurement() }

    @MainActor final class Probe: NSView {
        let geometry: ToolbarGeometry
        private var pending = false
        init(geometry: ToolbarGeometry) {
            self.geometry = geometry
            super.init(frame: .zero)
            identifier = NSUserInterfaceItemIdentifier("editor-tabs-probe")
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
        override func viewDidMoveToWindow() { super.viewDidMoveToWindow(); scheduleMeasurement() }
        override func layout() { super.layout(); scheduleMeasurement() }
        override func setFrameOrigin(_ point: NSPoint) { super.setFrameOrigin(point); scheduleMeasurement() }
        override func setFrameSize(_ size: NSSize) { super.setFrameSize(size); scheduleMeasurement() }

        func scheduleMeasurement() {
            guard !pending else { return }
            pending = true
            // 系统提交本轮工具栏布局后再更新 SwiftUI，避免在布局过程中修改状态。
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                pending = false
                guard let window, let toolbar = window.toolbar, bounds.width > 0,
                      toolbar.items.contains(where: { $0.itemIdentifier.rawValue == "editor-tabs" }) else { return }
                let x = convert(bounds, to: nil).minX
                if let spacer = toolbar.items.first(where: { $0.itemIdentifier.rawValue == "editor-alignment" })?.view {
                    let origin = x - spacer.frame.width
                    if abs(geometry.alignedOrigin - origin) > 0.25 { geometry.alignedOrigin = origin }
                } else if abs(geometry.compactOrigin - x) > 0.25 { geometry.compactOrigin = x }
                let trailing = (toolbar.visibleItems ?? []).filter { $0.itemIdentifier.rawValue == "bottom-panel" || $0.itemIdentifier.rawValue == "right-panel" }
                    .compactMap(\.view).filter { $0.window === window }
                    .map { $0.convert($0.bounds, to: nil).minX }.min()
                if let trailing {
                    let clearance = window.frame.width - trailing + 20
                    if abs(geometry.trailingClearance - clearance) > 0.25 { geometry.trailingClearance = clearance }
                }
            }
        }
    }
}

/// 自检读取实际正文与面板坐标；不拦截输入。
struct ContentFrameProbe: NSViewRepresentable {
    let name: String
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        view.identifier = NSUserInterfaceItemIdentifier(name)
        return view
    }
    func updateNSView(_ nsView: NSView, context: Context) {}
}
