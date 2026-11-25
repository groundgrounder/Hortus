import CoreText
import Foundation
import HortusApple
import HortusCore
import SwiftUI
import UIKit

final class IOSLayoutFragment: NSTextLayoutFragment {
    var sourceMask: UInt32 = 0
    var noticeWidth: CGFloat = 0

    override var renderingSurfaceBounds: CGRect {
        var rect = super.renderingSurfaceBounds
        if sourceMask & (32 | 64 | 4096) != 0 {
            rect = rect.union(CGRect(x: -12, y: 0, width: max(rect.width, noticeWidth) + 18, height: layoutFragmentFrame.height))
        }
        return rect
    }

    override func draw(at point: CGPoint, in context: CGContext) {
        if sourceMask & (32 | 64 | 4096) != 0 {
            context.saveGState()
            if sourceMask & 32 != 0 {
                let rect = CGRect(x: point.x - 6, y: point.y, width: noticeWidth + 12, height: layoutFragmentFrame.height)
                context.setFillColor(PlatformGraphics.sRGBCGColor(UIColor.quaternaryLabel.withAlphaComponent(0.08)))
                context.fill(rect)
            } else {
                context.setFillColor(PlatformGraphics.sRGBCGColor(UIColor.tintColor.withAlphaComponent(0.28)))
                context.fill(CGRect(x: point.x - 10, y: point.y + 2, width: 2.5, height: max(0, layoutFragmentFrame.height - 4)))
            }
            context.restoreGState()
        }
        if sourceMask & 32768 != 0 {
            context.saveGState()
            context.setFillColor(PlatformGraphics.sRGBCGColor(UIColor.separator))
            context.fill(CGRect(x: point.x, y: point.y + 10, width: noticeWidth, height: 0.5))
            context.restoreGState()
        }
        super.draw(at: point, in: context)
    }
}

@MainActor
final class ReadingLayoutDelegate: NSObject, @preconcurrency NSTextLayoutManagerDelegate {
    weak var view: UITextView?
    nonisolated(unsafe) weak var baseDelegate: (any NSTextLayoutManagerDelegate)?
    var runs: [SourceStyleRun] = []
    var displayedNS: NSString = ""

    override func responds(to aSelector: Selector!) -> Bool {
        if super.responds(to: aSelector) { return true }
        return (baseDelegate as AnyObject?)?.responds(to: aSelector) ?? false
    }

    override func forwardingTarget(for aSelector: Selector!) -> Any? {
        if !super.responds(to: aSelector),
           let base = baseDelegate as AnyObject?,
           base.responds(to: aSelector) {
            return base
        }
        return super.forwardingTarget(for: aSelector)
    }

    func maskAt(position: Int) -> UInt32 {
        var low = 0, high = runs.count
        while low < high {
            let mid = (low + high) / 2
            if runs[mid].start + runs[mid].length <= UInt64(position) { low = mid + 1 } else { high = mid }
        }
        guard low < runs.count, runs[low].start <= UInt64(position) else { return 0 }
        return runs[low].mask
    }

    func textLayoutManager(_ textLayoutManager: NSTextLayoutManager,
                           textLayoutFragmentFor location: any NSTextLocation,
                           in textElement: NSTextElement) -> NSTextLayoutFragment {
        let fragment = IOSLayoutFragment(textElement: textElement, range: nil)
        if let content = textLayoutManager.textContentManager, let view {
            let offset = content.offset(from: content.documentRange.location, to: location)
            fragment.noticeWidth = max(200, view.bounds.width - view.textContainerInset.left - view.textContainerInset.right)
            var mask: UInt32 = maskAt(position: offset)
            let ns = displayedNS
            if offset < ns.length {
                let line = ns.lineRange(for: NSRange(location: offset, length: 0))
                for run in runs where run.start < UInt64(NSMaxRange(line)) && run.start + run.length > UInt64(line.location) {
                    mask |= run.mask & (32 | 64 | 4096 | 32768)
                }
            }
            fragment.sourceMask = mask
        }
        return fragment
    }
}

