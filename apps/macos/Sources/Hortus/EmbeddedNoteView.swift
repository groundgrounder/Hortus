import AppKit
import HortusCore

/// 一次嵌入解析所需的全部只读上下文；卡片只会向后追加祖先路径。
struct EmbedRequest: Sendable {
    struct ReuseKey: Hashable, Sendable {
        let source: String
        let target: String
        let heading: String?
        let alias: String?
        let ancestors: [String]
        let depth: Int
        let vault: String
        let widthBucket: Int
        let sizeBitPattern: UInt64
        let dark: Bool
        let knowledgeVersion: Int
    }

    let source: String
    let target: String
    let heading: String?
    let alias: String?
    let ancestors: [String]
    let depth: Int
    let handle: VaultHandle
    let vault: String
    let width: Double
    let size: Double
    let dark: Bool
    let service: DiagramService
    let mathService: MathService
    var knowledgeVersion: Int = 0

    var reuseKey: ReuseKey {
        ReuseKey(source: source, target: target, heading: heading, alias: alias, ancestors: ancestors,
                 depth: depth, vault: vault, widthBucket: Int((width * 2).rounded()),
                 sizeBitPattern: size.bitPattern, dark: dark, knowledgeVersion: knowledgeVersion)
    }

    var maxDepth: Int { 3 }
    static func bodyWidth(_ width: Double) -> Double { max(120, width - 24) }
    static func contentWidth(_ width: Double) -> Double { max(120, bodyWidth(width) - 28) }

    func resolveDiagramLink(from path: String?, target: String) async -> String? {
        guard let path else { return nil }
        return await Task.detached { handle.resolveLink(sourcePath: path, rawTarget: target).resolvedPath }.value
    }

    @MainActor
    func diagramExport(info: EmbedContentInfo, block: NoteDiagram) -> DiagramExportRequest? {
        guard let path = info.resolvedPath, let scene = block.prepared else { return nil }
        return DiagramExportRequest(prepared: scene, note: path, ordinal: block.ordinal) {
            try await Task.detached(priority: .userInitiated) {
                try DiagramExport.renderSVG(prepared: scene)
            }.value
        }
    }
}

struct PreparedEmbed: Sendable {
    let info: EmbedContentInfo
    let media: [PreparedMedia]
    let math: [PreparedMathBlock]
    let diagrams: [NoteDiagram]
    /// 切片首行在整篇笔记中的零基偏移；未能匹配同版内容时省略行号。
    let precedingLines: Int?
}

