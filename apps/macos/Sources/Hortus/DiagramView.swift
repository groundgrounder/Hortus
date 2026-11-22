import AppKit
import HortusCore

@MainActor
final class DiagramView: NSView {
    private(set) var prepared: PreparedScene
    var onToggle: ((String) -> Void)?
    var onSourceRange: ((NSRange) -> Void)?
    var onEditSource: (() -> Void)?
    var onFocus: (() -> Void)?
    var highlightedNodeID: String? {
        didSet { if oldValue != highlightedNodeID { needsDisplay = true } }
    }
    private var hover: NSTrackingArea?
    private lazy var focusButton: NSButton = {
        let button = NSButton(title: L10n.string("Focus"), target: self, action: #selector(enterFocus))
        button.bezelStyle = .inline
        button.font = .systemFont(ofSize: 12)
        button.isHidden = true
        addSubview(button)
        return button
    }()
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
            // 场景可能为镜像胶囊复用 nodeId；这里必须去重，不能让运行时陷阱终结应用。
            let previous = Dictionary(nodes.map { ($0.hit.nodeId, $0) },
                                      uniquingKeysWith: { first, _ in first })
            nodes = hits.map { hit in
                let node = previous[hit.nodeId] ?? DiagramAccessibilityNode(hit: hit, owner: self)
                node.update(hit)
                return node
            }
            if !previous.isEmpty { NSAccessibility.post(element: self, notification: .layoutChanged) }
        }
        let previousEdges = Dictionary(edges.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let labelBounds: [UInt64: CGRect] = Dictionary(prepared.scene.primitives.compactMap { primitive -> (UInt64, CGRect)? in
            guard case let .labelBackground(id, rect, _) = primitive else { return nil }
            return (id, rect.cgRect)
        }, uniquingKeysWith: { first, _ in first })
        edges = prepared.scene.primitives.compactMap { primitive -> DiagramAccessibilityEdge? in
            guard case let .graphPath(id, _, points, _, _, _, _, _, _, _, _, _) = primitive,
                  let first = points.first else { return nil }
            let pathBounds = points.dropFirst().reduce(CGRect(origin: first.cgPoint, size: .zero)) {
                $0.union(CGRect(origin: $1.cgPoint, size: .zero))
            }.insetBy(dx: -5, dy: -5)
            let edge = previousEdges[id] ?? DiagramAccessibilityEdge(id: id, owner: self)
            edge.update(label: prepared.edgeLabels[id] ?? "", bounds: labelBounds[id] ?? pathBounds)
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
        onFocus = nil
        highlightedNodeID = nil
        removeFromSuperview()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hover { removeTrackingArea(hover) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(area); hover = area
        let editWidth: CGFloat = 76
        let focusWidth: CGFloat = 62
        editButton.frame = NSRect(x: max(0, bounds.width - (onFocus == nil ? editWidth + 10 : editWidth + focusWidth + 18)),
                                  y: 8, width: editWidth, height: 24)
        focusButton.frame = NSRect(x: max(0, bounds.width - focusWidth - 10), y: 8, width: focusWidth, height: 24)
    }
    /// 只有编辑视图里、笔记可写时才给"编辑源码"：阅读视图和只读笔记里点了也没法改
    var canEditSource: Bool {
        guard onEditSource != nil, let host = superview as? DiagramTextView else { return false }
        return !host.isReading && host.isEditable
    }
    override func mouseEntered(with event: NSEvent) {
        editButton.isHidden = !canEditSource
        focusButton.isHidden = onFocus == nil
    }
    override func mouseExited(with event: NSEvent) {
        editButton.isHidden = true
        focusButton.isHidden = true
    }
    @objc private func editSource() { onEditSource?() }
    @objc private func enterFocus() { onFocus?() }
    @objc private func jumpFromMenu(_ item: NSMenuItem) {
        guard let value = item.representedObject as? NSValue else { return }
        onSourceRange?(value.rangeValue)
    }
    @objc private func toggleFromMenu(_ item: NSMenuItem) {
        guard let id = item.representedObject as? String else { return }
        onToggle?(id)
    }
    @objc private func focusFromMenu() { onFocus?() }
    func refreshHoverButtons() { updateTrackingAreas() }
    override func draw(_ dirtyRect: NSRect) {
        let started = ProcessInfo.processInfo.systemUptime
        defer { (superview as? DiagramTextView)?.recordMainWork(since: started) }
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        SceneRenderer.draw(prepared, in: context, highlight: highlightStyle())
    }
    /// 高亮只用淡底叠加，颜色从节点文字色派生；不改动场景或排版结果。
    private func highlightStyle() -> SceneRenderer.Highlight? {
        guard let id = highlightedNodeID,
              let hit = prepared.scene.hitRegions.first(where: { $0.nodeId == id }) else { return nil }
        var color = NSColor.controlAccentColor.cgColor
        let rect = hit.rect.cgRect
        for primitive in prepared.scene.primitives {
            guard case let .text(span) = primitive,
                  rect.insetBy(dx: -3, dy: -3).contains(span.baseline.cgPoint) else { continue }
            color = span.color.cgColor
            break
        }
        let dark = effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return SceneRenderer.Highlight(rect: rect.insetBy(dx: -6, dy: -4),
                                       color: color.copy(alpha: dark ? 0.18 : 0.10) ?? color,
                                       border: color.copy(alpha: dark ? 0.30 : 0.18) ?? color)
    }
    override func menu(for event: NSEvent) -> NSMenu? {
        let point = convert(event.locationInWindow, from: nil)
        let hit = prepared.scene.hitRegions.first(where: { $0.rect.cgRect.contains(point) })
        let menu = NSMenu()
        if let range = sourceRange(at: point) {
            let item = NSMenuItem(title: L10n.string("Jump to Source"), action: #selector(jumpFromMenu(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = NSValue(range: range)
            menu.addItem(item)
        }
        if let hit, hit.canCollapse {
            let item = NSMenuItem(title: hit.collapsed ? L10n.string("Expand") : L10n.string("Collapse"),
                                  action: #selector(toggleFromMenu(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = hit.nodeId
            menu.addItem(item)
        }
        if !menu.items.isEmpty { menu.addItem(.separator()) }
        let focus = NSMenuItem(title: L10n.string("Focus Mode"), action: #selector(focusFromMenu), keyEquivalent: "")
        focus.target = self
        focus.isEnabled = onFocus != nil
        menu.addItem(focus)
        return menu
    }
    override func resetCursorRects() {
        for hit in prepared.scene.hitRegions where hit.canCollapse { addCursorRect(hit.rect.cgRect, cursor: .pointingHand) }
    }
    func sourceRange(at point: CGPoint) -> NSRange? {
        guard prepared.acceptsInteraction else { return nil }
        let containerRects = prepared.scene.primitives.compactMap { primitive -> CGRect? in
            guard case let .graphShape(rect, shape, _, _, _, _, _) = primitive, shape == "Container" else { return nil }
            return rect.cgRect
        }
        let hit = prepared.scene.hitRegions.first(where: { $0.rect.cgRect.contains(point) })
        if let hit, !containerRects.contains(hit.rect.cgRect) { return prepared.sourceRanges[hit.nodeId] }
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
        return hit.flatMap { prepared.sourceRanges[$0.nodeId] }
    }
    override func mouseDown(with event: NSEvent) {
        guard prepared.acceptsInteraction else { return }
        let point = convert(event.locationInWindow, from: nil)
        // ⌘-点击优先于折叠：可折叠节点也直接跳到源码。
        if event.modifierFlags.contains(.command), let range = sourceRange(at: point) {
            onSourceRange?(range)
            return
        }
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
    private let jump: @MainActor @Sendable (String) -> Bool
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
        jump = { [weak owner] id in
            guard let owner, owner.prepared.acceptsInteraction,
                  let range = owner.prepared.sourceRanges[id], let action = owner.onSourceRange else { return false }
            action(range)
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
        if selector == #selector(accessibilityCustomActions) { return true }
        return super.isAccessibilitySelectorAllowed(selector)
    }
    override func accessibilityCustomActions() -> [NSAccessibilityCustomAction]? {
        let action = jump
        let id = hit.nodeId
        return [NSAccessibilityCustomAction(name: L10n.string("Jump to Source")) {
            MainActor.assumeIsolated { action(id) }
        }]
    }
}
