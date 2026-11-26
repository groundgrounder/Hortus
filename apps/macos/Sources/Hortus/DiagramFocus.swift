import AppKit
import SwiftUI
import HortusCore

/// 进入专注模式时的快照；正文表面保持原样，退出后按这些值恢复。
@MainActor
struct DiagramFocusSession {
    let id = UUID()
    let note: String
    let vault: String
    let ordinal: UInt32
    let document: NoteDocument
    let reading: Bool
    let selection: NSRange
    let scroll: CGPoint
    let initial: DiagramFocus?
}

/// 只有显式自检参数下才收集逐帧耗时。
@MainActor
enum FocusMeasurement {
    static var active = false
}

/// 专注模式快捷键；只接受精确的设备无关修饰键组合。
enum FocusKeyCommand: Equatable {
    case search, exit, nextMatch, previousMatch, fit, actualSize, zoomIn, zoomOut

    static func resolve(_ event: NSEvent) -> FocusKeyCommand? {
        let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let key = (event.charactersIgnoringModifiers ?? "").lowercased()
        switch (modifiers, key) {
        case (.command, "f"): return .search
        case ([.command, .shift], "d"): return .exit
        case (.command, "g"): return .nextMatch
        case ([.command, .shift], "g"): return .previousMatch
        case (.command, "0"): return .fit
        case (.command, "1"): return .actualSize
        case (.command, "="), (.command, "+"): return .zoomIn
        case (.command, "-"), (.command, "_"): return .zoomOut
        default: return nil
        }
    }
}

/// SwiftUI 的 CommandMenu 不随正文图块的异步排版重建；显式同步原生菜单项。
@MainActor
enum FocusMenuState {
    #if DEBUG
    static var inspectionItem: NSMenuItem?
    #endif
    static func setEnabled(_ enabled: Bool) {
        guard let item = menuItem(), item.isEnabled != enabled else { return }
        item.isEnabled = enabled
    }

    private static func menuItem() -> NSMenuItem? {
        #if DEBUG
        if let inspectionItem { return inspectionItem }
        #endif
        func visit(_ menu: NSMenu) -> NSMenuItem? {
            for item in menu.items {
                let modifiers = item.keyEquivalentModifierMask.intersection(.deviceIndependentFlagsMask)
                if item.keyEquivalent.lowercased() == "d", modifiers == [.command, .shift] { return item }
                if let submenu = item.submenu, let found = visit(submenu) { return found }
            }
            return nil
        }
        return NSApp.mainMenu.flatMap(visit)
    }
}

struct FocusOverlay: NSViewRepresentable {
    let session: DiagramFocusSession
    let core: WorkspaceCore?
    let fontSize: Double
    let revision: UInt64
    let refreshToken: Int
    let onExit: () -> Void
    let onJump: (NSRange) -> Void
    let onCollapse: ([String]) -> Void
    let onPersist: (Double, Double, Double) -> Void
    let onOpenLink: (String, String?) -> Void

    func makeNSView(context: Context) -> FocusModeView {
        FocusModeView(session: session, core: core, fontSize: fontSize,
                      onExit: onExit, onJump: onJump, onCollapse: onCollapse, onPersist: onPersist,
                      onOpenLink: onOpenLink,
                      revision: revision, refreshToken: refreshToken)
    }
    func updateNSView(_ view: FocusModeView, context: Context) {
        view.updateDocument(revision: revision, refreshToken: refreshToken)
    }
}

/// 专注模式：顶栏克制操作区 + 可缩放平移的矢量画布。
@MainActor
final class FocusModeView: NSView {
    let session: DiagramFocusSession
    let canvas: FocusCanvasView
    private let bar = NSView()
    private let percentLabel = NSTextField(labelWithString: "100%")
    private let fitButton = NSButton(title: L10n.string("Fit"), target: nil, action: nil)
    private let actualButton = NSButton(title: L10n.string("100%"), target: nil, action: nil)
    private let searchField = NSSearchField()
    private let matchLabel = NSTextField(labelWithString: "")
    private let hiddenButton = NSButton(title: "", target: nil, action: nil)
    private let exportButton = NSButton(title: DiagramExportStrings.export, target: nil, action: nil)
    private let closeButton = NSButton()
    private let onExit: () -> Void
    private let onOpenLink: (String, String?) -> Void
    private var searchVisible = false
    private var documentRevision: UInt64
    private var documentRefreshToken: Int
    private var revisionTask: Task<Void, Never>?

