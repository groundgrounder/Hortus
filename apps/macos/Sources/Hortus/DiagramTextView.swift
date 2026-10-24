import AppKit
import SwiftUI
import HortusCore

struct NoteText: NSViewRepresentable {
    let text, path, vault: String
    let fontSize: Double
    let service: DiagramService
    let core: WorkspaceCore?
    var document: NoteDocument? = nil
    let remember: (UInt32, [String]) -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        let view = DiagramTextView(diagramLayout: true)
        view.isSelectable = true
        view.isRichText = false
        view.usesFindBar = true
        view.allowsUndo = true
        view.isAutomaticQuoteSubstitutionEnabled = false
        view.isAutomaticDashSubstitutionEnabled = false
        view.isAutomaticTextReplacementEnabled = false
        view.isAutomaticSpellingCorrectionEnabled = false
        view.drawsBackground = false
        view.isVerticallyResizable = true
        view.isHorizontallyResizable = false
        view.textContainerInset = NSSize(width: 30, height: 24)
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = true
        view.autoresizingMask = [.width]
        scroll.documentView = view
        context.coordinator.view = view
        view.editingCoordinator = context.coordinator
        return scroll
    }
    func updateNSView(_ scroll: NSScrollView, context: Context) { context.coordinator.update(self) }
    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.detach()
        guard let view = scroll.documentView as? DiagramTextView else { return }
        view.editingCoordinator = nil
        view.onWidthOrThemeChange = nil; view.onToggle = nil; view.onEditSource = nil
        view.onCompositionState = nil; view.onCommit = nil
        view.delegate = nil; view.textStorage?.delegate = nil
        for diagram in view.diagramViews.values { diagram.detach() }
        for media in view.mediaViews { media.detach(); media.removeFromSuperview() }
        view.mediaViews = []; view.mediaBlocks = []
        view.diagramViews = [:]
        view.string = ""
        view.textLayoutManager?.delegate = nil
        if let manager = view.textLayoutManager { manager.textContentManager?.removeTextLayoutManager(manager) }
        view.diagramStorage = nil; scroll.documentView = nil; coordinator.view = nil
        NSWorkspace.shared.notificationCenter.removeObserver(view,
            name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil)
    }

    @MainActor
    final class Coordinator: NSObject, @preconcurrency NSTextLayoutManagerDelegate,
                             @preconcurrency NSTextStorageDelegate, NSTextViewDelegate {
        weak var view: DiagramTextView?
        var task: Task<Void, Never>?
        let styling = SourceStyling()
        private var input: NoteText?
        var document: NoteDocument? { input?.document }
        var retainedBlockCount: Int { blocks.count }
        private var blocks: [NoteDiagram] = []
        private var generation = 0
        private var lastWidth = 0.0
        private var lastDark = false
        private var suppress = false
        private var loading = false
        private var suppressSelection = false
        private var changeScheduled = false

        func detach() {
            task?.cancel(); task = nil; generation += 1
            styling.reset()
            if document?.viewOwner === self {
                document?.onReload = nil
                document?.onClosing = nil; document?.onStateChange = nil
                document?.viewOwner = nil
            }
            view?.editingCoordinator = nil
            view?.mediaViews.forEach { $0.detach(); $0.removeFromSuperview() }
            view?.mediaViews = []; view?.mediaBlocks = []
            input = nil; blocks = []
        }

        func update(_ input: NoteText) {
            let changed = self.input?.path != input.path || self.input?.vault != input.vault
                || self.input?.document !== input.document || (input.document == nil && self.input?.text != input.text)
            if changed, self.input?.document?.viewOwner === self {
                self.input?.document?.onReload = nil
                self.input?.document?.onClosing = nil; self.input?.document?.onStateChange = nil
            }
            self.input = input
            input.document?.viewOwner = self
            guard let view else { return }
            view.representedPath = input.path
            view.sourceStyling = styling
            view.representedDocument = input.document.map(ObjectIdentifier.init)
            view.delegate = self; view.textStorage?.delegate = self; view.textLayoutManager?.delegate = self
            if changed { view.font = .systemFont(ofSize: input.fontSize) }
            view.isEditable = input.document != nil && input.document?.readOnlyReason == nil && input.document?.issue != .recovery && !loading
            view.setAccessibilityLabel(view.isEditable ? L10n.string("Note") : L10n.string("Note, read-only"))
            view.onWidthOrThemeChange = { [weak self] in self?.schedule(); self?.scheduleStyles(force: true) }
            view.onToggle = { [weak self] ordinal, id in self?.toggle(ordinal: ordinal, id: id) }
            view.onEditSource = { [weak self] ordinal in self?.editSource(ordinal) }
            view.onCompositionState = { [weak self] marked in self?.document?.compositionChanged(marked) }
            view.onCommit = { [weak self] in self?.textDidChange(Notification(name: NSText.didChangeNotification)) }
            input.document?.onReload = { [weak self] text in self?.reload(text, preserve: true) }
            input.document?.onClosing = { [weak view] in view?.isEditable = false }
            input.document?.onStateChange = { [weak self, weak view] in
                guard let self, let view else { return }
                view.isEditable = document?.readOnlyReason == nil && document?.issue != .recovery
            }
            if changed {
                styling.reset()
                generation += 1; task?.cancel(); blocks = []
                suppressSelection = true; view.expanded = []; view.install([]); suppressSelection = false
                if let document = input.document {
                    loading = true; view.isEditable = false; view.diagramPending = true
                    let token = generation
                    task = Task { [weak self] in
                        do {
                            let snapshot = try await document.io.perform { try $0.snapshot() }
                            guard let self, !Task.isCancelled, token == generation else { return }
                            loading = false
                            reload(snapshot.text, preserve: false)
                        } catch { self?.view?.diagramIssue = String(describing: error); self?.view?.diagramPending = false }
                    }
                } else { reload(input.text, preserve: false) }
            } else { schedule(); scheduleStyles() }
        }
        private func reload(_ text: String, preserve: Bool) {
            guard let view else { return }
            let selection = view.selectedRange()
            let origin = view.enclosingScrollView?.contentView.bounds.origin ?? .zero
            suppress = true; suppressSelection = true
            view.expanded = []; view.install([]); blocks = []
            if preserve { styling.reset() }
            view.string = text; view.sourceUTF16Length = view.textStorage?.length ?? 0
            view.undoManager?.removeAllActions()
            view.setSelectedRange(NSRange(location: preserve ? min(selection.location, view.sourceUTF16Length) : 0,
                                          length: preserve ? min(selection.length, max(0, view.sourceUTF16Length - selection.location)) : 0))
            view.enclosingScrollView?.contentView.scroll(to: preserve ? origin : .zero)
            suppress = false; suppressSelection = false
            view.isEditable = document != nil && document?.readOnlyReason == nil && document?.issue != .recovery
            if !preserve, let document,
               (document.sourceSelectionLength > 0 || document.sourceAnchor > 0) {
                let location = min(document.sourceAnchor, view.sourceUTF16Length)
                let length = min(document.sourceSelectionLength, max(0, view.sourceUTF16Length - location))
                view.setSelectedRange(NSRange(location: location, length: length))
                view.scrollRangeToVisible(view.selectedRange())
            }
            schedule(force: true)
            scheduleStyles(force: true)
        }
        private func scheduleStyles(force: Bool = false) {
            guard let view, let input else { return }
            styling.schedule(view: view, document: document, size: input.fontSize, force: force)
        }
        func textStorage(_ storage: NSTextStorage, didProcessEditing editedMask: NSTextStorageEditActions,
                         range editedRange: NSRange, changeInLength delta: Int) {
            guard !suppress, editedMask.contains(.editedCharacters), let view else { return }
            let replacement = storage.attributedSubstring(from: editedRange).string
            let original = NSRange(location: editedRange.location, length: editedRange.length - delta)
            document?.edited(range: original, replacement: replacement)
            view.sourceUTF16Length = storage.length
            styling.shift(for: original, delta: delta)
            view.shiftBlocks(for: original, delta: delta)
            blocks = view.blocks
            scheduleStyles()
            generation += 1; task?.cancel()
            if !view.compositionActive { view.diagramPending = true }
            // TextKit 2 的原生撤销也可能只发文本存储通知；在事务结束后统一重排。
            if !changeScheduled {
                changeScheduled = true
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    changeScheduled = false
                    textDidChange(Notification(name: NSText.didChangeNotification))
                }
            }
        }
        func textDidChange(_ notification: Notification) {
            guard !suppress, let view else { return }
            document?.compositionChanged(view.compositionActive)
            guard !view.compositionActive else { return }
            selectionChanged()
            scheduleStyles()
            schedule(force: true, debounce: true)
        }
        func textViewDidChangeSelection(_ notification: Notification) {
            guard !suppressSelection, !suppress, !loading, view?.compositionActive == false else { return }
            selectionChanged()
        }
        private func selectionChanged() {
            guard let view, !view.compositionActive else { return }
            view.updateMediaProjection()
            let selections = view.selectedRanges.map(\.rangeValue)
            let expanded = Set(view.blocks.filter { block in
                selections.contains { selection in
                    selection.length == 0 ? NSLocationInRange(selection.location, block.range)
                        : NSIntersectionRange(selection, block.range).length > 0
                }
            }.map(\.ordinal))
            guard expanded != view.expanded else { return }
            suppressSelection = true; view.expanded = expanded; view.install(view.blocks); suppressSelection = false
        }
        func mediaSelectionChanged() { selectionChanged() }

        func alignTable() {
            guard let view, let document, view.isEditable, !view.compositionActive else { return }
            let selection = view.selectedRange(), revision = document.revision
            Task { [weak self, weak view] in
                guard let patch = try? await document.io.perform({ try $0.alignTable(caret: UInt64(selection.location)) }),
                      let self, let view, document.revision == revision, view.selectedRange() == selection,
                      !view.compositionActive else { return }
                view.breakUndoCoalescing()
                view.insertText(patch.replacement, replacementRange: NSRange(location: Int(patch.start), length: Int(patch.length)))
                view.breakUndoCoalescing()
                view.undoManager?.setActionName(L10n.string("Align Table Source"))
                view.setSelectedRange(NSRange(location: min(selection.location, view.sourceUTF16Length), length: 0))
                self.selectionChanged()
            }
        }
        private func editSource(_ ordinal: UInt32) {
            guard let view, !view.compositionActive, let block = view.blocks.first(where: { $0.ordinal == ordinal }) else { return }
            view.window?.makeFirstResponder(view)
            // 跳入源码由明确的“编辑源码”动作触发；节点行仍只处理折叠。
            let line = view.textStorage?.mutableString.lineRange(for: NSRange(location: block.range.location, length: 0))
            let start = input?.path.lowercased().hasSuffix(".mmd") == true ? block.range.location : line.map(NSMaxRange) ?? block.range.location
            view.setSelectedRange(NSRange(location: min(NSMaxRange(block.range) - 1, start), length: 0))
            selectionChanged()
            view.scrollRangeToVisible(view.selectedRange())
        }
        func schedule(force: Bool = false, debounce: Bool = false) {
            guard let input, let view, !loading, !view.compositionActive else { return }
            let width = max(120, view.bounds.width - view.textContainerInset.width * 2)
            let dark = view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            guard force || abs(width - lastWidth) > 0.25 || dark != lastDark else { return }
            lastWidth = width; lastDark = dark
            generation += 1; let token = generation
            task?.cancel(); view.diagramPending = true
            let expected = document?.revision
            task = Task { [weak self] in
                guard let self else { return }
                do {
                    if debounce { try await Task.sleep(for: .milliseconds(150)) }
                    let source: String
                    var loaded: [NoteDiagram]
                    var media: [PreparedMedia] = []
                    if let document = input.document {
                        let snapshot = try await document.io.perform { try $0.snapshot() }
                        source = snapshot.text
                        let wire = try await document.io.perform { try $0.mediaBlocks(width: width, size: input.fontSize) }
                        media = await LocalImageCache.shared.prepare(wire, vault: input.vault)
                        loaded = try await input.service.blocks(snapshot: snapshot, vault: input.vault, note: input.path, core: input.core)
                    } else {
                        source = input.text
                        loaded = try await input.service.blocks(source: source, mmd: input.path.lowercased().hasSuffix(".mmd"),
                                                                 vault: input.vault, note: input.path, core: input.core)
                    }
                    for index in loaded.indices {
                        if let old = blocks.first(where: { $0.ordinal == loaded[index].ordinal }) { loaded[index].collapsed = old.collapsed }
                    }
                    let rendered = await input.service.render(source: source, mmd: input.path.lowercased().hasSuffix(".mmd"),
                                                               blocks: loaded, width: width, dark: dark)
                    guard !Task.isCancelled, generation == token, !view.compositionActive, expected == document?.revision else { return }
                    blocks = rendered
                    suppressSelection = true; view.install(rendered); suppressSelection = false
                    view.installMedia(media.map { NoteMedia(range: NSRange(location: Int($0.block.sourceStart), length: Int($0.block.sourceLength)), prepared: $0) },
                        width: width, size: input.fontSize, vault: input.vault, refresh: { [weak self] in self?.schedule(force: true) })
                    // 初次打开保持图投影；用户后续移动选择才展开。
                    if debounce { selectionChanged() }
                    view.diagramPending = false
                    (NSApp.delegate as? AppDelegate)?.inspectWhenReady()
                } catch {
                    guard !Task.isCancelled, token == generation else { return }
                    view.diagramIssue = String(describing: error); view.diagramPending = false
                }
            }
        }
        private func toggle(ordinal: UInt32, id: String) {
            guard !view!.compositionActive, let index = blocks.firstIndex(where: { $0.ordinal == ordinal }) else { return }
            if blocks[index].collapsed.contains(id) { blocks[index].collapsed.remove(id) }
            else { blocks[index].collapsed.insert(id) }
            input?.remember(ordinal, blocks[index].collapsed.sorted())
            schedule(force: true)
        }
        func textLayoutManager(_ manager: NSTextLayoutManager, textLayoutFragmentFor location: any NSTextLocation,
                               in element: NSTextElement) -> NSTextLayoutFragment {
            let fragment = NoteLayoutFragment(textElement: element, range: nil)
            if let content = manager.textContentManager {
                view?.configure(fragment, at: content.offset(from: content.documentRange.location, to: location))
            }
            return fragment
        }
    }
}

