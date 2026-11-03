import AppKit
import HortusCore

@MainActor
final class DiagramView: NSView {
    private(set) var prepared: PreparedScene
    var onToggle: ((String) -> Void)?
    var onSourceRange: ((NSRange) -> Void)?
    var onEditSource: (() -> Void)?
    private var hover: NSTrackingArea?
    private lazy var editButton: NSButton = {
        let button = NSButton(title: L10n.string("Edit Source"), target: self, action: #selector(editSource))
        button.bezelStyle = .inline
        button.font = .systemFont(ofSize: 12)
        button.isHidden = true
        addSubview(button)
        return button
    }()
    private var nodes: [DiagramAccessibilityNode] = []
    private var edges: [DiagramAccessibilityEdge] = []
    override var isFlipped: Bool { true }

    init(prepared: PreparedScene) {
        self.prepared = prepared
        super.init(frame: NSRect(x: 0, y: 0, width: prepared.scene.width, height: prepared.scene.height))
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(L10n.string("Mermaid diagram"))
        update(prepared)
    }
    func update(_ prepared: PreparedScene) {
        self.prepared = prepared
        let hits = prepared.scene.hitRegions
        let sameContent = nodes.count == hits.count && zip(nodes, hits).allSatisfy { node, hit in
            node.hit.nodeId == hit.nodeId && node.hit.label == hit.label && node.hit.prefix == hit.prefix
                && node.hit.canCollapse == hit.canCollapse && node.hit.collapsed == hit.collapsed
        }
        if sameContent { for (node, hit) in zip(nodes, hits) { node.hit = hit } }
        else {
            // 折叠后保留仍可见节点的对象，让辅助技术继续持有当前焦点。
            let previous = Dictionary(uniqueKeysWithValues: nodes.map { ($0.hit.nodeId, $0) })
            nodes = hits.map { hit in
                let node = previous[hit.nodeId] ?? DiagramAccessibilityNode(hit: hit, owner: self)
                node.update(hit)
                return node
            }
            if !previous.isEmpty { NSAccessibility.post(element: self, notification: .layoutChanged) }
        }
        let previousEdges = Dictionary(uniqueKeysWithValues: edges.map { ($0.id, $0) })
        edges = prepared.scene.primitives.compactMap { primitive in
            guard case let .graphPath(id, _, points, _, _, _, _, _, _, _, _, _) = primitive,
                  let first = points.first else { return nil }
            let bounds = points.dropFirst().reduce(CGRect(origin: first.cgPoint, size: .zero)) {
                $0.union(CGRect(origin: $1.cgPoint, size: .zero))
            }.insetBy(dx: -5, dy: -5)
            let edge = previousEdges[id] ?? DiagramAccessibilityEdge(id: id, owner: self)
            edge.update(label: prepared.edgeLabels[id] ?? "", bounds: bounds)
            return edge
        }
        needsDisplay = true
        window?.invalidateCursorRects(for: self)
    }
    override func accessibilityChildren() -> [Any]? { nodes + edges.map { $0 as NSAccessibilityElement } }
    var accessibilityNodeCount: Int { nodes.count }
    var accessibilityEdgeCount: Int { edges.count }
    func screenPoint(_ point: CGPoint) -> ScenePoint {
        let value = window?.convertPoint(toScreen: convert(point, to: nil)) ?? point
        return ScenePoint(x: value.x, y: value.y)
    }
    func viewportAnchors() -> [DiagramViewportAnchor] {
        prepared.scene.hitRegions.filter { $0.rect.cgRect.intersects(visibleRect) }.map {
            DiagramViewportAnchor(nodeId: $0.nodeId, screenCenter: screenPoint(CGPoint(x: $0.rect.cgRect.midX, y: $0.rect.cgRect.midY)))
        }
    }
    func detach() {
        nodes = []
        edges = []
        onToggle = nil
        onEditSource = nil
        onSourceRange = nil
        removeFromSuperview()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hover { removeTrackingArea(hover) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(area); hover = area
        editButton.frame = NSRect(x: max(0, bounds.width - 86), y: 8, width: 76, height: 24)
    }
    /// 只有编辑视图里、笔记可写时才给"编辑源码"：阅读视图和只读笔记里点了也没法改
    var canEditSource: Bool {
        guard onEditSource != nil, let host = superview as? DiagramTextView else { return false }
        return !host.isReading && host.isEditable
    }
    override func mouseEntered(with event: NSEvent) { editButton.isHidden = !canEditSource }
    override func mouseExited(with event: NSEvent) { editButton.isHidden = true }
    @objc private func editSource() { onEditSource?() }
    override func draw(_ dirtyRect: NSRect) {
        let started = ProcessInfo.processInfo.systemUptime
        defer { (superview as? DiagramTextView)?.recordMainWork(since: started) }
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        SceneRenderer.draw(prepared, in: context)
    }
    override func resetCursorRects() {
        for hit in prepared.scene.hitRegions where hit.canCollapse { addCursorRect(hit.rect.cgRect, cursor: .pointingHand) }
    }
    func sourceRange(at point: CGPoint) -> NSRange? {
        guard prepared.acceptsInteraction else { return nil }
        if let hit = prepared.scene.hitRegions.first(where: { $0.rect.cgRect.contains(point) }) { return prepared.sourceRanges[hit.nodeId] }
        for primitive in prepared.scene.primitives.reversed() {
            if case let .labelBackground(id, rect, _) = primitive, rect.cgRect.contains(point) { return prepared.edgeRanges[id] }
            if case let .graphPath(id, _, points, _, _, _, _, _, _, _, _, _) = primitive {
                for (a, b) in zip(points, points.dropFirst()) {
                    let dx = b.x - a.x, dy = b.y - a.y, length = dx * dx + dy * dy
                    guard length > 0 else { continue }
                    let t = min(1, max(0, ((point.x - a.x) * dx + (point.y - a.y) * dy) / length))
                    if hypot(point.x - a.x - t * dx, point.y - a.y - t * dy) <= 5 { return prepared.edgeRanges[id] }
                }
            }
        }
        return nil
    }
    override func mouseDown(with event: NSEvent) {
        guard prepared.acceptsInteraction else { return }
        let point = convert(event.locationInWindow, from: nil)
        if let hit = prepared.scene.hitRegions.first(where: { $0.canCollapse && $0.rect.cgRect.contains(point) }) {
            onToggle?(hit.nodeId)
        } else if let range = sourceRange(at: point) { onSourceRange?(range) }
    }
}

private final class DiagramAccessibilityEdge: NSAccessibilityElement {
    let id: UInt64
    private var bounds = CGRect.zero
    private let parentProvider: @MainActor @Sendable () -> DiagramView?
    @MainActor init(id: UInt64, owner: DiagramView) {
        self.id = id
        parentProvider = { [weak owner] in owner }
        super.init()
        setAccessibilityRole(.button)
    }
    func update(label: String, bounds: CGRect) {
        self.bounds = bounds
        setAccessibilityLabel(label)
    }
    override func accessibilityParent() -> Any? {
        let provider = parentProvider
        return MainActor.assumeIsolated { provider() }
    }
    override func accessibilityFrame() -> NSRect {
        let provider = parentProvider
        let rect = bounds
        return MainActor.assumeIsolated {
            guard let owner = provider(), let window = owner.window else { return .zero }
            return window.convertToScreen(owner.convert(rect, to: nil))
        }
    }
    override func accessibilityPerformPress() -> Bool {
        let provider = parentProvider
        let edgeID = id
        return MainActor.assumeIsolated {
            guard let owner = provider(), let range = owner.prepared.edgeRanges[edgeID],
                  owner.prepared.acceptsInteraction, let action = owner.onSourceRange else { return false }
            action(range)
            return true
        }
    }
}

/// 不可变回调将 AppKit 的同步无障碍入口转回主执行器。
private final class DiagramAccessibilityNode: NSAccessibilityElement {
    var hit: DiagramHit
    private let parentProvider: @MainActor @Sendable () -> DiagramView?
    private let frameProvider: @MainActor @Sendable (SceneRect) -> NSRect
    private let press: @MainActor @Sendable (String) -> Bool
    @MainActor init(hit: DiagramHit, owner: DiagramView) {
        self.hit = hit
        parentProvider = { [weak owner] in owner }
        frameProvider = { [weak owner] rect in
            guard let owner, let window = owner.window else { return .zero }
            return window.convertToScreen(owner.convert(rect.cgRect, to: nil))
        }
        press = { [weak owner] id in
            guard let owner, owner.prepared.acceptsInteraction else { return false }
            if let hit = owner.prepared.scene.hitRegions.first(where: { $0.nodeId == id }), hit.canCollapse {
                owner.onToggle?(id)
            } else if let range = owner.prepared.sourceRanges[id], owner.onSourceRange != nil { owner.onSourceRange?(range) }
            else { return false }
            return true
        }
        super.init()
        update(hit)
    }
    func update(_ hit: DiagramHit) {
        let changed = self.hit.collapsed != hit.collapsed
        self.hit = hit
        setAccessibilityRole(hit.canCollapse ? .disclosureTriangle : .staticText)
        setAccessibilityLabel(hit.accessibilityLabel)
        if hit.canCollapse { setAccessibilityValue(hit.collapsed ? 0 : 1) }
        if changed { NSAccessibility.post(element: self, notification: .valueChanged) }
    }
    override func accessibilityParent() -> Any? {
        let provider = parentProvider
        return MainActor.assumeIsolated { provider() }
    }
    override func accessibilityFrame() -> NSRect {
        let provider = frameProvider
        let rect = hit.rect
        return MainActor.assumeIsolated { provider(rect) }
    }
    override func isAccessibilityExpanded() -> Bool { !hit.collapsed }
    override func accessibilityPerformPress() -> Bool {
        let action = press
        let id = hit.nodeId
        return MainActor.assumeIsolated { action(id) }
    }
    override func isAccessibilitySelectorAllowed(_ selector: Selector) -> Bool {
        if selector == #selector(isAccessibilityExpanded) { return hit.canCollapse }
        if selector == #selector(accessibilityPerformPress) { return true }
        return super.isAccessibilitySelectorAllowed(selector)
    }
}