final class HortusReadingUITextView: UITextView {
    var mathCopyItems: [(range: NSRange, source: String)] = []
    var onTapAtOffset: ((Int) -> Bool)?
    var onAttachmentLayoutChanged: (() -> Void)?
    var onAppearanceChanged: ((Bool) -> Void)?
    var lastRevealRange: NSRange?
    private var mountedAttachmentViews: [ObjectIdentifier: UIView] = [:]
    private var isSyncingAttachments = false
    private var attachmentsDirty = true
    private var lastSyncedWidth: CGFloat = -1
    private var lastSyncedInsets: UIEdgeInsets = .zero
    private(set) var attachmentSyncCount = 0

    override var attributedText: NSAttributedString! {
        didSet {
            attachmentsDirty = true
        }
    }

    convenience init() {
        self.init(frame: .zero, textContainer: nil)
    }

    override init(frame: CGRect, textContainer: NSTextContainer?) {
        super.init(frame: frame, textContainer: textContainer)
        let tap = UITapGestureRecognizer(target: self, action: #selector(handleTap(_:)))
        tap.cancelsTouchesInView = false
        addGestureRecognizer(tap)
        panGestureRecognizer.addTarget(self, action: #selector(handleUserScroll))
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: Self, _) in
            self.onAppearanceChanged?(self.traitCollection.userInterfaceStyle == .dark)
        }
    }

    required init?(coder: NSCoder) { nil }