/// 嵌入内容在主线程之外准备：解析、切片、图片探测、公式和 Mermaid 布局都不占用窗口线程。
actor EmbedRenderer {
    static let shared = EmbedRenderer()
    private static let maxCachedEntries = 32
    private var cache: [EmbedRequest.ReuseKey: PreparedEmbed] = [:]
    private var order: [EmbedRequest.ReuseKey] = []
    private var vaultVersions: [String: Int] = [:]
    #if DEBUG
    private(set) var prepareCallCount = 0
    private(set) var cacheHitCount = 0
    #endif

    func clearCache() {
        cache.removeAll(keepingCapacity: false)
        order.removeAll(keepingCapacity: false)
        vaultVersions.removeAll(keepingCapacity: false)
    }

    /// 驱逐包含失败媒体（如缺失图片或嵌套嵌入）的缓存项，以便外部补齐附件后重新准备。
    func invalidateFailedMedia() {
        let failed = order.filter { key in
            cache[key]?.media.contains(where: { !$0.reason.isEmpty }) == true
        }
        for key in failed {
            cache.removeValue(forKey: key)
        }
        let failedSet = Set(failed)
        order.removeAll { failedSet.contains($0) }
    }

    #if DEBUG
    func resetMetrics() {
        prepareCallCount = 0
        cacheHitCount = 0
    }
    #endif

    private func touch(_ key: EmbedRequest.ReuseKey, value: PreparedEmbed) {
        cache[key] = value
        order.removeAll { $0 == key }
        order.append(key)
        while order.count > Self.maxCachedEntries {
            let evicted = order.removeFirst()
            cache.removeValue(forKey: evicted)
        }
    }

    private func invalidateStaleVault(_ vault: String, version: Int) {
        if let previous = vaultVersions[vault], previous != version {
            let stale = order.filter { $0.vault == vault && $0.knowledgeVersion != version }
            for key in stale { cache.removeValue(forKey: key) }
            order.removeAll { $0.vault == vault && $0.knowledgeVersion != version }
        }
        vaultVersions[vault] = version
    }

    /// 64 位 FNV-1a 内容指纹：避免同路径不同 heading/修订的嵌入在 DiagramService 中发生身份碰撞。
    static func contentFingerprint(_ text: String) -> String {
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in text.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x100000001b3
        }
        return String(hash, radix: 16)
    }

    func prepare(_ request: EmbedRequest) async -> PreparedEmbed? {
        guard request.width > 0, !Task.isCancelled else { return nil }
        #if DEBUG
        prepareCallCount += 1
        #endif
        invalidateStaleVault(request.vault, version: request.knowledgeVersion)
        let key = request.reuseKey
        if let cached = cache[key] {
            order.removeAll { $0 == key }
            order.append(key)
            #if DEBUG
            cacheHitCount += 1
            #endif
            return cached
        }
        do {
            let width = EmbedRequest.contentWidth(request.width)
            let info = try request.handle.resolveEmbed(
                sourcePath: request.source, rawTarget: request.target, heading: request.heading,
                alias: request.alias, ancestors: request.ancestors, maxDepth: UInt32(request.maxDepth),
                width: width, size: request.size)
            guard !Task.isCancelled else { return nil }
            var media: [PreparedMedia] = []
            if info.kind == "note" {
                media = await LocalImageCache.shared.prepare(info.media, vault: request.vault,
                                                            size: request.size, mathService: request.mathService)
            }
            guard !Task.isCancelled else { return nil }
            var math: [PreparedMathBlock] = []
            if info.kind == "note", !info.math.isEmpty {
                math = await request.mathService.prepare(info.math, source: info.content, size: request.size,
                                                         width: width, displayText: info.text)
            }
            guard !Task.isCancelled else { return nil }
            let sliceIdentity = "embed:" + (info.resolvedPath ?? "") + "#" + (request.heading ?? "")
                + ":" + Self.contentFingerprint(info.content)
            var diagrams: [NoteDiagram] = []
            if info.kind == "diagram" {
                diagrams = [NoteDiagram(ordinal: 1, sourceStart: 0,
                                        range: NSRange(location: 0, length: (info.content as NSString).length),
                                        collapsed: [], tailStart: 0,
                                        documentIdentity: sliceIdentity,
                                        revision: 1)]
            } else if info.kind == "note", !info.diagrams.isEmpty {
                diagrams = info.diagrams.map { item in
                    NoteDiagram(ordinal: item.ordinal, sourceStart: item.startUtf16,
                                range: NSRange(location: Int(item.startUtf16), length: Int(item.lengthUtf16)),
                                collapsed: [], tailStart: 0,
                                documentIdentity: sliceIdentity, revision: 1)
                }
            }
            if !diagrams.isEmpty {
                guard !Task.isCancelled else { return nil }
                diagrams = await request.service.render(source: info.content, mmd: info.isMmd, blocks: diagrams,
                    width: info.isMmd ? EmbedRequest.bodyWidth(request.width) : width, dark: request.dark)
            }
            guard !Task.isCancelled else { return nil }
            let precedingLines = info.kind == "note" ? self.precedingLines(for: info, request: request) : nil
            guard !Task.isCancelled else { return nil }
            let prepared = PreparedEmbed(info: info, media: media, math: math, diagrams: diagrams,
                                         precedingLines: precedingLines)
            touch(key, value: prepared)
            return prepared
        } catch {
            return nil
        }
    }

    private func precedingLines(for info: EmbedContentInfo, request: EmbedRequest) -> Int? {
        guard let path = info.resolvedPath, let source = try? request.handle.readNote(path: path) else { return nil }
        let text = source as NSString
        let match = text.range(of: info.content, options: request.heading == nil ? [.anchored, .backwards, .literal] : .literal)
        guard match.location != NSNotFound else { return nil }
        return SendToTerminalSelection.lineNumber(in: text, offset: match.location) - 1
    }
}

/// 阅读视图里的原生嵌入卡片：标题栏可跳转，正文沿用阅读排版，边界状态退化为提示条。
@MainActor
final class EmbeddedNoteView: NSView {
    let request: EmbedRequest
    private(set) var preferredSize = CGSize(width: 0, height: 48)
    var onHeightChange: (() -> Void)?
    var onOpen: ((String, String?) -> Void)?
    var onOpenLink: ((String, String?) -> Void)?
    var onShowTag: ((String) -> Void)?