/// 每个段落始终使用同一种片段；切换显示或改变宽度时只更新几何，不重建源码段落。
final class NoteLayoutFragment: NSTextLayoutFragment {
    var extraLineHeight: CGFloat = 22
    private var extraLine: SourceEmptyLine?
    override var textLineFragments: [NSTextLineFragment] {
        let lines = super.textLineFragments
        guard let last = lines.last, last.characterRange.length == 0 else { return lines }
        // AppKit 首次把光标移到文末时，会把同高空行的上方行距移进其高度。
        // 统一这个合成行的度量，源码行和插入位置都不随选择改变。
        if extraLine?.source !== last || extraLine?.height != extraLineHeight {
            extraLine = SourceEmptyLine(source: last, height: extraLineHeight)
        }
        return Array(lines.dropLast()) + [extraLine!]
    }
    var replacement: CGSize?
    var previewHeight: CGFloat = 0
    var reason: String?
    var noticeWidth = 0.0
    var sourceMask: UInt32 = 0
    var quoteHead = true
    var quoteTail = true
    override var layoutFragmentFrame: CGRect {
        guard let replacement else {
            var frame = super.layoutFragmentFrame
            frame.size.height += previewHeight
            return frame
        }
        return CGRect(origin: super.layoutFragmentFrame.origin, size: replacement)
    }
    override var renderingSurfaceBounds: CGRect {
        if let replacement { return CGRect(origin: .zero, size: replacement) }
        var rect = super.renderingSurfaceBounds
        if reason != nil { rect.size.width = max(rect.width, noticeWidth) }
        if sourceMask & (32 | 64 | 4096) != 0 {
            // 原生表面默认只覆盖字形；块底色与左侧竖条也要纳入绘制范围。
            rect = rect.union(CGRect(x: -12, y: 0, width: noticeWidth + 17, height: layoutFragmentFrame.height))
        }
        return rect
    }
    override var topMargin: CGFloat { super.topMargin + (reason == nil ? 0 : 24) }
    override func draw(at point: CGPoint, in context: CGContext) {
        guard replacement == nil else { return }
        if sourceMask & (32 | 64 | 4096) != 0 {
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
            let rect = CGRect(x: point.x - 5, y: point.y, width: noticeWidth + 10, height: layoutFragmentFrame.height)
            if sourceMask & 32 != 0 {
                NSColor.quaternaryLabelColor.withAlphaComponent(0.07).setFill(); rect.fill()
            } else {
                NSColor.controlAccentColor.withAlphaComponent(0.22).setFill()
                CGRect(x: point.x - 12, y: point.y + (quoteHead ? 2 : 0), width: 2,
                       height: max(0, rect.height - (quoteHead ? 2 : 0) - (quoteTail ? 2 : 0))).fill()
            }
            NSGraphicsContext.restoreGraphicsState()
        }
        if sourceMask & 32768 != 0 {
            context.saveGState()
            context.setFillColor(NSColor.separatorColor.cgColor)
            context.fill(CGRect(x: point.x, y: point.y + 10, width: noticeWidth, height: 0.5))
            context.restoreGState()
        }
        super.draw(at: point, in: context)
        if let reason {
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
            (reason as NSString).draw(at: point, withAttributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.secondaryLabelColor])
            NSGraphicsContext.restoreGraphicsState()
        }
    }
}