    init(session: DiagramFocusSession, core: WorkspaceCore?, fontSize: Double,
         onExit: @escaping () -> Void, onJump: @escaping (NSRange) -> Void,
         onCollapse: @escaping ([String]) -> Void, onPersist: @escaping (Double, Double, Double) -> Void,
         onOpenLink: @escaping (String, String?) -> Void = { _, _ in },
         revision: UInt64? = nil, refreshToken: Int = 0) {
        self.session = session
        self.onExit = onExit
        self.onOpenLink = onOpenLink
        documentRevision = revision ?? session.document.revision
        documentRefreshToken = refreshToken
        canvas = FocusCanvasView(session: session, core: core, fontSize: fontSize)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        bar.wantsLayer = true
        bar.layer?.backgroundColor = NSColor.windowBackgroundColor.withAlphaComponent(0.94).cgColor
        // 画布层在翻转坐标下可能盖到顶栏；显式把顶栏放到最上层。
        bar.layer?.zPosition = 1
        addSubview(bar)
        addSubview(canvas)

        percentLabel.font = .monospacedDigitSystemFont(ofSize: 12, weight: .medium)
        percentLabel.textColor = .secondaryLabelColor
        percentLabel.alignment = .right
        fitButton.bezelStyle = .inline
        fitButton.controlSize = .small
        fitButton.font = .systemFont(ofSize: 12)
        fitButton.target = self
        fitButton.action = #selector(fit)
        actualButton.bezelStyle = .inline
        actualButton.controlSize = .small
        actualButton.font = .systemFont(ofSize: 12)
        actualButton.target = self
        actualButton.action = #selector(actualSize)
        searchField.placeholderString = L10n.string("Find node")
        searchField.delegate = self
        searchField.controlSize = .small
        searchField.font = .systemFont(ofSize: 12)
        searchField.isHidden = true
        matchLabel.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        matchLabel.textColor = .secondaryLabelColor
        hiddenButton.bezelStyle = .inline
        hiddenButton.controlSize = .small
        hiddenButton.font = .systemFont(ofSize: 11)
        hiddenButton.target = self
        hiddenButton.action = #selector(expandHidden)
        hiddenButton.isHidden = true
        closeButton.bezelStyle = .inline
        closeButton.isBordered = false
        closeButton.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: L10n.string("Close"))
        closeButton.target = self
        closeButton.action = #selector(close)
        exportButton.bezelStyle = .inline
        exportButton.controlSize = .small
        exportButton.font = .systemFont(ofSize: 12)
        exportButton.image = NSImage(systemSymbolName: "square.and.arrow.up",
                                     accessibilityDescription: DiagramExportStrings.export)
        exportButton.imagePosition = .imageLeading
        exportButton.target = self
        exportButton.action = #selector(showExportMenu(_:))
        exportButton.isEnabled = false
        for view in [percentLabel, fitButton, actualButton, searchField, matchLabel, hiddenButton, exportButton, closeButton] {
            bar.addSubview(view)
        }
        canvas.onScale = { [weak self] value in self?.percentLabel.stringValue = "\(Int((value * 100).rounded()))%" }
        canvas.onSearchUpdate = { [weak self] index, count, hidden in self?.updateMatches(index, count, hidden) }
        canvas.onJump = onJump
        canvas.onCollapsed = { nodes in onCollapse(nodes.sorted()) }
        canvas.onPersist = { scale, point in onPersist(Double(scale), Double(point.x), Double(point.y)) }
        canvas.onExit = onExit
        canvas.onOpenLink = onOpenLink
        canvas.onReady = { [weak self] in
            self?.exportButton.isEnabled = true
            self?.needsLayout = true
        }
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    /// 外部修改或回滚后按修订号刷新画布；防抖保留用户当前缩放与视口中心。
    func updateDocument(revision: UInt64, refreshToken: Int) {
        guard revision != documentRevision || refreshToken != documentRefreshToken else { return }
        documentRevision = revision
        documentRefreshToken = refreshToken
        revisionTask?.cancel()
        revisionTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(200))
            guard !Task.isCancelled, let self else { return }
            self.canvas.reloadPreservingViewport()
        }
    }
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.textBackgroundColor.setFill()
        dirtyRect.fill()
        NSColor.separatorColor.withAlphaComponent(0.7).setFill()
        NSRect(x: 0, y: topInset + FocusModeView.barHeight - 1, width: bounds.width, height: 1).fill()
    }
    static let barHeight: CGFloat = 46
    /// 正文铺到系统工具栏下面；顶栏必须让开这一层，否则被工具栏盖住。
    private var topInset: CGFloat {
        guard let window, let content = window.contentView else { return 8 }
        return max(8, content.bounds.height - window.contentLayoutRect.height)
    }
    override func layout() {
        super.layout()
        let top = topInset
        bar.frame = NSRect(x: 0, y: top, width: bounds.width, height: Self.barHeight)
        canvas.frame = NSRect(x: 0, y: top + Self.barHeight,
                              width: bounds.width, height: max(0, bounds.height - top - Self.barHeight))
        let right = bounds.width - 12
        closeButton.frame = NSRect(x: right - 22, y: 12, width: 22, height: 22)
        percentLabel.frame = NSRect(x: 14, y: 14, width: 52, height: 18)
        fitButton.frame = NSRect(x: 74, y: 10, width: 42, height: 24)
        actualButton.frame = NSRect(x: 118, y: 10, width: 52, height: 24)
        exportButton.sizeToFit()
        let exportWidth = max(64, exportButton.fittingSize.width)
        exportButton.frame = NSRect(x: closeButton.frame.minX - 8 - exportWidth,
                                    y: 10, width: exportWidth, height: 24)
        hiddenButton.sizeToFit()
        matchLabel.sizeToFit()
        let searchX: CGFloat = 182
        let available = max(120, exportButton.frame.minX - 8 - searchX)
        searchField.frame = NSRect(x: searchX, y: 9, width: min(240, available), height: 26)
        matchLabel.frame.origin = CGPoint(x: searchField.frame.maxX + 8, y: 14)
        hiddenButton.frame.origin = CGPoint(x: matchLabel.frame.maxX + 10, y: 11)
        if !searchVisible { hiddenButton.isHidden = true; matchLabel.isHidden = true }
    }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard let window else { return }
        needsLayout = true
        layoutSubtreeIfNeeded()
        window.makeFirstResponder(canvas)
    }
    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if newWindow == nil { canvas.persistNow() }
        super.viewWillMove(toWindow: newWindow)
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // 专注快捷键只属于本覆盖层内的第一响应者；侧栏、终端与 QuickOpen 保持各自行为。
        guard let responder = window?.firstResponder as? NSView,
              responder.isDescendant(of: self) else {
            // 专注命令一律不处理；其他键仍交给响应链与系统菜单。
            return FocusKeyCommand.resolve(event) == nil ? super.performKeyEquivalent(with: event) : false
        }
        switch FocusKeyCommand.resolve(event) {
        case .search?: showSearch(); return true
        case .exit?: onExit(); return true
        default: break
        }
        if canvas.handleShortcut(event) { return true }
        return super.performKeyEquivalent(with: event)
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { onExit(); return }
        super.keyDown(with: event)
    }
    @objc private func fit() { canvas.fit() }
    @objc private func actualSize() { canvas.actualSize() }
    @objc private func close() { onExit() }
    @objc private func expandHidden() { canvas.expandHiddenMatches() }
    @objc private func showExportMenu(_ sender: NSButton) {
        guard canvas.canExport else { return }
        let menu = NSMenu()
        for format in DiagramExportFormat.allCases {
            let item = NSMenuItem(title: format.menuTitle,
                                  action: #selector(exportFormat(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = format.rawValue
            menu.addItem(item)
        }
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: sender.bounds.maxY + 4), in: sender)
    }
    @objc private func exportFormat(_ item: NSMenuItem) {
        guard let raw = item.representedObject as? String,
              let format = DiagramExportFormat(rawValue: raw),
              let request = canvas.exportRequest() else { return }
        DiagramExport.presentSavePanel(format: format, request: request, window: window)
    }
    private func showSearch() {
        searchVisible = true
        searchField.isHidden = false
        matchLabel.isHidden = false
        needsLayout = true
        window?.makeFirstResponder(searchField)
        searchField.currentEditor()?.selectAll(nil)
    }
    private func updateMatches(_ index: Int, _ count: Int, _ hidden: Int) {
        matchLabel.stringValue = count > 0 ? "\(min(index + 1, count)) / \(count)" : ""
        hiddenButton.title = hidden > 0 ? L10n.string("%d in collapsed nodes", hidden) : ""
        hiddenButton.isHidden = hidden == 0
        hiddenButton.sizeToFit()
        matchLabel.sizeToFit()
        needsLayout = true
    }
}