    private let titleButton = NSButton()
    private let message = NSTextField(labelWithString: "")
    private var body: NSView?
    private weak var diagramView: DiagramView?
    private var diagram: NoteDiagram?
    private var collapsed: Set<String> = []
    private var diagramTask: Task<Void, Never>?
    private var content: EmbedContentInfo?
    private var task: Task<Void, Never>?
    private let titleHeight: CGFloat = 30

    var cardAccessibilityLabel: String {
        var label = content?.displayTitle ?? request.alias ?? request.target
        if let heading = content?.heading ?? request.heading, !heading.isEmpty {
            label += " › " + heading
        }
        if let path = content?.resolvedPath {
            label += ", " + path
        }
        return label
    }

    init(_ request: EmbedRequest) {
        self.request = request
        preferredSize.width = request.width
        super.init(frame: CGRect(origin: .zero, size: preferredSize))
        wantsLayer = true
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(request.alias ?? request.target)
        titleButton.isBordered = false
        titleButton.bezelStyle = .inline
        titleButton.alignment = .left
        titleButton.font = .systemFont(ofSize: 12.5, weight: .semibold)
        titleButton.imagePosition = .imageLeading
        titleButton.image = NSImage(systemSymbolName: "doc.text", accessibilityDescription: nil)
        titleButton.image?.size = NSSize(width: 12, height: 14)
        titleButton.target = self
        titleButton.action = #selector(openSource)
        titleButton.title = request.alias ?? request.target
        titleButton.toolTip = request.target
        addSubview(titleButton)
        message.font = .systemFont(ofSize: 12)
        message.textColor = .secondaryLabelColor
        message.lineBreakMode = .byTruncatingTail
        message.isHidden = true
        addSubview(message)
        load()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override var isFlipped: Bool { true }

    func cancel() {
        task?.cancel(); task = nil
        diagramTask?.cancel(); diagramTask = nil
        (body as? EmbeddedReadingView)?.cancel()
    }

    var hasMissingImageMedia: Bool {
        (body as? EmbeddedReadingView)?.hasMissingImageMedia == true
    }

    func reload() {
        cancel()
        load()
    }

    private func load() {
        task = Task { [weak self] in
            guard let self else { return }
            let prepared = await EmbedRenderer.shared.prepare(request)
            guard !Task.isCancelled, let prepared else {
                if !Task.isCancelled { showMessage(L10n.string("Embedded note could not be read")) }
                return
            }
            install(prepared)
        }
    }

    private func install(_ prepared: PreparedEmbed) {
        content = prepared.info
        titleButton.title = title(for: prepared.info)
        titleButton.toolTip = prepared.info.resolvedPath ?? request.target
        titleButton.image = NSImage(systemSymbolName: prepared.info.isMmd ? "chart.xyaxis.line" : "doc.text",
                                    accessibilityDescription: nil)
        titleButton.isEnabled = prepared.info.resolvedPath != nil
        setAccessibilityLabel(cardAccessibilityLabel)
        switch prepared.info.kind {
        case "note":
            let reading = EmbeddedReadingView(prepared: prepared, request: request)
            reading.onOpenLink = { [weak self] path, heading in self?.onOpenLink?(path, heading) }
            reading.onShowTag = { [weak self] tag in self?.onShowTag?(tag) }
            reading.onSizeChange = { [weak self, weak reading] height in
                guard let reading else { return }
                self?.setBody(reading, height: height)
            }
            body = reading
            addSubview(reading)
            reading.start()
        case "diagram":
            if let diagram = prepared.diagrams.first, let scene = diagram.prepared {
                let view = DiagramView(prepared: scene)
                view.setAccessibilityLabel(L10n.string("Mermaid diagram"))
                view.onToggle = { [weak self] id in self?.toggle(id) }
                view.onOpenLink = { [weak self] target, heading in
                    Task { [weak self] in
                        guard let path = await self?.request.resolveDiagramLink(
                            from: prepared.info.resolvedPath, target: target) else { return }
                        self?.onOpenLink?(path, heading)
                    }
                }
                view.exportRequestProvider = { [weak self] in
                    guard let self, let block = self.diagram else { return nil }
                    return request.diagramExport(info: prepared.info, block: block)
                }
                self.diagram = diagram
                diagramView = view
                body = view
                addSubview(view)
                setBody(view, height: scene.scene.height)
                if abs(scene.scene.width - EmbedRequest.bodyWidth(bounds.width)) > 0.5 {
                    rerenderDiagram()
                }
            } else {
                let fallback = sourceFallback(prepared.info.content,
                                              reason: prepared.diagrams.first?.reason ?? "")
                body = fallback
                addSubview(fallback)
                setBody(fallback, height: min(max(fallback.intrinsicContentSize.height, 44), 420))
                message.isHidden = true
            }
            if let path = prepared.info.resolvedPath { titleButton.toolTip = path }
        case "cycle":
            showMessage(L10n.string("Circular embed: %@", prepared.info.displayTitle))
        case "depth_exceeded":
            showMessage(L10n.string("Embedding is too deep: %@", prepared.info.displayTitle))
        case "heading_missing":
            showMessage(L10n.string("Heading not found: %@", prepared.info.heading ?? request.heading ?? ""))
        case "ambiguous":
            showMessage(L10n.string("Several notes match “%@”: %@", request.target,
                                    prepared.info.candidates.prefix(3).joined(separator: ", ")))
        case "broken":
            showMessage(L10n.string("No note found for “%@”", request.target))
        default:
            showMessage(L10n.string("Embedding is unavailable: %@", request.target))
        }
        needsLayout = true
    }

    private func title(for info: EmbedContentInfo) -> String {
        var value = info.displayTitle.isEmpty ? request.target : info.displayTitle
        if let heading = info.heading, !heading.isEmpty { value += " › " + heading }
        return value
    }

    private func toggle(_ id: String) {
        if collapsed.contains(id) { collapsed.remove(id) } else { collapsed.insert(id) }
        rerenderDiagram()
    }

    private func rerenderDiagram() {
        guard let info = content, var block = diagram else { return }
        block.collapsed = collapsed
        let width = EmbedRequest.bodyWidth(bounds.width)
        diagramTask?.cancel()
        diagramTask = Task { [weak self, request] in
            let rendered = await request.service.render(source: info.content, mmd: true, blocks: [block],
                                                        width: width, dark: request.dark)
            guard !Task.isCancelled, let self, let block = rendered.first,
                  let scene = block.prepared, let view = diagramView else { return }
            self.diagram = block
            view.update(scene)
            setBody(view, height: scene.scene.height)
        }
    }

    private func showMessage(_ text: String) {
        message.stringValue = text
        message.isHidden = false
        setBody(nil, height: 30)
    }

    private func sourceFallback(_ source: String, reason: String) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.hasHorizontalScroller = false
        let text = NSTextView()
        text.isEditable = false
        text.isSelectable = true
        text.drawsBackground = false
        text.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        text.string = reason.isEmpty ? source : L10n.string("Diagram could not be rendered") + "\n" + reason + "\n\n" + source
        text.textContainerInset = NSSize(width: 6, height: 4)
        scroll.documentView = text
        return scroll
    }