@MainActor
final class DiagramTextView: NSTextView {
    let editorActions = EditorActions()
    var isReading = false
    weak var readingCoordinator: ReadingText.Coordinator?
    weak var editingCoordinator: NoteText.Coordinator?
    var representedPath = ""
    var representedDocument: ObjectIdentifier?
    var sourceTypingAttributes: [NSAttributedString.Key: Any]?
    weak var sourceStyling: SourceStyling?
    override var typingAttributes: [NSAttributedString.Key: Any] {
        get { compositionActive ? super.typingAttributes : sourceTypingAttributes ?? super.typingAttributes }
        set { super.typingAttributes = compositionActive ? newValue : sourceTypingAttributes ?? newValue }
    }
    var diagramStorage: DiagramContentStorage?
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        readingCoordinator?.didMoveToWindow()
    }
    /// 先迁移原生视口，再按实际片段定位；避免长文的估算高度把目标夹在可视区外。
    func revealCharacter(at position: Int) {
        setSelectedRange(NSRange(location: position, length: 0))
        guard let manager = textLayoutManager, let content = manager.textContentManager,
              let location = content.location(content.documentRange.location, offsetBy: position) else { return }
        if sourceUTF16Length < 65536 { manager.ensureLayout(for: content.documentRange) }
        else if let end = content.location(location, offsetBy: min(1, sourceUTF16Length - position)),
           let range = NSTextRange(location: location, end: end) { manager.ensureLayout(for: range) }
        scrollRangeToVisible(selectedRange())
        needsLayout = true; layoutSubtreeIfNeeded()
        scrollRangeToVisible(selectedRange())
        for _ in 0..<2 {
            guard let fragment = manager.textLayoutFragment(for: location) else { break }
            let target = fragment.layoutFragmentFrame.offsetBy(dx: textContainerInset.width, dy: textContainerInset.height)
            guard !visibleRect.intersects(target) else { break }
            _ = scrollToVisible(target)
        }
    }
    convenience init(diagramLayout: Bool) {
        let content = DiagramContentStorage()
        content.textStorage = NSTextStorage()
        let manager = NSTextLayoutManager()
        content.addTextLayoutManager(manager)
        let container = NSTextContainer(size: CGSize(width: 700, height: CGFloat.greatestFiniteMagnitude))
        manager.textContainer = container
        self.init(frame: .zero, textContainer: container)
        diagramStorage = content
        content.editor = self
        isAutomaticQuoteSubstitutionEnabled = false
        isAutomaticDashSubstitutionEnabled = false
        isAutomaticTextReplacementEnabled = false
        isAutomaticSpellingCorrectionEnabled = false
        isAutomaticTextCompletionEnabled = false
        isContinuousSpellCheckingEnabled = false
        usesFindBar = true
        usesAdaptiveColorMappingForDarkAppearance = true
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(contrastChanged), name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil)
    }
    // 显式自检参数仅模拟正文的高对比度颜色，不改系统辅助功能设置。
    var contrastForInspection = ProcessInfo.processInfo.arguments.contains("--simulate-high-contrast") {
        didSet { onWidthOrThemeChange?() }
    }
    var increasedContrast: Bool {
        let best = effectiveAppearance.bestMatch(from: [.accessibilityHighContrastAqua, .accessibilityHighContrastDarkAqua, .aqua, .darkAqua])
        return contrastForInspection || NSWorkspace.shared.accessibilityDisplayShouldIncreaseContrast
            || best == .accessibilityHighContrastAqua || best == .accessibilityHighContrastDarkAqua
    }
    @objc private func contrastChanged() { onWidthOrThemeChange?() }
    var sourceUTF16Length = 0
    var blocks: [NoteDiagram] = []
    var diagramViews: [UInt32: DiagramView] = [:]
    var mediaBlocks: [NoteMedia] = []
    var mediaViews: [MediaView] = []
    var mediaProjection: [NSRange] = []
    var mediaPaintedAt = 0.0
    var diagramPending = false
    var diagramIssue: String?
    var onWidthOrThemeChange: (() -> Void)?
    var onToggle: ((UInt32, String) -> Void)?
    var onEditSource: ((UInt32) -> Void)?
    var onCompositionState: ((Bool) -> Void)?
    var onCommit: (() -> Void)?
    var expanded: Set<UInt32> = []
    private var installedExpanded: Set<UInt32> = []
    private var settingMarked = false
    var compositionActive: Bool { settingMarked || hasMarkedText() }
    private var placing = false
    var maximumMainWorkMilliseconds = 0.0
    var installMilliseconds = 0.0
    var installPhases: [String: Double] = [:]
    var layoutPhases: [String: Double] = [:]
    func recordMainWork(since start: Double) {
        maximumMainWorkMilliseconds = max(maximumMainWorkMilliseconds, (ProcessInfo.processInfo.systemUptime - start) * 1000)
    }

    override func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        settingMarked = true
        onCompositionState?(true)
        super.setMarkedText(string, selectedRange: selectedRange, replacementRange: replacementRange)
        settingMarked = false
    }
    override func insertText(_ string: Any, replacementRange: NSRange) {
        super.insertText((string as? NSAttributedString)?.string ?? string, replacementRange: replacementRange)
        if !hasMarkedText() { onCompositionState?(false); onCommit?() }
    }
    override func unmarkText() {
        super.unmarkText()
        if !settingMarked { onCompositionState?(false); onCommit?() }
    }

    func shiftBlocks(for range: NSRange, delta: Int) {
        var keptMedia: [NoteMedia] = []
        var keptViews: [MediaView] = []
        for (index, var item) in mediaBlocks.enumerated() {
            if range.location >= NSMaxRange(item.range) { }
            else if NSMaxRange(range) <= item.range.location { item.range.location += delta }
            else { mediaViews[index].detach(); mediaViews[index].removeFromSuperview(); continue }
            keptMedia.append(item); keptViews.append(mediaViews[index])
        }
        mediaBlocks = keptMedia; mediaViews = keptViews
        mediaProjection = mediaProjection.compactMap { item in
            if range.location >= NSMaxRange(item) { return item }
            if NSMaxRange(range) <= item.location { return NSRange(location: item.location + delta, length: item.length) }
            return nil
        }
        blocks = blocks.compactMap { block in
            var block = block
            if range.location >= NSMaxRange(block.range) { return block }
            if NSMaxRange(range) <= block.range.location { block.range.location += delta; block.tailStart += delta; return block }
            if compositionActive {
                block.range.length = max(0, block.range.length + delta)
                if range.location <= block.tailStart { block.tailStart += delta }
                return block
            }
            // 受编辑影响的图立即失效；后台重扫前不显示旧图或旧错误。
            diagramViews.removeValue(forKey: block.ordinal)?.detach()
            return nil
        }
        // 组合时仅平移已有投影的范围，不改变展开状态、场景或组合区属性。
        updateProjectionRanges()
    }

    override func setFrameSize(_ newSize: NSSize) {
        let changed = abs(frame.width - newSize.width) > 0.25
        super.setFrameSize(newSize)
        if changed { onWidthOrThemeChange?() }
    }
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        onWidthOrThemeChange?()
    }
    func install(_ blocks: [NoteDiagram]) {
        let started = ProcessInfo.processInfo.systemUptime
        defer { installMilliseconds = (ProcessInfo.processInfo.systemUptime - started) * 1000; recordMainWork(since: started) }
        guard !compositionActive else { return }
        let nextProjection: [NSRange] = isReading ? [] : blocks.filter { $0.prepared != nil && !expanded.contains($0.ordinal) }.map(\.range) + projectedMediaParagraphs
        let projectionChanged = diagramStorage?.diagrams != nextProjection
        let geometryChanged = projectionChanged || installedExpanded != expanded || self.blocks.count != blocks.count || zip(self.blocks, blocks).contains { old, new in
            old.range != new.range || old.prepared?.scene.height != new.prepared?.scene.height || old.reason != new.reason
        }
        let onlyTailHeightChanged = !projectionChanged && installedExpanded == expanded && expanded.isEmpty && self.blocks.count == blocks.count
            && blocks.last.map { $0.prepared != nil && NSMaxRange($0.range) == sourceUTF16Length } == true
            && zip(self.blocks.dropLast(), blocks.dropLast()).allSatisfy { $0.prepared?.scene.height == $1.prepared?.scene.height && $0.reason == $1.reason }
        self.blocks = blocks
        installedExpanded = expanded
        diagramStorage?.diagrams = nextProjection
        let valid = Set(blocks.filter { $0.prepared != nil }.map(\.ordinal))
        for ordinal in Array(diagramViews.keys) where !valid.contains(ordinal) {
            diagramViews.removeValue(forKey: ordinal)?.detach()
        }
        for block in blocks {
            guard let prepared = block.prepared else { continue }
            if let view = diagramViews[block.ordinal] { view.update(prepared) }
            else {
                let view = DiagramView(prepared: prepared)
                view.isHidden = true
                view.onToggle = { [weak self] id in self?.onToggle?(block.ordinal, id) }
                view.onEditSource = { [weak self] in self?.onEditSource?(block.ordinal) }
                diagramViews[block.ordinal] = view
                addSubview(view)
            }
        }
        let updated = ProcessInfo.processInfo.systemUptime
        if !geometryChanged || onlyTailHeightChanged {
            if let manager = textLayoutManager, let content = manager.textContentManager {
                enumerateVisibleFragments { fragment in
                    if let fragment = fragment as? NoteLayoutFragment {
                        self.configure(fragment, at: content.offset(from: content.documentRange.location, to: fragment.rangeInElement.location))
                    }
                }
            }
            for view in diagramViews.values { view.setFrameSize(CGSize(width: view.prepared.scene.width, height: view.prepared.scene.height)) }
            resizeDocument()
            installPhases = ["update_ms": (ProcessInfo.processInfo.systemUptime - started) * 1000]
            needsDisplay = true
            return
        }
        guard let manager = textLayoutManager, let content = manager.textContentManager else { return }
        if projectionChanged, let storage = (content as? NSTextContentStorage)?.textStorage, storage.length > 0 {
            // 仅在源码/图之间切换时重建元素，宽度变化复用既有片段。
            content.performEditingTransaction {
                storage.edited(.editedAttributes, range: NSRange(location: 0, length: storage.length), changeInLength: 0)
            }
        }
        enumerateVisibleFragments { fragment in
            if let fragment = fragment as? NoteLayoutFragment {
                self.configure(fragment, at: content.offset(from: content.documentRange.location, to: fragment.rangeInElement.location))
            }
        }
        let enumerated = ProcessInfo.processInfo.systemUptime
        manager.invalidateLayout(for: content.documentRange)
        let invalidated = ProcessInfo.processInfo.systemUptime
        needsLayout = true
        needsDisplay = true
        layoutSubtreeIfNeeded()
        installPhases = ["update_ms": (updated - started) * 1000,
                         "enumerate_ms": (enumerated - updated) * 1000,
                         "invalidate_ms": (invalidated - enumerated) * 1000,
                         "layout_ms": (ProcessInfo.processInfo.systemUptime - invalidated) * 1000]
    }
    func configure(_ fragment: NoteLayoutFragment, at offset: Int) {
        fragment.replacement = nil
        fragment.previewHeight = 0
        fragment.reason = nil
        fragment.noticeWidth = bounds.width - 2 * textContainerInset.width
        let bodyFont = sourceTypingAttributes?[.font] as? NSFont ?? font ?? NSFont.systemFont(ofSize: 15)
        fragment.extraLineHeight = ceil(bodyFont.ascender - bodyFont.descender + bodyFont.leading) + 4
        fragment.sourceMask = 0
        let media = mediaIndices(at: offset)
        if let index = media.first, index < mediaViews.count {
            if mediaReplacesParagraph(mediaBlocks[index].range) {
                fragment.replacement = mediaViews[index].preferredSize
                return
            }
            fragment.previewHeight = media.reduce(0) { $0 + mediaViews[$1].preferredSize.height }
        }
        if let storage = textStorage, offset < storage.length, let styling = sourceStyling {
            fragment.sourceMask = styling.mask(at: offset)
            let line = storage.mutableString.lineRange(for: NSRange(location: offset, length: 0))
            styling.overlapping(line) { run, _ in fragment.sourceMask |= run.mask & (32 | 64 | 4096 | 32768) }
        }
        if fragment.sourceMask & (64 | 4096) != 0, let storage = textStorage, let styling = sourceStyling {
            let line = storage.mutableString.lineRange(for: NSRange(location: offset, length: 0))
            func quote(_ position: Int) -> Bool {
                guard position >= 0, position < storage.length else { return false }
                let range = storage.mutableString.lineRange(for: NSRange(location: position, length: 0))
                var yes = false
                styling.overlapping(range) { run, _ in yes = yes || run.mask & (64 | 4096) != 0 }
                return yes
            }
            fragment.quoteHead = !quote(line.location - 1)
            fragment.quoteTail = !quote(NSMaxRange(line))
        }
        guard let block = blocks.first(where: { NSLocationInRange(offset, $0.range) }) else { return }
        let head = offset == block.range.location
        if let prepared = block.prepared {
            if expanded.contains(block.ordinal) {
                if offset == block.tailStart { fragment.previewHeight = prepared.scene.height + 12 }
            } else { fragment.replacement = head ? CGSize(width: prepared.scene.width, height: prepared.scene.height) : .zero }
        } else if head { fragment.reason = block.reason }
    }
    private func resizeDocument() {
        let contentHeight: CGFloat
        if let last = blocks.last, NSMaxRange(last.range) == sourceUTF16Length, let view = diagramViews[last.ordinal] {
            // 末块没有后续正文，只需更新图与滚动尺寸，无需重新排版所有原生段落。
            contentHeight = view.frame.maxY + textContainerInset.height
        } else { contentHeight = (textLayoutManager?.usageBoundsForTextContainer.maxY ?? 0) + 2 * textContainerInset.height }
        let height = max(enclosingScrollView?.contentSize.height ?? 0, contentHeight)
        if abs(frame.height - height) > 0.5 { setFrameSize(NSSize(width: frame.width, height: height)) }
    }
    private func enumerateVisibleFragments(_ body: (NSTextLayoutFragment) -> Void) {
        guard let manager = textLayoutManager, let content = manager.textContentManager else { return }
        let viewport = manager.textViewportLayoutController.viewportRange
        let start = viewport?.location ?? content.documentRange.location
        let end = viewport?.endLocation ?? content.location(start, offsetBy: min(4096, sourceUTF16Length)) ?? content.documentRange.endLocation
        // 无 ensuresLayout 也会创建所经段落的缓存。必须在视口末尾停止，防止每键平移整篇元素缓存。
        manager.enumerateTextLayoutFragments(from: start, options: []) { fragment in
            if fragment.rangeInElement.location.compare(end) != .orderedAscending { return false }
            body(fragment)
            return true
        }
    }
    override func accessibilityChildren() -> [Any]? {
        (super.accessibilityChildren() ?? []) + diagramViews.keys.sorted().compactMap { diagramViews[$0] }
    }
    override func layout() {
        let started = ProcessInfo.processInfo.systemUptime
        defer { recordMainWork(since: started) }
        super.layout()
        let native = ProcessInfo.processInfo.systemUptime
        guard !placing, let manager = textLayoutManager, let content = manager.textContentManager else { return }
        placing = true
        defer { placing = false }
        for view in diagramViews.values { view.isHidden = true }
        for view in mediaViews { view.isHidden = true }
        let ensured = ProcessInfo.processInfo.systemUptime
        // 由原生视口控制器排版可见内容，不为放置叠加视图强制排完整篇笔记。
        enumerateVisibleFragments { fragment in
            let offset = content.offset(from: content.documentRange.location, to: fragment.rangeInElement.location)
            if let fragment = fragment as? NoteLayoutFragment { self.configure(fragment, at: offset) }
            let indices = self.mediaIndices(at: offset)
            var mediaY = fragment.layoutFragmentFrame.minY
            if let index = indices.first, !self.mediaReplacesParagraph(self.mediaBlocks[index].range) {
                mediaY = fragment.layoutFragmentFrame.maxY - indices.reduce(0) { $0 + self.mediaViews[$1].preferredSize.height }
            }
            for index in indices {
                let media = self.mediaViews[index]
                media.frame.origin = CGPoint(x: self.textContainerInset.width, y: mediaY + self.textContainerInset.height)
                media.isHidden = false; media.becameVisible()
                mediaY += media.preferredSize.height
            }
            if let block = self.blocks.first(where: { block in
                if !self.expanded.contains(block.ordinal) { return block.range.location == offset }
                return block.tailStart == offset
            }), let view = self.diagramViews[block.ordinal] {
                let preview = self.expanded.contains(block.ordinal)
                let previewHeight = (fragment as? NoteLayoutFragment)?.previewHeight ?? 0
                let y = preview ? fragment.layoutFragmentFrame.maxY - previewHeight + 12 : fragment.layoutFragmentFrame.minY
                view.frame = CGRect(x: self.textContainerInset.width,
                                    y: y + self.textContainerInset.height,
                                    width: view.prepared.scene.width, height: view.prepared.scene.height)
                view.isHidden = false
            }
        }
        let enumerated = ProcessInfo.processInfo.systemUptime
        resizeDocument()
        recordMediaDisplay()
        layoutPhases = ["native_ms": (native - started) * 1000, "ensure_ms": (ensured - native) * 1000,
                        "enumerate_ms": (enumerated - ensured) * 1000,
                        "size_ms": (ProcessInfo.processInfo.systemUptime - enumerated) * 1000]
    }
}