extension FocusModeView: NSSearchFieldDelegate {
    func controlTextDidChange(_ notification: Notification) {
        canvas.search(searchField.stringValue)
    }
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        if selector == #selector(NSResponder.cancelOperation(_:)) {
            if searchField.stringValue.isEmpty {
                searchVisible = false
                searchField.isHidden = true
                window?.makeFirstResponder(canvas)
            } else {
                searchField.stringValue = ""
                canvas.search("")
            }
            return true
        }
        if selector == #selector(NSResponder.insertNewline(_:)) {
            canvas.nextMatch(1)
            return true
        }
        return false
    }
}

#if DEBUG
extension FocusModeView {
    var inspectionSearchVisible: Bool { !searchField.isHidden }
    var inspectionSearchField: NSSearchField { searchField }
    var inspectionExportPresent: Bool { exportButton.superview != nil && !exportButton.isHidden }
    var inspectionExportEnabled: Bool { exportButton.isEnabled }
    var inspectionExportFrame: NSRect { exportButton.frame }
    var inspectionExportTitles: [String] { DiagramExportFormat.allCases.map(\.menuTitle) }
    var inspectionLayout: [String: Any] {
        ["focus": NSStringFromRect(frame), "bar": NSStringFromRect(bar.frame),
         "canvas": NSStringFromRect(canvas.frame), "bounds": NSStringFromRect(bounds),
         "bar_hidden": bar.isHidden, "canvas_hidden": canvas.isHidden,
         "percent": NSStringFromRect(percentLabel.frame), "percent_hidden": percentLabel.isHidden,
         "fit": NSStringFromRect(fitButton.frame), "fit_hidden": fitButton.isHidden,
         "top_inset": topInset,
         "bar_alpha": bar.alphaValue, "bar_layer_hidden": bar.layer?.isHidden ?? false,
         "bar_bg": bar.layer?.backgroundColor.map { String(describing: $0) } ?? "nil",
         "subviews": subviews.map { "\(type(of: $0)) \(NSStringFromRect($0.frame)) hidden=\($0.isHidden) alpha=\($0.alphaValue)" },
         "window": window.map { NSStringFromRect($0.frame) } ?? "",
         "content_layout": window.map { NSStringFromRect($0.contentLayoutRect) } ?? ""]
    }
    func inspectionShowSearch(_ query: String) {
        showSearch()
        searchField.stringValue = query
        canvas.search(query)
        window?.displayIfNeeded()
    }
}
#endif

/// 专注模式的缩放边界；绘制直接用 CTM，不依赖位图缩放。
enum DiagramFocusLimits {
    static let minimum: CGFloat = 0.25
    static let maximum: CGFloat = 4
    static func clamp(_ value: CGFloat) -> CGFloat { min(maximum, max(minimum, value)) }
}

/// 直接对 CoreGraphics 施加矢量变换；缩放与平移不触发 Rust 重排。
@MainActor
final class FocusCanvasView: NSView {
    private let session: DiagramFocusSession
    private let core: WorkspaceCore?
    private let fontSize: Double
    private let service = DiagramService()
    private let enteredAt = ProcessInfo.processInfo.systemUptime
    private var prepared: PreparedScene?
    private var source = ""
    private var collapsed: Set<String>
    private var generation = 0
    private var loadWidth: CGFloat = 0
    private var loadTask: Task<Void, Never>?
    private var widthTask: Task<Void, Never>?
    private var didLoad = false
    private var appliedInitial = false
    private var pendingSearchUpdate = false
    private var persistenceSuspended = false
    private var persistTask: Task<Void, Never>?

    private(set) var scale: CGFloat = 1
    private var offset = CGPoint.zero
    private var spaceHeld = false
    private var dragging = false
    private var dragStart = CGPoint.zero
    private var dragOffset = CGPoint.zero
    private var minimapDragging = false
    private var minimapRect = CGRect.zero
    private var minimapHidden = true
    private var matchQuery = ""
    private var visibleMatches: [SceneHit] = []
    private var hiddenMatches: [(hit: SceneHit, ancestors: Set<String>)] = []
    private var currentMatch = 0
    private var allPrepared: PreparedScene?
    private var searchTask: Task<Void, Never>?

    var onScale: ((CGFloat) -> Void)?
    var onSearchUpdate: ((Int, Int, Int) -> Void)?
    var onJump: ((NSRange) -> Void)?
    var onCollapsed: ((Set<String>) -> Void)?
    var onPersist: ((CGFloat, CGPoint) -> Void)?
    var onExit: (() -> Void)?
    var onOpenLink: ((String, String?) -> Void)?
    var onReady: (() -> Void)?
    var canExport: Bool { prepared != nil }

    /// 当前画布只含一张图，导出请求直接复用已排好的场景。
    func exportRequest() -> DiagramExportRequest? {
        guard let prepared else { return nil }
        let ordinal = session.ordinal
        return DiagramExportRequest(prepared: prepared, note: session.note, ordinal: ordinal) {
            try await Task.detached(priority: .userInitiated) {
                try DiagramExport.renderSVG(prepared: prepared)
            }.value
        }
    }

    // 自检数据；常规使用时只保留第一个计数。
    private(set) var rustLayoutCalls = 0
    private(set) var drawDurations: [Double] = []
    private(set) var firstFrameMilliseconds: Double?
    private(set) var layoutMilliseconds = 0.0
    private(set) var searchSceneRequests = 0