    private func setBody(_ view: NSView?, height: CGFloat) {
        if body !== view {
            body?.removeFromSuperview()
            body = view
        }
        let bodyHeight = max(24, height)
        message.isHidden = view != nil
        let next = CGSize(width: preferredSize.width, height: titleHeight + bodyHeight + 10)
        guard abs(preferredSize.height - next.height) > 0.5 || abs(frame.width - next.width) > 0.5 else {
            needsLayout = true
            return
        }
        preferredSize = next
        frame.size = next
        needsLayout = true
        onHeightChange?()
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        if abs(preferredSize.width - newSize.width) > 0.5 {
            preferredSize.width = newSize.width
            needsLayout = true
            if diagram != nil { rerenderDiagram() }
        }
    }

    override func layout() {
        super.layout()
        let width = max(120, bounds.width)
        titleButton.frame = CGRect(x: 14, y: 6, width: width - 24, height: 20)
        if let body {
            body.frame = CGRect(x: 12, y: titleHeight, width: EmbedRequest.bodyWidth(bounds.width),
                                height: max(0, bounds.height - titleHeight - 8))
        } else {
            message.frame = CGRect(x: 14, y: titleHeight, width: width - 24, height: 20)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let rect = bounds.insetBy(dx: 0.5, dy: 0.5)
        NSColor.controlBackgroundColor.withAlphaComponent(0.48).setFill()
        let card = NSBezierPath(roundedRect: rect, xRadius: 8, yRadius: 8)
        card.fill()
        NSColor.separatorColor.withAlphaComponent(0.7).setStroke()
        card.lineWidth = 1
        card.stroke()
        let rule = NSBezierPath(roundedRect: CGRect(x: 1.5, y: 6, width: 3, height: max(0, bounds.height - 12)),
                                xRadius: 1.5, yRadius: 1.5)
        NSColor.controlAccentColor.withAlphaComponent(0.42).setFill()
        rule.fill()
    }

    @objc private func openSource() {
        guard let path = content?.resolvedPath else { return }
        onOpen?(path, content?.heading ?? request.heading)
    }
}

#if DEBUG
extension EmbeddedNoteView {
    var inspectionKind: String? { content?.kind }
    var inspectionResolvedPath: String? { content?.resolvedPath }
    var inspectionContent: String? { content?.content }
    var inspectionTitle: String { titleButton.title }
    var inspectionBodyText: String {
        if let reading = body as? EmbeddedReadingView { return reading.inspectionText }
        return message.isHidden ? "" : message.stringValue
    }
    var inspectionMessage: String? { message.isHidden ? nil : message.stringValue }
    var inspectionHasDiagram: Bool { ((body as? DiagramView)?.prepared.scene.height ?? 0) > 0 }
    var inspectionDiagramView: DiagramView? { diagramView }
    var inspectionReadingView: EmbeddedReadingView? { body as? EmbeddedReadingView }
    var inspectionEmbeddedCards: [EmbeddedNoteView] {
        if let reading = body as? EmbeddedReadingView { return reading.inspectionCards }
        return []
    }
    var inspectionMediaCount: Int { (body as? EmbeddedReadingView)?.inspectionMediaCount ?? 0 }
    var inspectionMathCount: Int { (body as? EmbeddedReadingView)?.inspectionMathCount ?? 0 }
    var inspectionDiagramCount: Int { (body as? EmbeddedReadingView)?.inspectionDiagramCount ?? 0 }
    var inspectionHasTable: Bool { (body as? EmbeddedReadingView)?.inspectionHasTable ?? false }
    var inspectionTitleFrameInWindow: NSRect? {
        guard let window else { return nil }
        return window.convertToScreen(titleButton.convert(titleButton.bounds, to: nil))
    }
    func inspectionOpen() { openSource() }
}
#endif

/// 卡片正文：复用阅读视图的 TextKit 排版、媒体、图与公式安装路径。
@MainActor
final class EmbeddedReadingView: NSView, @preconcurrency NSTextLayoutManagerDelegate {
    private let textView = DiagramTextView(diagramLayout: true)
    private let styling = SourceStyling()
    private let prepared: PreparedEmbed
    private let request: EmbedRequest
    var onOpenLink: ((String, String?) -> Void)?
    var onShowTag: ((String) -> Void)?
    var onSizeChange: ((CGFloat) -> Void)?
    private var width: CGFloat
    private var displayRanges: [NSRange] = []
    private var fallbackBlocks: Set<Int> = []
    private var diagrams: [NoteDiagram] = []
    private var pendingCollapses: [UInt32: Set<String>] = [:]
    private var diagramTasks: [UInt32: Task<Void, Never>] = [:]
    private var reportedHeight: CGFloat = 0

