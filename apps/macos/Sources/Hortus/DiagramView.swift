import AppKit
import HortusCore

@MainActor
final class DiagramView: NSView {
    private(set) var prepared: PreparedScene
    var onToggle: ((String) -> Void)?
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
    override var isFlipped: Bool { true }

    init(prepared: PreparedScene) {
        self.prepared = prepared
        super.init(frame: NSRect(x: 0, y: 0, width: prepared.scene.width, height: prepared.scene.height))
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(L10n.string("Tree diagram"))
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
        needsDisplay = true
        window?.invalidateCursorRects(for: self)
    }
    override func accessibilityChildren() -> [Any]? { nodes }
    func detach() {
        nodes = []
        onToggle = nil
        onEditSource = nil
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
    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let hit = prepared.scene.hitRegions.first(where: { $0.canCollapse && $0.rect.cgRect.contains(point) }) {
            onToggle?(hit.nodeId)
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
            guard let owner else { return false }
            owner.onToggle?(id)
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
        guard hit.canCollapse else { return false }
        let action = press
        let id = hit.nodeId
        return MainActor.assumeIsolated { action(id) }
    }
    override func isAccessibilitySelectorAllowed(_ selector: Selector) -> Bool {
        if selector == #selector(isAccessibilityExpanded) || selector == #selector(accessibilityPerformPress) { return hit.canCollapse }
        return super.isAccessibilitySelectorAllowed(selector)
    }
}