    init(session: DiagramFocusSession, core: WorkspaceCore?, fontSize: Double) {
        self.session = session
        self.core = core
        self.fontSize = fontSize
        if let core {
            collapsed = Set((try? core.collapsedNodes(vault: session.vault, note: session.note, block: session.ordinal)) ?? [])
        } else {
            collapsed = []
        }
        super.init(frame: .zero)
        wantsLayer = true
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(L10n.string("Diagram focus mode"))
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil else { return }
        window?.makeFirstResponder(self)
        reload()
    }
    override func layout() {
        super.layout()
        guard bounds.width > 1 else { return }
        if !didLoad { reload(); return }
        if abs(bounds.width - loadWidth) > 0.5 {
            widthTask?.cancel()
            let center = centerScenePoint()
            widthTask = Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(120))
                guard !Task.isCancelled, let self else { return }
                self.reload(preserve: center)
            }
        }
    }
    private var mmd: Bool { session.note.lowercased().hasSuffix(".mmd") }
    private var viewport: CGRect { bounds }

    /// 文档修订变化后的刷新入口：保留当前缩放和平移中心。
    func reloadPreservingViewport() {
        guard window != nil, bounds.width > 1 else { return }
        reload(preserve: centerScenePoint())
    }
    func reload(preserve: CGPoint? = nil, anchorNode: String? = nil) {
        guard bounds.width > 1 else { return }
        generation += 1
        let token = generation
        loadTask?.cancel()
        let width = max(120, bounds.width - 64)
        loadWidth = bounds.width
        didLoad = true
        let preserve = preserve ?? centerScenePoint()
        let document = session.document
        let vault = session.vault
        let note = session.note
        let ordinal = session.ordinal
        let mmd = mmd
        let collapsed = collapsed
        let dark = effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let service = service
        rustLayoutCalls += 1
        let started = ProcessInfo.processInfo.systemUptime
        loadTask = Task { [weak self] in
            do {
                let snapshot = try await document.io.perform { try $0.snapshot() }
                guard !Task.isCancelled, let self, token == generation else { return }
                var blocks = try await service.blocks(snapshot: snapshot, vault: vault, note: note,
                                                     core: self.core, documentIdentity: document.diagramIdentity)
                guard let index = blocks.firstIndex(where: { $0.ordinal == ordinal }) else {
                    throw CocoaError(.coderValueNotFound)
                }
                blocks[index].collapsed = collapsed
                let rendered = await service.render(source: snapshot.text, mmd: mmd, blocks: [blocks[index]],
                                                    width: Double(width), dark: dark,
                                                    fontGeneration: self.fontSize.bitPattern)
                guard !Task.isCancelled, self.generation == token else { return }
                guard let next = rendered.first?.prepared else {
                    self.prepared = nil
                    self.needsDisplay = true
                    return
                }
                self.source = snapshot.text
                self.layoutMilliseconds = (ProcessInfo.processInfo.systemUptime - started) * 1000
                self.apply(next, preserve: preserve, anchorNode: anchorNode)
            } catch {
                guard !Task.isCancelled, let self, self.generation == token else { return }
                self.prepared = nil
                self.needsDisplay = true
            }
        }
    }
    private func apply(_ next: PreparedScene, preserve: CGPoint?, anchorNode: String?) {
        prepared = next
        if !appliedInitial {
            appliedInitial = true
            setInitialTransform()
        }
        if let anchorNode, let hit = next.scene.hitRegions.first(where: { $0.nodeId == anchorNode }) {
            let screen = CGPoint(x: hit.rect.cgRect.midX * scale + offset.x,
                                 y: hit.rect.cgRect.midY * scale + offset.y)
            if let anchor = anchorScreen {
                offset = CGPoint(x: offset.x + anchor.x - screen.x, y: offset.y + anchor.y - screen.y)
            }
        } else if let preserve {
            offset = CGPoint(x: viewport.midX - preserve.x * scale, y: viewport.midY - preserve.y * scale)
        }
        anchorScreen = nil
        clampOffset()
        updateMinimapVisibility()
        onScale?(scale)
        if pendingSearchUpdate || !matchQuery.isEmpty {
            pendingSearchUpdate = false
            computeMatches()
        }
        needsDisplay = true
        window?.invalidateCursorRects(for: self)
        displayIfNeeded()
        if firstFrameMilliseconds == nil {
            firstFrameMilliseconds = (ProcessInfo.processInfo.systemUptime - enteredAt) * 1000
            onReady?()
        }
    }
    private var anchorScreen: CGPoint?
    private func setInitialTransform() {
        if let initial = session.initial {
            scale = clampScale(initial.scale)
            offset = CGPoint(x: initial.offsetX, y: initial.offsetY)
        } else {
            fit()
        }
    }
    func fit() {
        guard let prepared, bounds.width > 1 else { return }
        let width = Double(max(1, bounds.width - 96))
        let height = Double(max(1, bounds.height - 96))
        scale = clampScale(min(1, min(width / prepared.scene.width, height / prepared.scene.height)))
        offset = CGPoint(x: (bounds.width - prepared.scene.width * scale) / 2,
                         y: (bounds.height - prepared.scene.height * scale) / 2)
        finishTransformChange()
    }
    func actualSize() {
        setScale(1, anchor: CGPoint(x: bounds.midX, y: bounds.midY))
    }
    private func clampScale(_ value: CGFloat) -> CGFloat { DiagramFocusLimits.clamp(value) }
    func setScale(_ value: CGFloat, anchor: CGPoint) {
        guard prepared != nil else { return }
        let previous = scale
        let scenePoint = CGPoint(x: (anchor.x - offset.x) / previous, y: (anchor.y - offset.y) / previous)
        scale = clampScale(value)
        offset = CGPoint(x: anchor.x - scenePoint.x * scale, y: anchor.y - scenePoint.y * scale)
        finishTransformChange()
    }
    private func finishTransformChange() {
        clampOffset()
        updateMinimapVisibility()
        onScale?(scale)
        needsDisplay = true
        schedulePersist()
    }
    private func clampOffset() {
        guard let prepared else { return }
        let width = prepared.scene.width * scale, height = prepared.scene.height * scale
        let slack: CGFloat = 48
        let minX = min(0, bounds.width - width) - slack
        let maxX = max(0, bounds.width - width) + slack
        let minY = min(0, bounds.height - height) - slack
        let maxY = max(0, bounds.height - height) + slack
        offset.x = min(maxX, max(minX, offset.x))
        offset.y = min(maxY, max(minY, offset.y))
    }
    private func centerScenePoint() -> CGPoint? {
        guard prepared != nil else { return nil }
        return CGPoint(x: (bounds.midX - offset.x) / scale, y: (bounds.midY - offset.y) / scale)
    }
    private func updateMinimapVisibility() {
        guard let prepared else { minimapHidden = true; return }
        let rect = CGRect(origin: offset, size: CGSize(width: prepared.scene.width * scale,
                                                       height: prepared.scene.height * scale))
        minimapHidden = rect.minX >= 0 && rect.minY >= 0 && rect.maxX <= bounds.width && rect.maxY <= bounds.height
        let side: CGFloat = 150, height: CGFloat = 100
        minimapRect = CGRect(x: bounds.width - side - 16, y: bounds.height - height - 16, width: side, height: height)
    }
    private func schedulePersist() {
        guard !persistenceSuspended else { return }
        persistTask?.cancel()
        persistTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled, let self else { return }
            self.onPersist?(self.scale, self.offset)
        }
    }
    func persistNow() {
        guard prepared != nil else { return }
        persistTask?.cancel()
        onPersist?(scale, offset)
    }

    override func draw(_ dirtyRect: NSRect) {
        let started = ProcessInfo.processInfo.systemUptime
        defer {
            let elapsed = (ProcessInfo.processInfo.systemUptime - started) * 1000
            if FocusMeasurement.active { drawDurations.append(elapsed) }
        }
        NSColor.textBackgroundColor.setFill()
        dirtyRect.fill()
        guard let prepared, let context = NSGraphicsContext.current?.cgContext else {
            drawPlaceholder()
            return
        }
        let accent = NSColor.controlAccentColor.cgColor
        let matches = visibleMatches.map { hit in
            SceneRenderer.Highlight(rect: hit.rect.cgRect.insetBy(dx: -5, dy: -3),
                                    color: accent.copy(alpha: 0.10) ?? accent,
                                    border: accent.copy(alpha: 0.16) ?? accent)
        }
        let current: SceneRenderer.Highlight? = (currentMatch >= 0 && currentMatch < visibleMatches.count)
            ? SceneRenderer.Highlight(rect: visibleMatches[currentMatch].rect.cgRect.insetBy(dx: -5, dy: -3),
                                      color: accent.copy(alpha: 0.16) ?? accent,
                                      border: accent.copy(alpha: 0.85) ?? accent)
            : nil
        context.saveGState()
        context.translateBy(x: offset.x, y: offset.y)
        context.scaleBy(x: scale, y: scale)
        SceneRenderer.draw(prepared, in: context, matches: matches, currentMatch: current)
        context.restoreGState()
        drawMinimap(prepared, in: context)
        if firstFrameMilliseconds == nil {
            firstFrameMilliseconds = (ProcessInfo.processInfo.systemUptime - enteredAt) * 1000
            onReady?()
        }
    }
    private func drawPlaceholder() {
        let text = L10n.string("Diagram could not be laid out; showing source")
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 13),
            .foregroundColor: NSColor.secondaryLabelColor,
        ]
        let size = (text as NSString).size(withAttributes: attributes)
        (text as NSString).draw(at: CGPoint(x: (bounds.width - size.width) / 2,
                                            y: (bounds.height - size.height) / 2), withAttributes: attributes)
    }
    private func drawMinimap(_ prepared: PreparedScene, in context: CGContext) {
        guard !minimapHidden else { return }
        let box = minimapRect
        let background = NSBezierPath(roundedRect: box, xRadius: 8, yRadius: 8)
        NSColor.windowBackgroundColor.withAlphaComponent(0.82).setFill()
        background.fill()
        NSColor.separatorColor.withAlphaComponent(0.8).setStroke()
        background.lineWidth = 1
        background.stroke()
        let inset = box.insetBy(dx: 6, dy: 6)
        let scene = prepared.scene
        let s = min(inset.width / scene.width, inset.height / scene.height)
        let origin = CGPoint(x: inset.midX - scene.width * s / 2, y: inset.midY - scene.height * s / 2)
        context.saveGState()
        context.clip(to: inset)
        context.translateBy(x: origin.x, y: origin.y)
        context.scaleBy(x: s, y: s)
        SceneRenderer.draw(prepared, in: context, includeText: false)
        context.restoreGState()
        let visible = CGRect(x: -offset.x / scale, y: -offset.y / scale,
                             width: bounds.width / scale, height: bounds.height / scale)
            .intersection(CGRect(x: 0, y: 0, width: scene.width, height: scene.height))
        guard !visible.isNull else { return }
        let viewport = CGRect(x: origin.x + visible.minX * s, y: origin.y + visible.minY * s,
                              width: visible.width * s, height: visible.height * s)
        NSColor.controlAccentColor.withAlphaComponent(0.16).setFill()
        viewport.fill()
        NSColor.controlAccentColor.withAlphaComponent(0.7).setStroke()
        NSBezierPath(rect: viewport).stroke()
    }

    // MARK: 输入
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        handleShortcut(event) ? true : super.performKeyEquivalent(with: event)
    }
    func handleShortcut(_ event: NSEvent) -> Bool {
        guard let command = FocusKeyCommand.resolve(event) else { return false }
        switch command {
        case .nextMatch: nextMatch(1); return true
        case .previousMatch: nextMatch(-1); return true
        case .fit: fit(); return true
        case .actualSize: actualSize(); return true
        case .zoomIn:
            setScale(scale * 1.2, anchor: CGPoint(x: bounds.midX, y: bounds.midY)); return true
        case .zoomOut:
            setScale(scale / 1.2, anchor: CGPoint(x: bounds.midX, y: bounds.midY)); return true
        default: return false
        }
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { onExit?(); return }
        if event.keyCode == 49, window?.firstResponder === self {
            spaceHeld = true
            window?.invalidateCursorRects(for: self)
            return
        }
        super.keyDown(with: event)
    }
    override func keyUp(with event: NSEvent) {
        if event.keyCode == 49 {
            spaceHeld = false
            window?.invalidateCursorRects(for: self)
        }
        super.keyUp(with: event)
    }
    override func magnify(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        setScale(scale * (1 + event.magnification), anchor: point)
    }
    override func scrollWheel(with event: NSEvent) {
        guard prepared != nil else { return }
        offset.x += event.scrollingDeltaX
        offset.y += event.scrollingDeltaY
        finishTransformChange()
    }
    override func resetCursorRects() {
        addCursorRect(bounds, cursor: spaceHeld ? .openHand : .arrow)
        if !minimapHidden { addCursorRect(minimapRect, cursor: .pointingHand) }
        guard let prepared, !spaceHeld else { return }
        for primitive in prepared.scene.primitives {
            if case let .labelBackground(id, rect, _) = primitive, prepared.edgeRanges[id] != nil {
                let r = rect.cgRect
                addCursorRect(CGRect(x: r.minX * scale + offset.x, y: r.minY * scale + offset.y,
                                     width: r.width * scale, height: r.height * scale),
                              cursor: .pointingHand)
            } else if case let .graphPath(id, _, points, _, _, _, _, _, _, _, _, _) = primitive,
                      prepared.edgeRanges[id] != nil {
                for (a, b) in zip(points, points.dropFirst()) {
                    let minX = min(a.x, b.x) * scale + offset.x - 4
                    let minY = min(a.y, b.y) * scale + offset.y - 4
                    let width = abs(b.x - a.x) * scale + 8
                    let height = abs(b.y - a.y) * scale + 8
                    addCursorRect(CGRect(x: minX, y: minY, width: width, height: height), cursor: .pointingHand)
                }
            }
        }
        for hit in prepared.scene.hitRegions where hit.canCollapse || hit.linkTarget != nil {
            let rect = hit.rect.cgRect
            addCursorRect(CGRect(x: rect.minX * scale + offset.x, y: rect.minY * scale + offset.y,
                                 width: rect.width * scale, height: rect.height * scale),
                          cursor: .pointingHand)
        }
    }
    override func menu(for event: NSEvent) -> NSMenu? {
        guard let prepared else { return nil }
        let point = convert(event.locationInWindow, from: nil)
        let scenePoint = CGPoint(x: (point.x - offset.x) / scale, y: (point.y - offset.y) / scale)
        guard let hit = prepared.topHit(at: scenePoint),
              let target = hit.linkTarget else { return nil }
        let menu = NSMenu()
        let item = NSMenuItem(title: DiagramExportStrings.openLinkedNote,
                              action: #selector(openLinkedNoteFromMenu(_:)), keyEquivalent: "")
        item.target = self
        item.representedObject = [target, hit.linkHeading ?? ""]
        menu.addItem(item)
        return menu
    }
    @objc private func openLinkedNoteFromMenu(_ item: NSMenuItem) {
        guard let value = item.representedObject as? [String], let target = value.first,
              !target.isEmpty else { return }
        let heading = value.count > 1 && !value[1].isEmpty ? value[1] : nil
        onOpenLink?(target, heading)
    }
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        if !minimapHidden, minimapRect.contains(point) {
            minimapDragging = true
            minimapJump(to: point)
            return
        }
        if spaceHeld {
            dragging = true
            dragStart = point
            dragOffset = offset
            NSCursor.closedHand.set()
            return
        }
        guard let prepared else { return }
        let scenePoint = CGPoint(x: (point.x - offset.x) / scale, y: (point.y - offset.y) / scale)
        let nodeHit = prepared.topHit(at: scenePoint, includeContainer: false)
        let edgeRange = nodeHit == nil ? prepared.edgeRange(at: scenePoint, tolerance: max(5, 6 / scale)) : nil
        let containerHit = (nodeHit == nil && edgeRange == nil) ? prepared.topHit(at: scenePoint) : nil
        let hit = nodeHit ?? containerHit
        if event.modifierFlags.contains(.command) {
            if let hit, let range = prepared.sourceRanges[hit.nodeId] { onJump?(range) }
            else if let edgeRange { onJump?(edgeRange) }
            return
        }
        if let hit {
            if hit.canCollapse {
                // 可折叠节点单击继续折叠；⌥-点击才打开节点内双链。
                if event.modifierFlags.contains(.option), let target = hit.linkTarget, let open = onOpenLink {
                    open(target, hit.linkHeading)
                } else {
                    toggleNode(hit.nodeId, anchor: CGPoint(x: hit.rect.cgRect.midX * scale + offset.x,
                                                           y: hit.rect.cgRect.midY * scale + offset.y))
                }
                return
            }
            if let target = hit.linkTarget, let open = onOpenLink {
                open(target, hit.linkHeading)
                return
            }
            if let range = prepared.sourceRanges[hit.nodeId] { onJump?(range) }
            return
        }
        if let edgeRange {
            onJump?(edgeRange)
        }
    }
    private func toggleNode(_ id: String, anchor: CGPoint?) {
        anchorScreen = anchor
        if collapsed.contains(id) { collapsed.remove(id) } else { collapsed.insert(id) }
        invalidateSearchScene()
        onCollapsed?(collapsed)
        reload(anchorNode: id)
    }
    override func mouseDragged(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if minimapDragging { minimapJump(to: point); return }
        guard dragging else { return }
        offset = CGPoint(x: dragOffset.x + point.x - dragStart.x, y: dragOffset.y + point.y - dragStart.y)
        clampOffset()
        updateMinimapVisibility()
        needsDisplay = true
    }
    override func mouseUp(with event: NSEvent) {
        if dragging { NSCursor.arrow.set() }
        dragging = false
        minimapDragging = false
        if prepared != nil { schedulePersist() }
    }
    private func minimapJump(to point: CGPoint) {
        guard let prepared, minimapRect.width > 0 else { return }
        let inset = minimapRect.insetBy(dx: 6, dy: 6)
        let s = min(inset.width / prepared.scene.width, inset.height / prepared.scene.height)
        let origin = CGPoint(x: inset.midX - prepared.scene.width * s / 2, y: inset.midY - prepared.scene.height * s / 2)
        let scenePoint = CGPoint(x: (point.x - origin.x) / s, y: (point.y - origin.y) / s)
        offset = CGPoint(x: bounds.midX - scenePoint.x * scale, y: bounds.midY - scenePoint.y * scale)
        finishTransformChange()
    }

    // MARK: 节点搜索
    func search(_ query: String) {
        matchQuery = query
        currentMatch = 0
        if query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            visibleMatches = []; hiddenMatches = []; allPrepared = nil; publishMatches(); needsDisplay = true
            return
        }
        computeMatches()
        needsDisplay = true
    }
    /// 时序图底部的镜像胶囊与顶部胶囊共享源码范围；搜索计数按参与者去重。
    private func deduplicated(_ hits: [SceneHit], sourceRanges: [String: NSRange]) -> [SceneHit] {
        var seen = Set<String>()
        return hits.filter { hit in
            let range = sourceRanges[hit.nodeId]
            let key = range.map { "\($0.location):\($0.length):\(hit.label)" } ?? "id:\(hit.nodeId)"
            return seen.insert(key).inserted
        }
    }
    private func computeMatches() {
        guard let prepared else { visibleMatches = []; hiddenMatches = []; publishMatches(); return }
        let query = matchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { visibleMatches = []; hiddenMatches = []; publishMatches(); return }
        func matches(_ value: String) -> Bool {
            value.range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) != nil
        }
        visibleMatches = deduplicated(prepared.scene.hitRegions.filter {
            matches($0.label) || matches($0.prefix ?? "")
        }, sourceRanges: prepared.sourceRanges)
        if collapsed.isEmpty {
            hiddenMatches = []
        } else if let allPrepared {
            let visible = Set(prepared.scene.hitRegions.map(\.nodeId))
            hiddenMatches = deduplicated(allPrepared.scene.hitRegions.filter { hit in
                !visible.contains(hit.nodeId) && (matches(hit.label) || matches(hit.prefix ?? ""))
            }, sourceRanges: allPrepared.sourceRanges).map { hit in
                (hit, collapsedAncestors(of: hit, all: allPrepared, visible: prepared))
            }
        } else {
            requestAllHits()
        }
        if currentMatch >= visibleMatches.count { currentMatch = 0 }
        publishMatches()
    }
    private func publishMatches() {
        onSearchUpdate?(currentMatch, visibleMatches.count, hiddenMatches.count)
    }
    func nextMatch(_ delta: Int) {
        guard !visibleMatches.isEmpty else { return }
        currentMatch = (currentMatch + delta + visibleMatches.count) % visibleMatches.count
        center(on: visibleMatches[currentMatch].rect.cgRect)
        publishMatches()
        needsDisplay = true
    }
    private func center(on rect: CGRect) {
        offset = CGPoint(x: bounds.midX - rect.midX * scale, y: bounds.midY - rect.midY * scale)
        finishTransformChange()
    }
    func expandHiddenMatches() {
        var ancestors = Set(hiddenMatches.flatMap(\.ancestors))
        if ancestors.isEmpty { ancestors = collapsed }
        guard !ancestors.isEmpty else { return }
        collapsed.subtract(ancestors)
        invalidateSearchScene()
        onCollapsed?(collapsed)
        pendingSearchUpdate = true
        reload()
        visibleMatches = []
        publishMatches()
    }
    private func invalidateSearchScene() {
        searchTask?.cancel(); searchTask = nil; allPrepared = nil
    }
    private func requestAllHits() {
        guard !collapsed.isEmpty, searchTask == nil, !matchQuery.isEmpty else { return }
        let document = session.document
        let width = max(120, bounds.width - 64)
        let vault = session.vault, note = session.note, ordinal = session.ordinal
        let dark = effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let service = service, core = core, mmd = mmd
        let fontGeneration = fontSize.bitPattern
        searchSceneRequests += 1
        rustLayoutCalls += 1
        let collapsedSnapshot = collapsed
        searchTask = Task { [weak self] in
            do {
                let snapshot = try await document.io.perform { try $0.snapshot() }
                var blocks = try await service.blocks(snapshot: snapshot, vault: vault, note: note,
                                                      core: core, documentIdentity: document.diagramIdentity)
                guard let index = blocks.firstIndex(where: { $0.ordinal == ordinal }) else { return }
                blocks[index].collapsed = []
                let rendered = await service.render(source: snapshot.text, mmd: mmd, blocks: [blocks[index]],
                                                    width: Double(width), dark: dark,
                                                    fontGeneration: fontGeneration)
                guard let self, !Task.isCancelled, self.collapsed == collapsedSnapshot,
                      let all = rendered.first?.prepared else { return }
                self.allPrepared = all
                self.searchTask = nil
                self.computeMatches()
            } catch {
                self?.searchTask = nil
            }
        }
    }
    private func collapsedAncestors(of hit: SceneHit, all: PreparedScene, visible: PreparedScene) -> Set<String> {
        let source = self.source as NSString
        guard let hitRange = all.sourceRanges[hit.nodeId] else { return [] }
        let line = source.lineRange(for: NSRange(location: hitRange.location, length: 0))
        let indent = indentation(in: source, lineStart: line.location)
        var candidates: [(id: String, indent: Int, span: Int)] = []
        for collapsedHit in visible.scene.hitRegions where collapsedHit.collapsed {
            guard let range = visible.sourceRanges[collapsedHit.nodeId] else { continue }
            let collapsedLine = source.lineRange(for: NSRange(location: range.location, length: 0))
            if NSLocationInRange(hitRange.location, range) { return [collapsedHit.nodeId] }
            guard collapsedLine.location < line.location else { continue }
            let collapsedIndent = indentation(in: source, lineStart: collapsedLine.location)
            guard collapsedIndent < indent else { continue }
            let end = subtreeEnd(in: source, lineStart: collapsedLine.location, indent: collapsedIndent)
            guard line.location < end else { continue }
            candidates.append((collapsedHit.nodeId, collapsedIndent, end - collapsedLine.location))
        }
        guard let best = candidates.min(by: { $0.span < $1.span }) else { return [] }
        return [best.id]
    }
    private func indentation(in source: NSString, lineStart: Int) -> Int {
        var column = 0, position = lineStart
        while position < source.length {
            let character = source.character(at: position)
            if character == 0x20 { column += 1 }
            else if character == 0x09 { column += 4 }
            else { break }
            position += 1
        }
        return column
    }
    private func subtreeEnd(in source: NSString, lineStart: Int, indent: Int) -> Int {
        var position = NSMaxRange(source.lineRange(for: NSRange(location: lineStart, length: 0)))
        while position < source.length {
            let line = source.lineRange(for: NSRange(location: position, length: 0))
            let text = source.substring(with: line).trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty, indentation(in: source, lineStart: line.location) <= indent { return line.location }
            if NSMaxRange(line) <= position { break }
            position = NSMaxRange(line)
        }
        return source.length
    }

    // MARK: 自检
    func measureFrames(_ count: Int) -> [Double] {
        FocusMeasurement.active = true
        persistenceSuspended = true
        drawDurations = []
        defer { FocusMeasurement.active = false; persistenceSuspended = false }
        for index in 0..<count {
            if index.isMultiple(of: 2) {
                scale = clampScale(0.5 + CGFloat(index % 12) * 0.25)
            } else {
                offset.x += index.isMultiple(of: 4) ? 24 : -18
            }
            clampOffset()
            needsDisplay = true
            displayIfNeeded()
        }
        return drawDurations
    }
    func screenshotState() -> [String: Any] {
        ["scale": scale, "offset": [offset.x, offset.y], "minimap_hidden": minimapHidden,
         "rust_layout_calls": rustLayoutCalls, "first_frame_ms": firstFrameMilliseconds ?? -1,
         "search_visible": visibleMatches.count, "search_hidden": hiddenMatches.count,
         "search_current": currentMatch, "scene_width": prepared?.scene.width ?? 0,
         "scene_height": prepared?.scene.height ?? 0, "draw_frames": drawDurations.count]
    }
}