    init(prepared: PreparedEmbed, request: EmbedRequest) {
        self.prepared = prepared
        self.request = request
        width = EmbedRequest.bodyWidth(request.width)
        super.init(frame: CGRect(x: 0, y: 0, width: width, height: 1))
        textView.isReading = true
        textView.isEmbedded = true
        textView.embeddedTerminalSelection = { [weak self] in self?.terminalSelection() }
        textView.isEditable = false
        textView.isSelectable = true
        textView.drawsBackground = false
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.textContainer?.widthTracksTextView = true
        textView.textContainer?.lineFragmentPadding = 0
        textView.textContainerInset = NSSize(width: Self.textInset(for: width), height: 8)
        textView.frame = CGRect(x: 0, y: 0, width: width, height: 1)
        textView.autoresizingMask = [.width]
        textView.sourceStyling = styling
        textView.textLayoutManager?.delegate = self
        textView.onDocumentHeightChange = { [weak self] in self?.sizeChanged() }
        addSubview(textView)
        let click = NSClickGestureRecognizer(target: self, action: #selector(handleClick(_:)))
        click.delaysPrimaryMouseButtonEvents = false
        click.delegate = self
        textView.addGestureRecognizer(click)
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func start() {
        let info = prepared.info
        let source = info.content as NSString
        let display = NSMutableString(string: info.text)
        let hasProperties = info.text.hasPrefix("\u{e000}")
        var runs: [SourceStyleRun] = []
        var ordinal: UInt32 = 0
        var shift = 0
        for (index, block) in info.blocks.enumerated() {
            let original = NSRange(location: Int(block.displayStart) + shift, length: Int(block.displayLength))
            var range = original
            var labelDelta = 0
            if block.diagram {
                ordinal += 1
                if var diagram = prepared.diagrams.first(where: { $0.ordinal == ordinal }), diagram.prepared != nil {
                    diagram.range = range
                    diagram.tailStart = range.location
                    diagrams.append(diagram)
                } else if NSMaxRange(original) <= display.length {
                    let sourceRange = NSRange(location: Int(block.sourceStartUtf16), length: Int(block.sourceLengthUtf16))
                    let replacement = NSMaxRange(sourceRange) <= source.length ? source.substring(with: sourceRange) : ""
                    display.replaceCharacters(in: original, with: replacement)
                    range.length = (replacement as NSString).length
                    shift += range.length - original.length
                    fallbackBlocks.insert(index)
                }
            } else if hasProperties && block.displayStart == 0 && original.location < display.length {
                let label = L10n.string("Properties")
                labelDelta = (label as NSString).length - 1
                display.replaceCharacters(in: NSRange(location: original.location, length: 1), with: label)
                range.length += labelDelta
                shift += labelDelta
            }
            displayRanges.append(range)
            if fallbackBlocks.contains(index) {
                runs.append(SourceStyleRun(start: UInt64(range.location), length: UInt64(range.length), mask: 32))
            } else {
                for run in block.runs {
                    var start = Int(run.start)
                    var length = Int(run.length)
                    if labelDelta != 0 {
                        if start == 0 && run.mask == 16384 { length += labelDelta }
                        else if start >= 1 { start += labelDelta }
                    }
                    runs.append(SourceStyleRun(start: UInt64(range.location + start), length: UInt64(length), mask: run.mask))
                }
            }
        }
        styling.installReading(runs, size: request.size)
        textView.textStorage?.setAttributedString(NSAttributedString(string: display as String, attributes: styling.style(0)))
        textView.sourceUTF16Length = display.length
        textView.onOpenLink = { [weak self] _, target, heading in
            Task { [weak self] in
                guard let path = await self?.request.resolveDiagramLink(from: info.resolvedPath, target: target) else { return }
                self?.onOpenLink?(path, heading)
            }
        }
        textView.exportRequestProvider = { [weak self] ordinal in
            guard let self, let block = diagrams.first(where: { $0.ordinal == ordinal }) else { return nil }
            return request.diagramExport(info: info, block: block)
        }
        textView.onToggle = { [weak self] ordinal, id in self?.toggle(ordinal: ordinal, id: id) }
        installAttachments()
        textView.install(diagrams)
        textView.needsLayout = true
        textView.layoutSubtreeIfNeeded()
        sizeChanged()
    }

    func cancel() {
        diagramTasks.values.forEach { $0.cancel() }
        diagramTasks.removeAll()
        for media in textView.mediaViews { media.detach() }
    }

    var hasMissingImageMedia: Bool {
        textView.mediaViews.contains { $0.hasMissingImageMedia }
    }

    private func toggle(ordinal: UInt32, id: String) {
        guard let block = diagrams.first(where: { $0.ordinal == ordinal }) else { return }
        var collapsed = pendingCollapses[ordinal] ?? block.collapsed
        if collapsed.contains(id) { collapsed.remove(id) } else { collapsed.insert(id) }
        pendingCollapses[ordinal] = collapsed
        rerender(block)
    }

    private func rerender(_ diagram: NoteDiagram) {
        var block = diagram
        let ordinal = block.ordinal
        block.collapsed = pendingCollapses[ordinal] ?? diagram.collapsed
        let width = textView.layoutContentWidth
        let source = prepared.info.content
        diagramTasks[ordinal]?.cancel()
        diagramTasks[ordinal] = Task { [weak self, request] in
            let rendered = await request.service.render(source: source, mmd: false, blocks: [block],
                                                        width: width, dark: request.dark)
            guard !Task.isCancelled, let self,
                  let index = diagrams.firstIndex(where: { $0.ordinal == ordinal }) else { return }
            diagramTasks[ordinal] = nil
            pendingCollapses[ordinal] = nil
            guard let result = rendered.first, result.reason == nil, result.prepared != nil else { return }
            diagrams[index] = result
            textView.install(diagrams)
            textView.layoutSubtreeIfNeeded()
            sizeChanged()
        }
    }

    private func displayRange(start: UInt64, length: UInt64) -> NSRange? {
        guard let index = prepared.info.blocks.firstIndex(where: {
            start >= $0.displayStart && start < $0.displayStart + $0.displayLength
        }), !fallbackBlocks.contains(index) else { return nil }
        let location = displayRanges[index].location + Int(start - prepared.info.blocks[index].displayStart)
        guard location + Int(length) <= textView.sourceUTF16Length else { return nil }
        return NSRange(location: location, length: Int(length))
    }

    private func installAttachments() {
        let available = textView.layoutContentWidth
        let items = prepared.media.compactMap { item -> NoteMedia? in
            guard let range = displayRange(start: item.block.displayStart, length: item.block.displayLength) else { return nil }
            return NoteMedia(range: range, prepared: item)
        }
        textView.installMedia(items, width: available, size: request.size, vault: request.vault,
                              refresh: { [weak self] in self?.reload() }) { [weak self] item, media in
            self?.installEmbedCard(item, media: media)
        }
        let formulas = prepared.math.compactMap { item -> NoteMath? in
            guard let range = displayRange(start: item.block.displayStart, length: item.block.displayLength) else { return nil }
            return NoteMath(range: range, prepared: item, offset: range.location)
        }
        textView.installInlineMath(formulas.filter { !$0.prepared.block.block }, width: available,
                                   size: request.size, replacing: NSRange(location: 0, length: textView.sourceUTF16Length))
        textView.installMath(formulas, width: available, size: request.size)
        textView.installFormulaAccessibility(formulas)
    }

    private func installEmbedCard(_ item: NoteMedia, media: MediaView) {
        guard item.prepared.block.imageWiki, item.prepared.reason.hasPrefix("embed."),
              let path = prepared.info.resolvedPath,
              let offset = sourceUTF16(byte: Int(item.prepared.block.sourceStart)),
              let link = prepared.info.links.first(where: {
                  $0.embed && !$0.attachment && !$0.inDiagram
                      && UInt64(offset) >= $0.startUtf16 && UInt64(offset) < $0.startUtf16 + $0.lengthUtf16
              }) else { return }
        var ancestors = request.ancestors
        ancestors.append(path)
        let nested = EmbedRequest(source: path, target: link.target, heading: link.heading, alias: link.alias,
                                  ancestors: ancestors, depth: request.depth + 1, handle: request.handle,
                                  vault: request.vault, width: textView.layoutContentWidth, size: request.size,
                                  dark: request.dark, service: request.service, mathService: request.mathService,
                                  knowledgeVersion: request.knowledgeVersion)
        if let existing = media.existingEmbed(matching: nested.reuseKey) {
            existing.onOpen = { [weak self] target, heading in self?.onOpenLink?(target, heading) }
            existing.onOpenLink = { [weak self] target, heading in self?.onOpenLink?(target, heading) }
            existing.onShowTag = { [weak self] tag in self?.onShowTag?(tag) }
            return
        }
        let card = EmbeddedNoteView(nested)
        card.onOpen = { [weak self] target, heading in self?.onOpenLink?(target, heading) }
        card.onOpenLink = { [weak self] target, heading in self?.onOpenLink?(target, heading) }
        card.onShowTag = { [weak self] tag in self?.onShowTag?(tag) }
        media.installEmbed(card)
    }

    private func sourceUTF16(byte: Int) -> Int? {
        guard byte >= 0, byte <= prepared.info.content.utf8.count else { return nil }
        let utf8 = prepared.info.content.utf8
        guard let end = utf8.index(utf8.startIndex, offsetBy: byte, limitedBy: utf8.endIndex) else { return nil }
        return String(decoding: utf8[utf8.startIndex..<end], as: UTF8.self).utf16.count
    }

    private func reload() {
        onSizeChange?(reportedHeight)
    }

    private func sizeChanged() {
        let height = max(24, textView.frame.height + 4)
        guard abs(height - reportedHeight) > 0.5 else { return }
        reportedHeight = height
        onSizeChange?(height)
    }

    override func layout() {
        super.layout()
        guard abs(width - bounds.width) > 0.5 else { return }
        width = bounds.width
        textView.textContainerInset.width = Self.textInset(for: width)
        textView.frame.size.width = width
        installAttachments()
        for diagram in diagrams { rerender(diagram) }
        textView.needsLayout = true
    }

    private static func textInset(for width: CGFloat) -> CGFloat {
        min(14, max(0, (width - 120) / 2))
    }

    func textLayoutManager(_ manager: NSTextLayoutManager, textLayoutFragmentFor location: any NSTextLocation,
                           in element: NSTextElement) -> NSTextLayoutFragment {
        let fragment = NoteLayoutFragment(textElement: element, range: nil)
        if let content = manager.textContentManager {
            textView.configure(fragment, at: content.offset(from: content.documentRange.location, to: location))
        }
        return fragment
    }

    @objc private func handleClick(_ recognizer: NSClickGestureRecognizer) {
        guard let event = NSApp.currentEvent, let position = textView.knowledgeCharacter(at: event) else { return }
        activate(at: position)
    }

    private func activate(at position: Int) {
        guard let source = sourcePosition(at: position) else { return }
        let mask = styling.mask(at: position)
        if mask & (1 << 18) != 0, let link = prepared.info.links.first(where: {
            !$0.inDiagram && !$0.attachment && UInt64(source) >= $0.startUtf16
                && UInt64(source) < $0.startUtf16 + $0.lengthUtf16
        }), let path = link.resolvedPath {
            onOpenLink?(path, link.heading)
            return
        }
        if mask & 512 != 0, let tag = prepared.info.tags.first(where: {
            UInt64(source) >= $0.startUtf16 && UInt64(source) < $0.startUtf16 + $0.lengthUtf16
        }) {
            onShowTag?(tag.tag)
        }
    }

    private func sourcePosition(at position: Int) -> Int? {
        guard let index = displayRanges.firstIndex(where: { NSLocationInRange(position, $0) }) else { return nil }
        let block = prepared.info.blocks[index]
        let local = position - displayRanges[index].location
        if fallbackBlocks.contains(index) { return Int(block.sourceStartUtf16) + local }
        guard let run = block.runs.first(where: {
            local >= Int($0.start) && local < Int($0.start + $0.length)
        }) else { return Int(block.sourceStartUtf16) }
        let base = Int(block.sourceStartUtf16 + run.sourceStartUtf16)
        if run.sourceLengthUtf16 == run.length {
            return base + (local - Int(run.start))
        }
        return base
    }

    private func terminalSelection() -> SendToTerminalSelection.Selection? {
        let range = textView.selectedRange()
        let displayed = textView.string as NSString
        guard let path = prepared.info.resolvedPath, range.location != NSNotFound, range.length > 0,
              NSMaxRange(range) <= displayed.length else { return nil }
        let text = displayed.substring(with: range)
        var lines: ClosedRange<Int>?
        if let preceding = prepared.precedingLines, let start = sourcePosition(at: range.location),
           let end = sourcePosition(at: NSMaxRange(range) - 1),
           let local = SendToTerminalSelection.lineRange(in: prepared.info.content as NSString, start: start, end: end + 1) {
            lines = (local.lowerBound + preceding)...(local.upperBound + preceding)
        }
        return (path, text, lines)
    }
}

extension EmbeddedReadingView: NSGestureRecognizerDelegate {
    func gestureRecognizer(_ gestureRecognizer: NSGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: NSGestureRecognizer) -> Bool { true }
}

#if DEBUG
extension EmbeddedReadingView {
    var inspectionTextView: DiagramTextView { textView }
    var inspectionText: String { textView.string }
    func inspectionSourcePosition(at index: Int) -> Int? { sourcePosition(at: index) }
    func inspectionActivate(at index: Int) { activate(at: index) }
    var inspectionCards: [EmbeddedNoteView] {
        textView.mediaViews.compactMap(\.inspectionEmbeddedCard)
    }
    var inspectionTextHeight: Double { textView.frame.height }
    var inspectionMediaCount: Int { textView.mediaViews.count }
    var inspectionMathCount: Int { textView.mathViews.count + textView.inlineMath.count }
    var inspectionDiagramCount: Int { textView.diagramViews.count }
    var inspectionHasTable: Bool { textView.mediaBlocks.contains { !$0.prepared.block.columns.isEmpty } }
}
#endif