    @objc private func handleUserScroll() {
        if panGestureRecognizer.state == .began {
            lastRevealRange = nil
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        syncAttachmentViews()
    }

    func invalidateAttachmentLayout(for attachment: HortusBaseAttachment) {
        guard let tlm = textLayoutManager, let content = tlm.textContentManager,
              let attr = attributedText, attr.length > 0 else { return }
        var targetRange: NSRange?
        attr.enumerateAttribute(.attachment, in: NSRange(location: 0, length: attr.length)) { value, range, stop in
            if (value as AnyObject) === attachment {
                targetRange = range
                stop.pointee = true
            }
        }
        if let targetRange,
           let start = content.location(content.documentRange.location, offsetBy: targetRange.location),
           let end = content.location(start, offsetBy: targetRange.length),
           let textRange = NSTextRange(location: start, end: end) {
            tlm.invalidateLayout(for: textRange)
        } else {
            tlm.invalidateLayout(for: content.documentRange)
        }
        attachmentsDirty = true
        invalidateIntrinsicContentSize()
        setNeedsLayout()
        layoutIfNeeded()
        syncAttachmentViews()
        if let range = lastRevealRange {
            scrollRangeToVisible(range)
        }
        onAttachmentLayoutChanged?()
    }

    func measureContentHeight(forWidth width: CGFloat) -> CGFloat {
        bounds.size.width = width
        textContainer.size = CGSize(
            width: max(1, width - textContainerInset.left - textContainerInset.right),
            height: .greatestFiniteMagnitude
        )
        if let tlm = textLayoutManager, let content = tlm.textContentManager {
            tlm.ensureLayout(for: content.documentRange)
            var maxY: CGFloat = 0
            tlm.enumerateTextLayoutFragments(from: content.documentRange.endLocation, options: [.reverse, .ensuresLayout]) { frag in
                maxY = frag.layoutFragmentFrame.maxY
                return false
            }
            if maxY > 0 {
                return ceil(maxY + textContainerInset.top + textContainerInset.bottom)
            }
        }
        return ceil(sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height)
    }

    func syncAttachmentViews() {
        guard !isSyncingAttachments, window != nil, bounds.width > 0,
              let tlm = textLayoutManager, let content = tlm.textContentManager,
              let attr = attributedText, attr.length > 0 else { return }
        let needsSync = attachmentsDirty
            || abs(bounds.width - lastSyncedWidth) > 0.5
            || textContainerInset != lastSyncedInsets
        guard needsSync else { return }
        isSyncingAttachments = true
        defer { isSyncingAttachments = false }

        tlm.ensureLayout(for: content.documentRange)
        let docStart = content.documentRange.location
        var pending: [(key: ObjectIdentifier, host: UIView, frame: CGRect)] = []

        attr.enumerateAttribute(.attachment, in: NSRange(location: 0, length: attr.length)) { value, range, _ in
            guard let att = value as? HortusBaseAttachment,
                  let loc = content.location(docStart, offsetBy: range.location),
                  let frag = tlm.textLayoutFragment(for: loc) else { return }

            let key = ObjectIdentifier(att)
            let host = mountedAttachmentViews[key] ?? att.makeHostView()
            let fragOrigin = frag.layoutFragmentFrame.origin

            var attFrame = frag.frameForTextAttachment(at: loc)
            if attFrame.isEmpty {
                let fragStart = content.offset(from: docStart, to: frag.rangeInElement.location)
                let localInFrag = max(0, range.location - fragStart)
                for line in frag.textLineFragments {
                    let lineRange = line.characterRange
                    if localInFrag >= lineRange.location && localInFrag < NSMaxRange(lineRange) {
                        let charPt = line.locationForCharacter(at: localInFrag)
                        let size = att.bounds.size
                        let x = line.typographicBounds.origin.x + charPt.x
                        let y: CGFloat
                        if let inline = att as? InlineMathAttachment {
                            y = charPt.y - CGFloat(inline.ascent)
                        } else {
                            y = line.typographicBounds.origin.y
                        }
                        attFrame = CGRect(x: x, y: y, width: size.width, height: size.height)
                        break
                    }
                }
            }
            guard !attFrame.isEmpty else { return }
            let frame = CGRect(
                x: textContainerInset.left + fragOrigin.x + attFrame.origin.x,
                y: textContainerInset.top + fragOrigin.y + attFrame.origin.y,
                width: attFrame.width,
                height: attFrame.height
            )
            pending.append((key, host, frame))
        }

        var active: [ObjectIdentifier: UIView] = [:]
        for item in pending {
            active[item.key] = item.host
            if item.host.superview !== self {
                addSubview(item.host)
            }
            if item.host.frame != item.frame {
                item.host.frame = item.frame
            }
        }
        for (key, oldView) in mountedAttachmentViews where active[key] == nil {
            oldView.removeFromSuperview()
        }
        mountedAttachmentViews = active
        attachmentsDirty = false
        lastSyncedWidth = bounds.width
        lastSyncedInsets = textContainerInset
        attachmentSyncCount += 1
    }

    func selectedPlainTextWithLatex() -> String? {
        let range = selectedRange
        let ns = (attributedText?.string ?? text ?? "") as NSString
        guard range.length > 0, NSMaxRange(range) <= ns.length else { return nil }
        let mutable = NSMutableString(string: ns.substring(with: range))
        let intersecting = mathCopyItems
            .filter { NSIntersectionRange($0.range, range).length > 0 }
            .sorted { $0.range.location > $1.range.location }
        for item in intersecting {
            let local = NSRange(location: item.range.location - range.location, length: item.range.length)
            if local.location >= 0, NSMaxRange(local) <= mutable.length {
                mutable.replaceCharacters(in: local, with: item.source)
            }
        }
        return mutable as String
    }

    override func copy(_ sender: Any?) {
        if let plain = selectedPlainTextWithLatex() {
            UIPasteboard.general.string = plain
            return
        }
        super.copy(sender)
    }

    @objc private func handleTap(_ recognizer: UITapGestureRecognizer) {
        guard recognizer.state == .ended else { return }
        let point = recognizer.location(in: self)
        let containerPoint = CGPoint(x: point.x - textContainerInset.left, y: point.y - textContainerInset.top)
        if let manager = textLayoutManager, let content = manager.textContentManager {
            let nav = manager.textLayoutFragment(for: containerPoint)
            if let fragment = nav {
                for line in fragment.textLineFragments {
                    let linePoint = CGPoint(x: containerPoint.x - fragment.layoutFragmentFrame.origin.x,
                                            y: containerPoint.y - fragment.layoutFragmentFrame.origin.y)
                    if line.typographicBounds.contains(linePoint) {
                        let index = line.characterIndex(for: linePoint)
                        if index != NSNotFound {
                            let fragmentStart = content.offset(from: content.documentRange.location, to: fragment.rangeInElement.location)
                            let charOffset = fragmentStart + index
                            if onTapAtOffset?(charOffset) == true { return }
                        }
                    }
                }
            }
        }
    }
}

struct IOSReadingTextView: UIViewRepresentable {
    @Environment(\.colorScheme) private var colorScheme
    let document: IOSNoteDocument
    let path: String
    let vault: String
    let fontSize: Double
    let diagramService: DiagramService
    let core: WorkspaceCore?
    let vaultHandle: VaultHandle?
    let knowledgeVersion: Int
    let refreshToken: Int
    let onOpenLink: (String, String?) -> Void
    let onShowTag: (String) -> Void
    let onFocus: (UInt32) -> Void
    let onToggleDiagram: (UInt32, [String]) -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> HortusReadingUITextView {
        let view = HortusReadingUITextView()
        view.isEditable = false
        view.isSelectable = true
        view.backgroundColor = .clear
        view.alwaysBounceVertical = true
        view.textContainerInset = UIEdgeInsets(top: 16, left: 18, bottom: 28, right: 18)
        view.textContainer.lineFragmentPadding = 0
        context.coordinator.layoutDelegate.view = view
        context.coordinator.layoutDelegate.baseDelegate = view.textLayoutManager?.delegate
        view.textLayoutManager?.delegate = context.coordinator.layoutDelegate
        view.accessibilityIdentifier = "ios-reading-text"
        context.coordinator.view = view
        let coordinator = context.coordinator
        view.onAppearanceChanged = { [weak coordinator] dark in
            coordinator?.handleAppearanceChange(dark: dark)
        }
        return view
    }