#if DEBUG
extension FocusCanvasView {
    var inspectionReady: Bool { prepared != nil }
    var inspectionScale: CGFloat { scale }
    var inspectionOffset: CGPoint { offset }
    var inspectionFirstFrame: Double? { firstFrameMilliseconds }
    var inspectionRustLayoutCalls: Int { rustLayoutCalls }
    var inspectionSceneSize: CGSize {
        prepared.map { CGSize(width: $0.scene.width, height: $0.scene.height) } ?? .zero
    }
    var inspectionMinimapHidden: Bool { minimapHidden }
    var inspectionMinimapRect: CGRect { minimapRect }
    var inspectionMatches: [String: Any] {
        ["visible": visibleMatches.count, "hidden": hiddenMatches.count, "current": currentMatch]
    }
    var inspectionCollapsed: [String] { collapsed.sorted() }
    var inspectionViewport: CGRect { bounds }
    func inspectionZoom(_ value: CGFloat) { setScale(value, anchor: CGPoint(x: bounds.midX, y: bounds.midY)) }
    func inspectionFit() { fit() }
    func inspectionActual() { actualSize() }
    func inspectionPan(_ dx: CGFloat, _ dy: CGFloat) {
        offset.x += dx; offset.y += dy
        finishTransformChange()
    }
    func inspectionSearch(_ query: String) { search(query) }
    func inspectionNextMatch(_ delta: Int) { nextMatch(delta) }
    func inspectionToggle(_ id: String) { toggleNode(id, anchor: nil) }
    func inspectionPersist() { persistNow() }
    func inspectionMeasure(_ count: Int) -> [Double] { measureFrames(count) }
    func inspectionCurrentMatchFrame() -> CGRect? {
        guard currentMatch >= 0, currentMatch < visibleMatches.count else { return nil }
        let rect = visibleMatches[currentMatch].rect.cgRect
        return CGRect(x: rect.minX * scale + offset.x, y: rect.minY * scale + offset.y,
                      width: rect.width * scale, height: rect.height * scale)
    }
    func inspectionMinimapJumpCenter() {
        minimapJump(to: CGPoint(x: minimapRect.midX, y: minimapRect.midY))
    }
    func inspectionNodeHits() -> [[String: Any]] {
        guard let prepared else { return [] }
        return prepared.scene.hitRegions.map { hit in
            let rect = hit.rect.cgRect
            let sourceRange = prepared.sourceRanges[hit.nodeId] ?? NSRange(location: NSNotFound, length: 0)
            return ["id": hit.nodeId, "label": hit.label, "can_collapse": hit.canCollapse,
                    "collapsed": hit.collapsed,
                    "view_rect": [rect.minX * scale + offset.x, rect.minY * scale + offset.y,
                                  rect.width * scale, rect.height * scale],
                    "source_range": [sourceRange.location, sourceRange.length],
                    "source_start": hit.sourceStart, "source_end": hit.sourceEnd]
        }
    }
    func inspectionZoomTo(_ value: CGFloat, on id: String) {
        guard let hit = prepared?.scene.hitRegions.first(where: { $0.nodeId == id }) else { return }
        scale = clampScale(value)
        offset = CGPoint(x: bounds.midX - hit.rect.cgRect.midX * scale,
                         y: bounds.midY - hit.rect.cgRect.midY * scale)
        finishTransformChange()
    }
    func inspectionHitCenter(_ id: String) -> CGPoint? {
        guard let hit = prepared?.scene.hitRegions.first(where: { $0.nodeId == id }) else { return nil }
        let rect = hit.rect.cgRect
        return CGPoint(x: rect.midX * scale + offset.x, y: rect.midY * scale + offset.y)
    }
    func inspectionSceneScale() -> CGFloat { scale }
    var inspectionViewportCenter: CGPoint? { centerScenePoint() }
    func inspectionExpandHidden() { expandHiddenMatches() }
    func inspectionCommandClick(_ id: String) {
        guard let range = prepared?.sourceRanges[id] else { return }
        onJump?(range)
    }
    func inspectionLinkHits() -> [[String: Any]] {
        guard let prepared else { return [] }
        return prepared.scene.hitRegions.compactMap { hit in
            guard let target = hit.linkTarget else { return nil }
            let rect = hit.rect.cgRect
            return ["id": hit.nodeId, "label": hit.label, "target": target,
                    "heading": hit.linkHeading ?? "", "can_collapse": hit.canCollapse,
                    "collapsed": hit.collapsed,
                    "view_rect": [rect.minX * scale + offset.x, rect.minY * scale + offset.y,
                                  rect.width * scale, rect.height * scale]]
        }
    }
    func inspectionMenuTitles(_ id: String) -> [String] {
        guard let prepared,
              let hit = prepared.scene.hitRegions.first(where: { $0.nodeId == id }),
              let window else { return [] }
        let rect = hit.rect.cgRect
        let point = CGPoint(x: rect.midX * scale + offset.x, y: rect.midY * scale + offset.y)
        guard let event = NSEvent.mouseEvent(
            with: .rightMouseDown, location: convert(point, to: nil), modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil,
            eventNumber: 0, clickCount: 1, pressure: 1
        ) else { return [] }
        return menu(for: event)?.items.map(\.title) ?? []
    }
    func inspectionClick(_ id: String, modifiers: NSEvent.ModifierFlags) {
        guard let prepared,
              let hit = prepared.scene.hitRegions.first(where: { $0.nodeId == id }),
              let window else { return }
        let rect = hit.rect.cgRect
        let point = CGPoint(x: rect.midX * scale + offset.x, y: rect.midY * scale + offset.y)
        guard let event = NSEvent.mouseEvent(
            with: .leftMouseDown, location: convert(point, to: nil), modifierFlags: modifiers,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil,
            eventNumber: 0, clickCount: 1, pressure: 1
        ) else { return }
        mouseDown(with: event)
    }
    func inspectionRowCounts() -> [Int] {
        guard let prepared else { return [] }
        var rows: [Double: Int] = [:]
        for hit in prepared.scene.hitRegions { rows[hit.rect.y, default: 0] += 1 }
        return rows.sorted { $0.key < $1.key }.map(\.value)
    }
    func inspectionSetOffset(_ point: CGPoint) {
        offset = point
        finishTransformChange()
    }
}
#endif
