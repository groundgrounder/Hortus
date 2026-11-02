import AppKit
import CoreText
import Darwin

/// 所有循环、等待、离屏计量都只在显式验收参数下执行。
@MainActor
enum DiagramInspection {
    private final class WeakView {
        weak var value: NSView?
        init(_ value: NSView) { self.value = value }
    }
    static func textViews(in view: NSView) -> [DiagramTextView] {
        (view as? DiagramTextView).map { [$0] } ?? view.subviews.flatMap { textViews(in: $0) }
    }
    static func current(document: NoteDocument? = nil) -> DiagramTextView? {
        let views = NSApp.windows.filter(\.isVisible).compactMap(\.contentView).flatMap { textViews(in: $0) }
        if let document { return views.first { $0.representedDocument == ObjectIdentifier(document) } }
        if let view = NSApp.keyWindow?.contentView.flatMap({ textViews(in: $0).first }) { return view }
        // 显式单窗口检查可能在应用尚未取得 key 状态时开始；多个窗口时不猜测目标。
        return views.count == 1 ? views.first : nil
    }
    static func finishEvent() {
        // 自动循环没有原生输入事件；结束 AppKit 的事件临时对象池。
        if let event = NSEvent.otherEvent(with: .applicationDefined, location: .zero, modifierFlags: [],
                                         timestamp: 0, windowNumber: 0, context: nil, subtype: 0,
                                         data1: 0, data2: 0) {
            NSApp.postEvent(event, atStart: false)
        }
    }
    static func settled(source: String? = nil, document: NoteDocument? = nil) async throws -> DiagramTextView {
        for _ in 0..<500 {
            if let view = current(document: document), !view.diagramPending, source == nil || view.string == source {
                autoreleasepool { view.layoutSubtreeIfNeeded() }
                return view
            }
            try await Task.sleep(for: .milliseconds(10))
        }
        throw CocoaError(.coderValueNotFound)
    }
    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        if workspace.current?.document?.reading == true { await workspace.toggleCurrentView() }
        let memory = arguments.contains("--diagram-memory") ? try await memorySamples(workspace: workspace) : []
        var view = try await settled(source: workspace.current?.text)
        let firstMain = view.maximumMainWorkMilliseconds
        var beforeClick = 0
        if arguments.contains("--diagram-click") {
            guard let diagram = view.diagramViews.values.first, let window = diagram.window,
                  let hit = diagram.prepared.scene.hitRegions.first(where: \.canCollapse) else { throw CocoaError(.coderValueNotFound) }
            beforeClick = diagram.prepared.scene.hitRegions.count
            let point = diagram.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
            guard let event = NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [],
                                                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                                                context: nil, eventNumber: 0, clickCount: 1, pressure: 1) else { throw CocoaError(.coderValueNotFound) }
            diagram.mouseDown(with: event)
            view = try await settled()
        }
        var resize: [[String: Any]] = []
        if arguments.contains("--diagram-resize"), let window = view.window {
            let original = window.frame
            for index in 0..<60 {
                var frame = original
                frame.size.width = original.width - 200 + CGFloat(index % 30) * 10
                window.setFrame(frame, display: true)
                try await Task.sleep(for: .milliseconds(10))
                view = try await settled()
                if let block = view.blocks.first, let prepared = block.prepared, let diagram = view.diagramViews[block.ordinal] {
                    let visible = diagram.visibleRect.intersection(diagram.bounds)
                    let scale = window.backingScaleFactor
                    let draw = try drawMilliseconds(prepared, visible: visible, scale: scale)
                    resize.append(["width": prepared.scene.width, "height": prepared.scene.height, "layout_phases": view.layoutPhases, "render_ms": block.renderMilliseconds,
                                   "ffi_ms": block.transferMilliseconds, "install_ms": view.installMilliseconds,
                                   "rust_layout_ms": block.layoutMilliseconds,
                                   "install_phases": view.installPhases, "draw_ms": draw, "total_ms": block.renderMilliseconds + view.installMilliseconds + draw,
                                   "draw_width": visible.width, "draw_height": visible.height, "draw_scale": scale,
                                   "payload_bytes": prepared.payloadBytes])
                }
            }
            window.setFrame(original, display: true)
            try await Task.sleep(for: .milliseconds(20))
            view = try await settled()
        }
        var errors: [Double] = []
        let diagrams = view.diagramViews.values.sorted { $0.frame.minY < $1.frame.minY }
        var accessible = 0
        var expandedValid = true
        for diagram in diagrams {
            errors += diagram.prepared.lines.map { abs(CTLineGetTypographicBounds($0.1, nil, nil, nil) - $0.0.width) }
            let nodes = diagram.accessibilityChildren() as? [NSAccessibilityElement] ?? []
            accessible += nodes.count
            for (node, hit) in zip(nodes, diagram.prepared.scene.hitRegions) where hit.canCollapse {
                expandedValid = expandedValid && node.accessibilityRole() == .disclosureTriangle && node.isAccessibilityExpanded() == !hit.collapsed
            }
        }
        var customFragments = 0
        var errorFragments = 0
        if let manager = view.textLayoutManager, let content = manager.textContentManager {
            manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: []) { fragment in
                if let fragment = fragment as? NoteLayoutFragment, fragment.replacement != nil { customFragments += 1 }
                if let fragment = fragment as? NoteLayoutFragment, fragment.reason != nil { errorFragments += 1 }
                return true
            }
        }
        errors.sort()
        let hits = diagrams.reduce(0) { $0 + $1.prepared.scene.hitRegions.count }
        return ["diagrams": diagrams.count, "blocks": view.blocks.count,
                "dark": view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua,
                "custom_fragments": customFragments, "error_fragments": errorFragments,
                "source_preserved": view.string == workspace.current?.text, "textkit2": view.textLayoutManager != nil,
                "visible_nodes": hits, "accessibility_nodes": accessible, "accessibility_expanded_valid": expandedValid,
                "before_click_nodes": beforeClick,
                "collapsed_nodes": view.blocks.flatMap { Array($0.collapsed) }.count,
                "badge_count": diagrams.flatMap { $0.prepared.lines }.filter { $0.0.text.hasPrefix("+") }.count,
                "reasons": view.blocks.compactMap(\.reason),
                "width_max_px": errors.last ?? 0, "width_p95_px": errors.isEmpty ? 0 : errors[Int(Double(errors.count - 1) * 0.95)],
                "spans": errors.count, "first_main_work_max_ms": firstMain,
                "payload_bytes": diagrams.map { $0.prepared.payloadBytes },
                "resize": resize, "memory": memory]
    }
    private static func memorySamples(workspace: Workspace) async throws -> [[String: Any]] {
        guard let path = workspace.selected else { throw CocoaError(.coderValueNotFound) }
        var memory: [[String: Any]] = [], oldText: [WeakView] = [], oldDiagrams: [WeakView] = []
        var textIdentities: Set<ObjectIdentifier> = []
        for index in 0..<20 {
            autoreleasepool {
                if let view = current() {
                    textIdentities.insert(ObjectIdentifier(view))
                    oldText.append(WeakView(view))
                    oldDiagrams += view.diagramViews.values.map { WeakView($0) }
                }
            }
            // 每轮任务结束后再采样，避免自检异步栈持有上轮的原生视图。
            let closed = try await Task { @MainActor in
                workspace.closeTab(path)
                for _ in 0..<500 {
                    if !workspace.tabs.contains(where: { $0.path == path }) { break }
                    try await Task.sleep(for: .milliseconds(10))
                }
                guard !workspace.tabs.contains(where: { $0.path == path }) else { throw CocoaError(.coderValueNotFound) }
                finishEvent()
                try await Task.sleep(for: .milliseconds(200))
                let closed = try residentBytes()
                workspace.openNote(path)
                for _ in 0..<500 {
                    if workspace.current?.loading == false, current() != nil { break }
                    try await Task.sleep(for: .milliseconds(10))
                }
                if workspace.current?.document?.reading == true { await workspace.toggleCurrentView() }
                let view = try await settled(source: workspace.current?.text)
                autoreleasepool {
                    view.displayIfNeeded()
                    for diagram in view.diagramViews.values { diagram.displayIfNeeded() }
                }
                return closed
            }.value
            finishEvent()
            try await Task.sleep(for: .milliseconds(200))
            var usage = rusage()
            getrusage(RUSAGE_SELF, &usage)
            memory.append(["cycle": index + 1, "opened_bytes": try residentBytes(), "closed_bytes": closed,
                           "peak_bytes": usage.ru_maxrss, "text_view_instances": textIdentities.count,
                           "retained_old_text_views": oldText.filter { $0.value != nil && $0.value !== current() }.count,
                           "retained_old_diagram_views": oldDiagrams.filter { $0.value != nil }.count])
        }
        return memory
    }
    private static func drawMilliseconds(_ prepared: PreparedScene, visible: CGRect, scale: CGFloat) throws -> Double {
        guard !visible.isEmpty else { return 0 }
        // 与原生视图相同的可见区域、坐标和 backing scale，不为屏幕外的整张长图分配位图。
        guard let context = CGContext(data: nil, width: Int(ceil(visible.width * scale)), height: Int(ceil(visible.height * scale)),
                                      bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { throw CocoaError(.coderValueNotFound) }
        context.scaleBy(x: scale, y: scale)
        context.translateBy(x: -visible.minX, y: visible.height + visible.minY)
        context.scaleBy(x: 1, y: -1)
        context.clip(to: visible)
        let start = ProcessInfo.processInfo.systemUptime
        SceneRenderer.draw(prepared, in: context)
        return (ProcessInfo.processInfo.systemUptime - start) * 1000
    }
    private static func residentBytes() throws -> UInt64 {
        var info = proc_taskinfo()
        let size = Int32(MemoryLayout.size(ofValue: info))
        guard proc_pidinfo(getpid(), PROC_PIDTASKINFO, 0, &info, size) == size else { throw CocoaError(.fileReadUnknown) }
        return info.pti_resident_size
    }
}