    func updateUIView(_ view: HortusReadingUITextView, context: Context) {
        context.coordinator.update(self, dark: colorScheme == .dark)
    }

    @MainActor
    final class Coordinator: NSObject {
        weak var view: HortusReadingUITextView?
        let layoutDelegate = ReadingLayoutDelegate()
        private var input: IOSReadingTextView?
        private var model: ReadingViewModel?
        private var ranges: [NSRange] = []
        private var runs: [SourceStyleRun] = []
        private var source = ""
        private var displayedNS: NSString = ""
        private var knowledge: KnowledgeMarkupInfo?
        private var renderedDiagrams: [NoteDiagram] = []
        private var preparedMath: [PreparedMathBlock] = []
        private var lastWidth: Double = 0
        private var lastDark: Bool = false
        private var lastSize: Double = 0
        private var lastRevision: UInt64 = .max
        private var lastKnowledgeVersion = -1
        private var lastRefreshToken = -1
        private var refreshTask: Task<Void, Never>?
        private var generation = 0

        func handleAppearanceChange(dark: Bool) {
            guard lastDark != dark, let view else { return }
            let boundsWidth = Double(view.bounds.width)
            let contentWidth = max(200.0, (boundsWidth > 40 ? boundsWidth : 360.0) - Double(view.textContainerInset.left + view.textContainerInset.right))
            refresh(width: contentWidth, dark: dark)
        }

        func update(_ next: IOSReadingTextView, dark resolvedDark: Bool? = nil) {
            let docChanged = input?.document !== next.document
            if docChanged {
                model = nil
                ranges = []
                runs = []
                renderedDiagrams = []
                preparedMath = []
                lastRevision = .max
                view?.lastRevealRange = nil
            }
            self.input = next
            guard let view else { return }

            view.onTapAtOffset = { [weak self] offset in
                self?.handleTap(at: offset) ?? false
            }

            let dark = resolvedDark ?? (view.traitCollection.userInterfaceStyle == .dark)
            let boundsWidth = Double(view.bounds.width)
            let contentWidth = max(200.0, (boundsWidth > 40 ? boundsWidth : 360.0) - Double(view.textContainerInset.left + view.textContainerInset.right))

            let structuralChange = docChanged || lastRevision != next.document.revision || abs(lastWidth - contentWidth) > 1.0
                || lastDark != dark || lastSize != next.fontSize
                || lastKnowledgeVersion != next.knowledgeVersion
            if structuralChange {
                refresh(width: contentWidth, dark: dark)
            } else if lastRefreshToken != next.refreshToken {
                if let pending = next.document.pendingReveal {
                    lastRefreshToken = next.refreshToken
                    next.document.pendingReveal = nil
                    reveal(source: pending.source, length: pending.length)
                } else {
                    refresh(width: contentWidth, dark: dark)
                }
            } else if let pending = next.document.pendingReveal {
                next.document.pendingReveal = nil
                reveal(source: pending.source, length: pending.length)
            }
        }

