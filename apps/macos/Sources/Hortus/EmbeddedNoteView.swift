import AppKit
import HortusCore

/// 一次嵌入解析所需的全部只读上下文；卡片只会向后追加祖先路径。
struct EmbedRequest: Sendable {
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

    var maxDepth: Int { 3 }
}

struct PreparedEmbed: Sendable {
    let info: EmbedContentInfo
    let media: [PreparedMedia]
    let math: [PreparedMathBlock]
    let diagrams: [NoteDiagram]
}

/// 嵌入内容在主线程之外准备：解析、切片、图片探测、公式和 Mermaid 布局都不占用窗口线程。
actor EmbedRenderer {
    static let shared = EmbedRenderer()

    func prepare(_ request: EmbedRequest) async -> PreparedEmbed? {
        guard request.width > 0 else { return nil }
        do {
            let info = try request.handle.resolveEmbed(
                sourcePath: request.source, rawTarget: request.target, heading: request.heading,
                alias: request.alias, ancestors: request.ancestors, maxDepth: UInt32(request.maxDepth),
                width: request.width, size: request.size)
            var media: [PreparedMedia] = []
            if info.kind == "note" {
                media = await LocalImageCache.shared.prepare(info.media, vault: request.vault,
                                                            size: request.size, mathService: request.mathService)
            }
            var math: [PreparedMathBlock] = []
            if info.kind == "note", !info.math.isEmpty {
                math = await request.mathService.prepare(info.math, source: info.content, size: request.size,
                                                         width: request.width, displayText: info.text)
            }
            var diagrams: [NoteDiagram] = []
            if info.kind == "diagram" {
                diagrams = [NoteDiagram(ordinal: 1, sourceStart: 0,
                                        range: NSRange(location: 0, length: (info.content as NSString).length),
                                        collapsed: [], tailStart: 0,
                                        documentIdentity: "embed:" + (info.resolvedPath ?? info.content.hashValue.description),
                                        revision: 1)]
            } else if info.kind == "note", !info.diagrams.isEmpty {
                diagrams = info.diagrams.map { item in
                    NoteDiagram(ordinal: item.ordinal, sourceStart: item.startUtf16,
                                range: NSRange(location: Int(item.startUtf16), length: Int(item.lengthUtf16)),
                                collapsed: [], tailStart: 0,
                                documentIdentity: "embed:" + (info.resolvedPath ?? ""), revision: 1)
                }
            }
            if !diagrams.isEmpty {
                diagrams = await request.service.render(source: info.content, mmd: info.isMmd, blocks: diagrams,
                                                        width: max(120, request.width - 28), dark: request.dark)
            }
            return PreparedEmbed(info: info, media: media, math: math, diagrams: diagrams)
        } catch {
            return nil
        }
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
    private var collapsed: Set<String> = []
    private var content: EmbedContentInfo?
    private var task: Task<Void, Never>?
    private var loaded = false
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

    func cancel() { task?.cancel(); task = nil }

    private func load() {
        task = Task { [weak self] in
            guard let self else { return }
            let prepared = await EmbedRenderer.shared.prepare(request)
            guard !Task.isCancelled, let prepared else {
                if !Task.isCancelled { showMessage(L10n.string("Embedded note could not be read")) }
                return
            }
            guard !Task.isCancelled else { return }
            install(prepared)
        }
    }

    private func install(_ prepared: PreparedEmbed) {
        content = prepared.info
        loaded = true
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
            reading.onSizeChange = { [weak self] height in self?.setBody(reading, height: height) }
            body = reading
            addSubview(reading)
            reading.start()
        case "diagram":
            if let diagram = prepared.diagrams.first, let scene = diagram.prepared {
                let view = DiagramView(prepared: scene)
                view.setAccessibilityLabel(L10n.string("Mermaid diagram"))
                view.onToggle = { [weak self] id in self?.toggle(id) }
                diagramView = view
                body = view
                addSubview(view)
                setBody(view, height: scene.scene.height)
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
        guard let info = content, info.isMmd else { return }
        let block = NoteDiagram(ordinal: 1, sourceStart: 0,
                                range: NSRange(location: 0, length: (info.content as NSString).length),
                                collapsed: collapsed, tailStart: 0,
                                documentIdentity: "embed:" + (info.resolvedPath ?? ""), revision: 2)
        let width = max(120, request.width - 28)
        Task { [weak self] in
            guard let self else { return }
            let rendered = await request.service.render(source: info.content, mmd: true, blocks: [block],
                                                        width: width, dark: request.dark)
            guard !Task.isCancelled, let scene = rendered.first?.prepared, let view = diagramView else { return }
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
        }
    }

    override func layout() {
        super.layout()
        let width = max(120, bounds.width)
        titleButton.frame = CGRect(x: 14, y: 6, width: width - 24, height: 20)
        if let body {
            body.frame = CGRect(x: 12, y: titleHeight, width: width - 24,
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
    private var width: CGFloat = 0
    private var reportedHeight: CGFloat = 0

    init(prepared: PreparedEmbed, request: EmbedRequest) {
        self.prepared = prepared
        self.request = request
        width = max(120, CGFloat(request.width) - 28)
        super.init(frame: CGRect(x: 0, y: 0, width: width, height: 1))
        textView.isReading = true
        textView.isEmbedded = true
        textView.isEditable = false
        textView.isSelectable = true
        textView.drawsBackground = false
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.textContainer?.widthTracksTextView = true
        textView.textContainer?.lineFragmentPadding = 0
        textView.textContainerInset = NSSize(width: 14, height: 8)
        textView.frame = CGRect(x: 0, y: 0, width: width, height: 1)
        textView.autoresizingMask = [.width]
        textView.sourceStyling = styling
        textView.textLayoutManager?.delegate = self
        textView.onDocumentHeightChange = { [weak self] in self?.sizeChanged() }
        textView.onWidthOrThemeChange = { [weak self] in self?.relayout() }
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
        var runs: [SourceStyleRun] = []
        for block in info.blocks {
            for run in block.runs {
                runs.append(SourceStyleRun(start: run.start, length: run.length, mask: run.mask))
            }
        }
        styling.installReading(runs, size: request.size)
        textView.textStorage?.setAttributedString(NSAttributedString(string: info.text, attributes: styling.style(0)))
        textView.sourceUTF16Length = (info.text as NSString).length
        let items = prepared.media.map { item in
            NoteMedia(range: NSRange(location: Int(item.block.displayStart), length: Int(item.block.displayLength)),
                      prepared: item)
        }
        textView.installMedia(items, width: Double(width), size: request.size, vault: request.vault,
                              refresh: { [weak self] in self?.reload() }) { [weak self] item, media in
            self?.installEmbedCard(item, media: media)
        }
        let formulas = prepared.math.compactMap { item -> NoteMath? in
            let range = NSRange(location: Int(item.block.displayStart), length: Int(item.block.displayLength))
            guard NSMaxRange(range) <= textView.string.utf16.count else { return nil }
            return NoteMath(range: range, prepared: item, offset: Int(item.block.displayStart))
        }
        textView.installInlineMath(formulas.filter { !$0.prepared.block.block }, width: Double(width),
                                   size: request.size, replacing: NSRange(location: 0, length: textView.string.utf16.count))
        var diagrams = prepared.diagrams
        var ordinal: UInt32 = 0
        for block in info.blocks where block.diagram {
            ordinal += 1
            guard var diagram = diagrams.first(where: { $0.ordinal == ordinal }) else { continue }
            diagram.range = NSRange(location: Int(block.displayStart), length: Int(block.displayLength))
            diagram.tailStart = Int(block.displayStart)
            diagrams[diagrams.firstIndex(where: { $0.ordinal == ordinal })!] = diagram
        }
        textView.install(diagrams)
        textView.installMath(formulas, width: Double(width), size: request.size)
        textView.installFormulaAccessibility(formulas)
        textView.needsLayout = true
        textView.layoutSubtreeIfNeeded()
        sizeChanged()
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
                                  vault: request.vault, width: Double(width), size: request.size,
                                  dark: request.dark, service: request.service, mathService: request.mathService)
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

    private func relayout() {
        textView.frame.size.width = width
        textView.needsLayout = true
        textView.layoutSubtreeIfNeeded()
        sizeChanged()
    }

    private func sizeChanged() {
        let height = max(24, textView.frame.height + 4)
        guard abs(height - reportedHeight) > 0.5 else { return }
        reportedHeight = height
        frame.size = CGSize(width: width, height: height)
        onSizeChange?(height)
    }

    override func layout() {
        super.layout()
        if abs(width - bounds.width) > 0.5 {
            width = max(120, bounds.width)
            relayout()
        }
        textView.frame = CGRect(x: 0, y: 0, width: width, height: max(1, bounds.height))
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
        guard let event = NSApp.currentEvent, let position = textView.knowledgeCharacter(at: event),
              let source = sourcePosition(at: position) else { return }
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
        let blocks = prepared.info.blocks
        guard let block = blocks.first(where: {
            position >= Int($0.displayStart) && position < Int($0.displayStart + $0.displayLength)
        }) else { return nil }
        let local = position - Int(block.displayStart)
        guard let run = block.runs.first(where: {
            local >= Int($0.start) && local < Int($0.start + $0.length)
        }) else { return Int(block.sourceStartUtf16) }
        let base = Int(block.sourceStartUtf16 + run.sourceStartUtf16)
        if run.sourceLengthUtf16 == run.length {
            return base + (local - Int(run.start))
        }
        return base
    }
}

extension EmbeddedReadingView: NSGestureRecognizerDelegate {
    func gestureRecognizer(_ gestureRecognizer: NSGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: NSGestureRecognizer) -> Bool { true }
}

#if DEBUG
extension EmbeddedReadingView {
    var inspectionText: String { textView.string }
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