        func refresh(width: Double, dark: Bool) {
            guard let input, view != nil else { return }
            refreshTask?.cancel()
            generation &+= 1
            let token = generation
            let doc = input.document
            let io = doc.io
            let size = input.fontSize
            let notePath = input.path
            let vault = input.vault
            let handle = input.vaultHandle
            let core = input.core
            let service = input.diagramService
            let oldMath = lastSize == size ? preparedMath : []
            let kv = input.knowledgeVersion
            let rt = input.refreshToken

            refreshTask = Task { [weak self] in
                do {
                    let payload = try await io.perform { session in
                        let wire = try session.readingModelSince(since: UInt64.max)
                        let model = try ReadingViewModel(wire)
                        let math = try session.mathBlocks()
                        let snapshot = try session.snapshot()
                        let media = try session.mediaBlocks(width: width, size: size)
                        let knowledge = handle?.knowledgeMarkup(path: notePath, text: snapshot.text)
                        return (model, math, snapshot, media, knowledge)
                    }
                    guard !Task.isCancelled, let self, self.generation == token else { return }

                    var diagrams: [NoteDiagram] = []
                    if !payload.2.blocks.isEmpty {
                        diagrams = try await service.blocks(snapshot: payload.2, vault: vault, note: notePath,
                                                            core: core, documentIdentity: doc.diagramIdentity)
                        diagrams = await service.render(source: payload.2.text,
                                                        mmd: notePath.lowercased().hasSuffix(".mmd"),
                                                        blocks: diagrams, width: width, dark: dark,
                                                        requestGeneration: UInt64(token), fontGeneration: size.bitPattern)
                    }
                    let preparedMedia = await LocalImageCache.shared.prepare(payload.3, vault: vault, size: size,
                                                                             mathService: MathService.shared)
                    let mathBlocks = await MathService.shared.prepare(payload.1, source: payload.2.text, size: size,
                                                                      width: width, displayText: payload.0.text, previous: oldMath)
                    guard !Task.isCancelled, self.generation == token else { return }

                    self.install(model: payload.0, sourceText: payload.2.text, diagrams: diagrams,
                                 math: mathBlocks, media: preparedMedia, knowledge: payload.4,
                                 width: width, dark: dark, size: size, knowledgeVersion: kv, refreshToken: rt)
                } catch {
                    if !(error is CancellationError) {
                        NSLog("[Hortus] Reading refresh failed for %@: %@", notePath, String(describing: error))
                    }
                }
            }
        }

        private func install(model next: ReadingViewModel, sourceText: String, diagrams: [NoteDiagram],
                             math: [PreparedMathBlock], media: [PreparedMedia], knowledge: KnowledgeMarkupInfo?,
                             width: Double, dark: Bool, size: Double, knowledgeVersion: Int, refreshToken: Int) {
            guard let view, let input else { return }
            self.lastWidth = width
            self.lastDark = dark
            self.lastSize = size
            self.lastRevision = next.revision
            self.lastKnowledgeVersion = knowledgeVersion
            self.lastRefreshToken = refreshToken
            self.source = sourceText
            self.knowledge = knowledge
            self.renderedDiagrams = diagrams
            self.preparedMath = math
            input.document.adoptReadingText(sourceText, revision: next.revision)

            let sourceNS = sourceText as NSString
            let hasProperties = next.text.hasPrefix("\u{e000}")
            var fallback = [Bool](repeating: false, count: next.blocks.count)
            var ordinal: UInt32 = 0
            for (index, block) in next.blocks.enumerated() where block.diagram {
                ordinal += 1
                if diagrams.first(where: { $0.ordinal == ordinal })?.prepared == nil {
                    fallback[index] = true
                }
            }

            var mutableDisplayText: NSMutableString?
            var blockLengths = [Int](repeating: 0, count: next.blocks.count)
            var labelDeltas = [Int](repeating: 0, count: next.blocks.count)
            var displayShift = 0
            for (index, block) in next.blocks.enumerated() {
                let originalLength = Int(block.displayLength)
                let displayStart = Int(block.displayStart) + displayShift
                if fallback[index] {
                    let srcRange = NSRange(location: Int(block.sourceStart), length: Int(block.sourceLength))
                    let replacement = NSMaxRange(srcRange) <= sourceNS.length ? sourceNS.substring(with: srcRange) : ""
                    let mutable = mutableDisplayText ?? NSMutableString(string: next.text)
                    mutableDisplayText = mutable
                    mutable.replaceCharacters(in: NSRange(location: displayStart, length: originalLength), with: replacement)
                    blockLengths[index] = (replacement as NSString).length
                } else if hasProperties && block.displayStart == 0 {
                    let label = L10n.string("Properties")
                    labelDeltas[index] = (label as NSString).length - 1
                    let mutable = mutableDisplayText ?? NSMutableString(string: next.text)
                    mutableDisplayText = mutable
                    mutable.replaceCharacters(in: NSRange(location: displayStart, length: 1), with: label)
                    blockLengths[index] = originalLength + labelDeltas[index]
                } else {
                    blockLengths[index] = originalLength
                }
                displayShift += blockLengths[index] - originalLength
            }

            let unresolved = (knowledge?.links ?? []).compactMap { link -> Range<Int>? in
                guard !link.inDiagram, !link.attachment, link.resolvedPath == nil else { return nil }
                return Int(link.startUtf16)..<(Int(link.startUtf16) + Int(link.lengthUtf16))
            }

            var nextRanges: [NSRange] = []
            var nextRuns: [SourceStyleRun] = []
            var diagramByLocation: [Int: NoteDiagram] = [:]
            var offset = 0
            ordinal = 0
            for (index, block) in next.blocks.enumerated() {
                var diagram: NoteDiagram?
                if block.diagram {
                    ordinal += 1
                    diagram = diagrams.first { $0.ordinal == ordinal }
                }
                let length = blockLengths[index]
                let range = NSRange(location: offset, length: length)
                nextRanges.append(range)
                if let diagram, !fallback[index], diagram.prepared != nil {
                    diagramByLocation[offset] = diagram
                }
                if fallback[index] {
                    nextRuns.append(SourceStyleRun(start: UInt64(offset), length: UInt64(length), mask: 32))
                } else {
                    for runIndex in block.runRange {
                        let run = next.run(at: runIndex)
                        var start = Int(run.start)
                        var runLength = Int(run.length)
                        if labelDeltas[index] != 0 {
                            if start == 0 && run.mask == 16384 { runLength += labelDeltas[index] }
                            else if start >= 1 { start += labelDeltas[index] }
                        }
                        let srcStart = Int(block.sourceStart + run.sourceStart)
                        var mask = run.mask
                        if mask & (1 << 18) != 0, unresolved.contains(where: { $0.contains(srcStart) }) {
                            mask |= 1 << 19
                        }
                        nextRuns.append(SourceStyleRun(start: UInt64(offset + start), length: UInt64(runLength), mask: mask))
                    }
                }
                offset += length
            }

            let displayed = (mutableDisplayText as String?) ?? next.text
            let attributed = NSMutableAttributedString(
                string: displayed,
                attributes: SourceStylePalette.attributes(for: 0, fontSize: size, contrast: false)
            )
            for run in nextRuns {
                let range = NSRange(location: Int(run.start), length: Int(run.length))
                if NSMaxRange(range) <= attributed.length {
                    attributed.setAttributes(SourceStylePalette.attributes(for: run.mask, fontSize: size, contrast: false), range: range)
                }
            }

            let blockIndices = Dictionary(uniqueKeysWithValues: next.blocks.enumerated().map { ($0.element.id, $0.offset) })
            var copyItems: [(range: NSRange, source: String)] = []

            for item in math {
                guard let index = blockIndices[item.block.blockId], index < nextRanges.count else { continue }
                let local = Int(item.block.displayStart - next.blocks[index].displayStart)
                let range = NSRange(location: nextRanges[index].location + local, length: Int(item.block.displayLength))
                guard NSMaxRange(range) <= attributed.length else { continue }
                let noteMath = NoteMath(range: range, prepared: item, offset: local)
                copyItems.append((range: range, source: item.block.displaySource))
                if item.block.block {
                    let attachment = BlockMathAttachment(noteMath, width: width, size: size)
                    attributed.addAttribute(.attachment, value: attachment, range: range)
                } else {
                    let color = (attributed.attribute(.foregroundColor, at: range.location, effectiveRange: nil) as? UIColor) ?? .label
                    let attachment = InlineMathAttachment(noteMath, width: width, size: size, foreground: color)
                    attributed.addAttribute(.attachment, value: attachment, range: range)
                }
            }

            for (location, diagram) in diagramByLocation {
                let range = NSRange(location: location, length: 1)
                guard NSMaxRange(range) <= attributed.length else { continue }
                let attachment = DiagramBlockAttachment(
                    diagram, width: width,
                    onFocus: { [weak self] ord in self?.input?.onFocus(ord) },
                    onToggle: { [weak self] ord, nodeId in
                        guard let self, let idx = self.renderedDiagrams.firstIndex(where: { $0.ordinal == ord }) else { return }
                        var set = self.renderedDiagrams[idx].collapsed
                        if set.contains(nodeId) { set.remove(nodeId) } else { set.insert(nodeId) }
                        self.input?.onToggleDiagram(ord, set.sorted())
                    },
                    onOpenLink: { [weak self] target, heading in self?.input?.onOpenLink(target, heading) }
                )
                attributed.addAttribute(.attachment, value: attachment, range: range)
            }

            let embeds = (knowledge?.links ?? []).filter {
                $0.embed && !$0.attachment && !$0.inDiagram
            }
            for item in media {
                guard let index = blockIndices[item.block.blockId], index < nextRanges.count else { continue }
                let block = next.blocks[index]
                let local = Int(item.block.displayStart - block.displayStart)
                let range = NSRange(location: nextRanges[index].location + local, length: Int(item.block.displayLength))
                guard NSMaxRange(range) <= attributed.length else { continue }
                let noteMedia = NoteMediaItem(range: range, prepared: item)
                var embedTarget = item.block.imageTarget.isEmpty ? item.block.imageAlt : item.block.imageTarget
                var embedHeading: String?
                var embedAlias: String? = item.block.imageAlt.isEmpty ? nil : item.block.imageAlt
                if item.block.imageWiki, item.reason.hasPrefix("embed."),
                   let offset = Self.utf16Offset(byte: Int(item.block.sourceStart), in: sourceText),
                   let link = embeds.first(where: {
                       UInt64(offset) >= $0.startUtf16 && UInt64(offset) < $0.startUtf16 + $0.lengthUtf16
                   }) {
                    embedTarget = link.target
                    embedHeading = link.heading
                    embedAlias = link.alias
                }
                let attachment = MediaBlockAttachment(
                    noteMedia,
                    width: width,
                    fontSize: size,
                    vault: input.vault,
                    notePath: input.path,
                    dark: dark,
                    handle: input.vaultHandle,
                    diagramService: input.diagramService,
                    embedTarget: embedTarget,
                    embedHeading: embedHeading,
                    embedAlias: embedAlias,
                    ancestors: [input.path],
                    depth: 0,
                    onOpenLink: input.onOpenLink,
                    onShowTag: input.onShowTag,
                    onSizeChange: { [weak view] att in
                        view?.invalidateAttachmentLayout(for: att)
                    }
                )
                // MediaBlock 对应的 `\u{fffc}` 可能在 `displayStart`（表格）或 `displayStart + 1`（`\n\u{fffc}\n`）
                let slice = (displayed as NSString).substring(with: range)
                if let fffcOffset = slice.utf16.firstIndex(of: 0xfffc).map({ slice.utf16.distance(from: slice.utf16.startIndex, to: $0) }) {
                    let attachRange = NSRange(location: range.location + fffcOffset, length: 1)
                    attributed.addAttribute(.attachment, value: attachment, range: attachRange)
                }
            }

            self.model = next
            self.ranges = nextRanges
            self.runs = nextRuns
            self.displayedNS = displayed as NSString
            layoutDelegate.runs = nextRuns
            layoutDelegate.displayedNS = self.displayedNS
            view.mathCopyItems = copyItems
            view.attributedText = attributed
            view.setNeedsLayout()

            if let pending = input.document.pendingReveal {
                input.document.pendingReveal = nil
                reveal(source: pending.source, length: pending.length)
            }
        }

        private static func utf16Offset(byte: Int, in text: String) -> Int? {
            guard byte >= 0, byte <= text.utf8.count else { return nil }
            let utf8 = text.utf8
            guard let end = utf8.index(utf8.startIndex, offsetBy: byte, limitedBy: utf8.endIndex) else { return nil }
            return String(decoding: utf8[utf8.startIndex..<end], as: UTF8.self).utf16.count
        }

        private func reveal(source: Int, length: Int) {
            guard let view, let model, !ranges.isEmpty else { return }
            let target = displayPosition(source: source, model: model)
            let totalLength = displayedNS.length
            let clamped = min(max(0, target), totalLength)
            let range = NSRange(location: clamped, length: min(max(0, length), max(0, totalLength - clamped)))
            view.lastRevealRange = range
            view.scrollRangeToVisible(range)
        }

        private func displayPosition(source: Int, model: ReadingViewModel) -> Int {
            let sourceLength = self.source.utf16.count
            guard source >= 0, source <= sourceLength else { return 0 }
            if source == sourceLength { return displayedNS.length }
            guard let index = model.blocks.firstIndex(where: { source < Int($0.sourceStart + $0.sourceLength) }),
                  index < ranges.count else {
                return displayedNS.length
            }
            let block = model.blocks[index]
            let local = max(0, source - Int(block.sourceStart))
            let run = model.firstRun(in: block.runRange, matching: { local < Int($0.sourceStart + $0.sourceLength) })
            let adj = run.map { $0.length == $0.sourceLength ? max(0, local - Int($0.sourceStart)) : 0 } ?? 0
            return ranges[index].location + Int(run?.start ?? 0) + adj
        }

        private func sourcePosition(at position: Int) -> Int? {
            guard let model, !ranges.isEmpty,
                  position >= 0, position <= displayedNS.length else { return nil }
            if let index = ranges.firstIndex(where: { NSLocationInRange(position, $0) }) {
                let block = model.blocks[index], local = position - ranges[index].location
                guard let run = model.firstRun(in: block.runRange, matching: {
                    local >= Int($0.start) && local < Int($0.start + $0.length)
                }) else { return Int(block.sourceStart) }
                return Int(block.sourceStart + run.sourceStart) + (run.sourceLength == run.length ? local - Int(run.start) : 0)
            }
            return nil
        }

        private func handleTap(at position: Int) -> Bool {
            guard let input, let model, let sourcePos = sourcePosition(at: position) else { return false }
            let mask = maskAt(position: position)
            if mask & (1 << 18) != 0, let knowledge,
               let link = knowledge.links.first(where: {
                   !$0.inDiagram && !$0.attachment && UInt64(sourcePos) >= $0.startUtf16
                       && UInt64(sourcePos) < $0.startUtf16 + $0.lengthUtf16
               }) {
                input.onOpenLink(link.target, link.heading)
                return true
            }
            if mask & (1 << 18) != 0 {
                // 外部链接（如 https://commonmark.org/）
                let ns = source as NSString
                let line = ns.lineRange(for: NSRange(location: min(sourcePos, ns.length), length: 0))
                let lineText = ns.substring(with: line)
                if let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue) {
                    let matches = detector.matches(in: lineText, range: NSRange(location: 0, length: (lineText as NSString).length))
                    for m in matches {
                        let global = NSRange(location: line.location + m.range.location, length: m.range.length)
                        if NSLocationInRange(sourcePos, global), let url = m.url {
                            UIApplication.shared.open(url)
                            return true
                        }
                    }
                }
            }
            if mask & 512 != 0, let knowledge,
               let tag = knowledge.tags.first(where: {
                   UInt64(sourcePos) >= $0.startUtf16 && UInt64(sourcePos) < $0.startUtf16 + $0.lengthUtf16
               }) {
                input.onShowTag(tag.tag)
                return true
            }
            if input.document.readOnlyReason == nil, !input.document.dirty,
               let i = ranges.firstIndex(where: { NSLocationInRange(position, $0) }),
               let run = model.firstRun(in: model.blocks[i].runRange, matching: {
                   $0.mask & 131072 != 0 && position - ranges[i].location >= Int($0.start)
                       && position - ranges[i].location < Int($0.start + $0.length)
               }) {
                let srcOffset = Int(model.blocks[i].sourceStart + run.sourceStart) + 1
                let srcNS = source as NSString
                if srcOffset >= 0, srcOffset < srcNS.length {
                    let ch = srcNS.substring(with: NSRange(location: srcOffset, length: 1))
                    input.document.edited(range: NSRange(location: srcOffset, length: 1), replacement: ch == " " ? "x" : " ")
                    Task { await input.document.saveNow() }
                    return true
                }
            }
            return false
        }

        private func maskAt(position: Int) -> UInt32 {
            layoutDelegate.maskAt(position: position)
        }
    }
}
