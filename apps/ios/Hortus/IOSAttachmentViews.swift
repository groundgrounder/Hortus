import CoreGraphics
import CoreText
import Foundation
import HortusApple
import HortusCore
import UIKit

final class NoteMediaItem {
    let range: NSRange
    let prepared: PreparedMedia
    init(range: NSRange, prepared: PreparedMedia) {
        self.range = range
        self.prepared = prepared
    }
}

class HortusBaseAttachment: NSTextAttachment {
    private static let emptyPlaceholder = UIImage()

    init() {
        super.init(data: nil, ofType: nil)
        allowsTextAttachmentView = true
    }

    required init?(coder: NSCoder) { nil }

    @MainActor
    func makeHostView() -> UIView { UIView() }

    override func image(for bounds: CGRect, attributes: [NSAttributedString.Key: Any],
                        location: any NSTextLocation, textContainer: NSTextContainer?) -> UIImage? {
        Self.emptyPlaceholder
    }
}

// MARK: - Inline Math Attachment

final class InlineMathAttachment: HortusBaseAttachment {
    let item: NoteMath
    let availableWidth: Double
    let bodySize: Double
    let foreground: UIColor
    let naturalSize: CGSize
    let ascent: Double
    let descent: Double
    let layoutBounds: CGRect
    var isSelected = false

    init(_ item: NoteMath, width: Double, size: Double, foreground: UIColor, bodyAscent: Double? = nil) {
        self.item = item
        self.availableWidth = max(1, width)
        self.bodySize = size
        self.foreground = foreground
        if let math = item.prepared.math {
            naturalSize = math.size
            ascent = math.ascent
            descent = math.descent
        } else {
            var up: CGFloat = 0, down: CGFloat = 0
            let lineWidth = item.prepared.fallbackLines.first.map { CTLineGetTypographicBounds($0, &up, &down, nil) } ?? 0
            naturalSize = CGSize(width: lineWidth, height: up + down)
            ascent = Double(up)
            descent = Double(down)
        }
        let baseAscent = bodyAscent ?? Double(UIFont.systemFont(ofSize: MathTypography.bodySize(size, mask: item.prepared.block.mask)).ascender)
        let layoutAscent = naturalSize.width <= availableWidth && ascent <= baseAscent + 4 ? min(ascent, baseAscent) : ascent
        layoutBounds = CGRect(x: 0, y: -descent, width: min(naturalSize.width, availableWidth), height: layoutAscent + descent)
        super.init()
        bounds = CGRect(x: 0, y: -descent, width: min(naturalSize.width, availableWidth), height: naturalSize.height)
    }

    required init?(coder: NSCoder) { nil }

    @MainActor
    override func makeHostView() -> UIView {
        InlineMathHostUIView(attachment: self)
    }

    override func attachmentBounds(for attributes: [NSAttributedString.Key: Any], location: any NSTextLocation,
                                   textContainer: NSTextContainer?, proposedLineFragment: CGRect, position: CGPoint) -> CGRect {
        layoutBounds
    }

    override func viewProvider(for parentView: UIView?, location: any NSTextLocation,
                               textContainer: NSTextContainer?) -> NSTextAttachmentViewProvider? {
        InlineMathViewProvider(textAttachment: self, parentView: parentView,
                               textLayoutManager: textContainer?.textLayoutManager, location: location)
    }
}

private extension NSTextAttachmentViewProvider {
    func installMainActorView<Attachment: NSTextAttachment>(
        as _: Attachment.Type,
        build: @MainActor (Attachment) -> UIView
    ) {
        nonisolated(unsafe) let provider = self
        MainActor.assumeIsolated {
            if let attachment = provider.textAttachment as? Attachment {
                provider.view = build(attachment)
            } else {
                provider.view = UIView()
            }
        }
    }
}

private final class InlineMathViewProvider: NSTextAttachmentViewProvider {
    override func loadView() {
        installMainActorView(as: InlineMathAttachment.self) { InlineMathHostUIView(attachment: $0) }
    }
}

private final class InlineMathHostUIView: UIView {
    let attachment: InlineMathAttachment
    init(attachment: InlineMathAttachment) {
        self.attachment = attachment
        super.init(frame: CGRect(origin: .zero, size: attachment.bounds.size))
        isOpaque = false
        backgroundColor = .clear
        let canvas = InlineMathCanvasUIView(attachment: attachment)
        if attachment.naturalSize.width > attachment.availableWidth {
            let scroll = UIScrollView(frame: bounds)
            scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            scroll.showsVerticalScrollIndicator = false
            scroll.showsHorizontalScrollIndicator = true
            scroll.contentSize = attachment.naturalSize
            canvas.frame = CGRect(origin: .zero, size: attachment.naturalSize)
            scroll.addSubview(canvas)
            addSubview(scroll)
        } else {
            canvas.frame = CGRect(origin: .zero, size: attachment.naturalSize)
            addSubview(canvas)
        }
        isAccessibilityElement = true
        accessibilityLabel = L10n.string("Formula: %@", attachment.item.prepared.block.latex)
    }
    required init?(coder: NSCoder) { nil }
}

private final class InlineMathCanvasUIView: UIView {
    let attachment: InlineMathAttachment
    init(attachment: InlineMathAttachment) {
        self.attachment = attachment
        super.init(frame: CGRect(origin: .zero, size: attachment.naturalSize))
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
        registerForTraitChanges([UITraitUserInterfaceStyle.self, UITraitDisplayScale.self]) { (self: Self, _) in
            self.setNeedsDisplay()
        }
    }
    required init?(coder: NSCoder) { nil }

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext() else { return }
        if attachment.item.prepared.math == nil {
            UIColor.quaternaryLabel.withAlphaComponent(0.10).setFill()
            UIRectFill(bounds)
        }
        let resolved = attachment.foreground.resolvedColor(with: traitCollection)
        let foreground = PlatformGraphics.sRGBCGColor(resolved)
        if let math = attachment.item.prepared.math {
            MathRenderer.draw(math, in: context, origin: .zero, foreground: foreground,
                              deviceScale: Double(traitCollection.displayScale > 0 ? traitCollection.displayScale : 2))
        } else if let line = attachment.item.prepared.fallbackLines.first {
            context.saveGState()
            context.setFillColor(foreground)
            context.translateBy(x: 0, y: attachment.ascent)
            context.scaleBy(x: 1, y: -1)
            context.textMatrix = .identity
            context.textPosition = .zero
            CTLineDraw(line, context)
            context.restoreGState()
        }
    }
}

// MARK: - Block Math Attachment

final class BlockMathAttachment: HortusBaseAttachment {
    let item: NoteMath
    let width: Double
    let bodySize: Double
    let preferredSize: CGSize

    init(_ item: NoteMath, width: Double, size: Double) {
        self.item = item
        self.width = max(80, width)
        self.bodySize = size
        let contentHeight = item.prepared.math.map { Double($0.size.height) } ?? item.prepared.fallbackHeight
        self.preferredSize = CGSize(width: self.width, height: contentHeight + 2 * MathTypography.padding)
        super.init()
        bounds = CGRect(origin: .zero, size: preferredSize)
    }

    required init?(coder: NSCoder) { nil }

    @MainActor
    override func makeHostView() -> UIView {
        BlockMathHostUIView(attachment: self)
    }

    override func attachmentBounds(for attributes: [NSAttributedString.Key: Any], location: any NSTextLocation,
                                   textContainer: NSTextContainer?, proposedLineFragment: CGRect, position: CGPoint) -> CGRect {
        CGRect(origin: .zero, size: preferredSize)
    }

    override func viewProvider(for parentView: UIView?, location: any NSTextLocation,
                               textContainer: NSTextContainer?) -> NSTextAttachmentViewProvider? {
        BlockMathViewProvider(textAttachment: self, parentView: parentView,
                              textLayoutManager: textContainer?.textLayoutManager, location: location)
    }
}

private final class BlockMathViewProvider: NSTextAttachmentViewProvider {
    override func loadView() {
        installMainActorView(as: BlockMathAttachment.self) { BlockMathHostUIView(attachment: $0) }
    }
}

private final class BlockMathHostUIView: UIView, UIContextMenuInteractionDelegate {
    let attachment: BlockMathAttachment
    init(attachment: BlockMathAttachment) {
        self.attachment = attachment
        super.init(frame: CGRect(origin: .zero, size: attachment.preferredSize))
        isOpaque = false
        backgroundColor = .clear

        let naturalWidth = attachment.item.prepared.math.map { Double($0.size.width) } ?? attachment.width
        let canvasWidth = max(attachment.width, naturalWidth)
        let canvas = BlockMathCanvasUIView(prepared: attachment.item.prepared, bodySize: attachment.bodySize,
                                           frame: CGRect(x: 0, y: 0, width: canvasWidth, height: attachment.preferredSize.height))
        if naturalWidth > attachment.width {
            let scroll = UIScrollView(frame: bounds)
            scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            scroll.showsVerticalScrollIndicator = false
            scroll.showsHorizontalScrollIndicator = true
            scroll.contentSize = canvas.frame.size
            scroll.addSubview(canvas)
            addSubview(scroll)
        } else {
            canvas.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            addSubview(canvas)
        }
        addInteraction(UIContextMenuInteraction(delegate: self))
        isAccessibilityElement = true
        accessibilityLabel = L10n.string("Formula: %@", attachment.item.prepared.block.latex)
    }

    required init?(coder: NSCoder) { nil }

    func contextMenuInteraction(_ interaction: UIContextMenuInteraction,
                                configurationForMenuAtLocation location: CGPoint) -> UIContextMenuConfiguration? {
        UIContextMenuConfiguration(identifier: nil, previewProvider: nil) { [weak self] _ in
            guard let self else { return nil }
            let copy = UIAction(title: L10n.string("Copy Markdown Source"), image: UIImage(systemName: "doc.on.doc")) { _ in
                UIPasteboard.general.string = self.attachment.item.prepared.source
            }
            return UIMenu(children: [copy])
        }
    }
}

private final class BlockMathCanvasUIView: UIView {
    let prepared: PreparedMathBlock
    let bodySize: Double
    init(prepared: PreparedMathBlock, bodySize: Double, frame: CGRect) {
        self.prepared = prepared
        self.bodySize = bodySize
        super.init(frame: frame)
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
        registerForTraitChanges([UITraitUserInterfaceStyle.self, UITraitDisplayScale.self]) { (self: Self, _) in
            self.setNeedsDisplay()
        }
    }
    required init?(coder: NSCoder) { nil }

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext() else { return }
        let foreground = PlatformGraphics.sRGBCGColor(UIColor.label.resolvedColor(with: traitCollection))
        if let math = prepared.math {
            let originX = max(0, (bounds.width - math.size.width) / 2)
            MathRenderer.draw(math, in: context, origin: CGPoint(x: originX, y: MathTypography.padding),
                              foreground: foreground, deviceScale: Double(traitCollection.displayScale > 0 ? traitCollection.displayScale : 2))
        } else {
            UIColor.quaternaryLabel.withAlphaComponent(0.07).resolvedColor(with: traitCollection).setFill()
            UIRectFill(bounds.insetBy(dx: 0, dy: MathTypography.padding))
            var lineY = 8.0
            if !prepared.reasonLines.isEmpty {
                context.setFillColor(PlatformGraphics.sRGBCGColor(UIColor.secondaryLabel.resolvedColor(with: traitCollection)))
                for line in prepared.reasonLines {
                    context.saveGState()
                    context.translateBy(x: 8, y: lineY + 14)
                    context.scaleBy(x: 1, y: -1)
                    context.textMatrix = .identity
                    context.textPosition = .zero
                    CTLineDraw(line, context)
                    context.restoreGState()
                    lineY += 16
                }
            }
            context.setFillColor(foreground)
            for (index, line) in prepared.fallbackLines.enumerated() {
                context.saveGState()
                context.translateBy(x: 8, y: lineY + bodySize + Double(index) * (bodySize + 5))
                context.scaleBy(x: 1, y: -1)
                context.textMatrix = .identity
                context.textPosition = .zero
                CTLineDraw(line, context)
                context.restoreGState()
            }
        }
    }
}

// MARK: - Inline Diagram Attachment

final class DiagramBlockAttachment: HortusBaseAttachment {
    let diagram: NoteDiagram
    let width: Double
    let showFocusButton: Bool
    let onFocus: (UInt32) -> Void
    let onToggle: (UInt32, String) -> Void
    let onOpenLink: (String, String?) -> Void

    init(_ diagram: NoteDiagram, width: Double, showFocusButton: Bool = true,
         onFocus: @escaping (UInt32) -> Void,
         onToggle: @escaping (UInt32, String) -> Void, onOpenLink: @escaping (String, String?) -> Void) {
        let clampedWidth = max(120, width)
        self.diagram = diagram
        self.width = clampedWidth
        self.showFocusButton = showFocusButton
        self.onFocus = onFocus
        self.onToggle = onToggle
        self.onOpenLink = onOpenLink
        let size = diagram.prepared.map { CGSize(width: min(clampedWidth, $0.scene.width), height: $0.scene.height) }
            ?? CGSize(width: clampedWidth, height: 48)
        super.init()
        bounds = CGRect(origin: .zero, size: size)
    }

    required init?(coder: NSCoder) { nil }

    @MainActor
    override func makeHostView() -> UIView {
        InlineDiagramUIView(attachment: self)
    }

    override func attachmentBounds(for attributes: [NSAttributedString.Key: Any], location: any NSTextLocation,
                                   textContainer: NSTextContainer?, proposedLineFragment: CGRect, position: CGPoint) -> CGRect {
        bounds
    }

    override func viewProvider(for parentView: UIView?, location: any NSTextLocation,
                               textContainer: NSTextContainer?) -> NSTextAttachmentViewProvider? {
        DiagramBlockViewProvider(textAttachment: self, parentView: parentView,
                                 textLayoutManager: textContainer?.textLayoutManager, location: location)
    }
}

private final class DiagramBlockViewProvider: NSTextAttachmentViewProvider {
    override func loadView() {
        installMainActorView(as: DiagramBlockAttachment.self) { InlineDiagramUIView(attachment: $0) }
    }
}

private final class InlineDiagramUIView: UIView {
    let attachment: DiagramBlockAttachment
    private let focusButton = UIButton(type: .system)

    init(attachment: DiagramBlockAttachment) {
        self.attachment = attachment
        super.init(frame: CGRect(origin: .zero, size: attachment.bounds.size))
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
        clipsToBounds = true

        if attachment.showFocusButton {
            var config = UIButton.Configuration.tinted()
            config.title = L10n.string("Focus")
            config.buttonSize = .mini
            config.cornerStyle = .capsule
            focusButton.configuration = config
            focusButton.addAction(UIAction { [weak self] _ in
                guard let self else { return }
                self.attachment.onFocus(self.attachment.diagram.ordinal)
            }, for: .touchUpInside)
            focusButton.translatesAutoresizingMaskIntoConstraints = false
            addSubview(focusButton)
            NSLayoutConstraint.activate([
                focusButton.topAnchor.constraint(equalTo: topAnchor, constant: 6),
                focusButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            ])
        }

        let tap = UITapGestureRecognizer(target: self, action: #selector(handleTap(_:)))
        addGestureRecognizer(tap)
        isAccessibilityElement = true
        accessibilityLabel = L10n.string("Mermaid diagram")
        accessibilityIdentifier = "inline-diagram-\(attachment.diagram.ordinal)"
    }

    required init?(coder: NSCoder) { nil }

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext(), let prepared = attachment.diagram.prepared else { return }
        SceneRenderer.draw(prepared, in: context)
    }

    @objc private func handleTap(_ recognizer: UITapGestureRecognizer) {
        let point = recognizer.location(in: self)
        guard let prepared = attachment.diagram.prepared else {
            attachment.onFocus(attachment.diagram.ordinal)
            return
        }
        if let hit = prepared.scene.hitRegions.last(where: { $0.rect.cgRect.contains(point) }) {
            if let target = hit.linkTarget, !target.isEmpty {
                attachment.onOpenLink(target, hit.linkHeading)
                return
            }
            if hit.canCollapse {
                attachment.onToggle(attachment.diagram.ordinal, hit.nodeId)
                return
            }
        }
        attachment.onFocus(attachment.diagram.ordinal)
    }
}

// MARK: - Media / Table / Embed Attachment

final class MediaBlockAttachment: HortusBaseAttachment {
    let media: NoteMediaItem
    let width: Double
    let fontSize: Double
    let vault: String
    let notePath: String
    let dark: Bool
    let handle: VaultHandle?
    let diagramService: DiagramService
    let embedTarget: String
    let embedHeading: String?
    let embedAlias: String?
    let ancestors: [String]
    let depth: Int
    let onOpenLink: (String, String?) -> Void
    let onShowTag: (String) -> Void
    var onSizeChange: ((MediaBlockAttachment) -> Void)?
    private(set) var preferredSize: CGSize

    init(_ media: NoteMediaItem, width: Double, fontSize: Double, vault: String, notePath: String,
         dark: Bool, handle: VaultHandle?, diagramService: DiagramService,
         embedTarget: String? = nil, embedHeading: String? = nil, embedAlias: String? = nil,
         ancestors: [String] = [], depth: Int = 0,
         onOpenLink: @escaping (String, String?) -> Void,
         onShowTag: @escaping (String) -> Void = { _ in },
         onSizeChange: ((MediaBlockAttachment) -> Void)? = nil) {
        self.media = media
        self.width = max(120, width)
        self.fontSize = fontSize
        self.vault = vault
        self.notePath = notePath
        self.dark = dark
        self.handle = handle
        self.diagramService = diagramService
        let fallbackTarget = media.prepared.block.imageTarget.isEmpty ? media.prepared.block.imageAlt : media.prepared.block.imageTarget
        self.embedTarget = embedTarget ?? fallbackTarget
        self.embedHeading = embedHeading
        self.embedAlias = embedAlias
        self.ancestors = ancestors.isEmpty ? [notePath] : ancestors
        self.depth = depth
        self.onOpenLink = onOpenLink
        self.onShowTag = onShowTag
        self.onSizeChange = onSizeChange

        let block = media.prepared.block
        if !block.columns.isEmpty {
            let tableHeight = TableLayoutMetrics.measure(block: block, size: fontSize, math: media.prepared.math).totalHeight
            self.preferredSize = CGSize(width: self.width, height: max(44, tableHeight + 8))
        } else if media.prepared.block.imageWiki && media.prepared.reason.hasPrefix("embed.") {
            self.preferredSize = CGSize(width: self.width, height: 68)
        } else if media.prepared.imageSize.width > 0 && media.prepared.imageSize.height > 0 {
            let requestedWidth = block.imageWidth.flatMap { $0 > 0 ? Double($0) : nil } ?? Double(media.prepared.imageSize.width)
            let maxW = min(self.width, requestedWidth)
            let h = maxW * Double(media.prepared.imageSize.height) / Double(media.prepared.imageSize.width)
            self.preferredSize = CGSize(width: self.width, height: max(24, h))
        } else {
            self.preferredSize = CGSize(width: self.width, height: 46)
        }
        super.init()
        bounds = CGRect(origin: .zero, size: preferredSize)
    }

    required init?(coder: NSCoder) { nil }

    @MainActor
    func updatePreferredHeight(_ height: Double) {
        let clamped = max(44.0, height)
        guard abs(preferredSize.height - clamped) > 0.5 else { return }
        preferredSize = CGSize(width: width, height: clamped)
        bounds = CGRect(origin: .zero, size: preferredSize)
        onSizeChange?(self)
    }

    @MainActor
    override func makeHostView() -> UIView {
        MediaBlockHostUIView(attachment: self)
    }

    override func attachmentBounds(for attributes: [NSAttributedString.Key: Any], location: any NSTextLocation,
                                   textContainer: NSTextContainer?, proposedLineFragment: CGRect, position: CGPoint) -> CGRect {
        bounds
    }

    override func viewProvider(for parentView: UIView?, location: any NSTextLocation,
                               textContainer: NSTextContainer?) -> NSTextAttachmentViewProvider? {
        MediaBlockViewProvider(textAttachment: self, parentView: parentView,
                               textLayoutManager: textContainer?.textLayoutManager, location: location)
    }
}

private final class MediaBlockViewProvider: NSTextAttachmentViewProvider {
    override func loadView() {
        installMainActorView(as: MediaBlockAttachment.self) { MediaBlockHostUIView(attachment: $0) }
    }
}

enum TableLayoutMetrics {
    struct Measured {
        let texts: [NSAttributedString]
        let heights: [Double]
        var totalHeight: Double { heights.reduce(0, +) }
        var totalWidth: Double { columns.reduce(0, +) }
        let columns: [Double]
    }

    static func measure(block: MediaBlock, size: Double, math: [[PreparedMathBlock]]) -> Measured {
        let texts = block.cells.enumerated().map { index, cell -> NSAttributedString in
            let header: UInt32 = index < block.columns.count ? 2 : 0
            let text = NSMutableAttributedString(string: cell.text, attributes: SourceStylePalette.attributes(for: header, fontSize: size, contrast: false))
            for run in cell.runs {
                let range = NSRange(location: Int(run.start), length: Int(run.length))
                if NSMaxRange(range) <= text.length {
                    text.setAttributes(SourceStylePalette.attributes(for: run.mask | header, fontSize: size, contrast: false), range: range)
                }
            }
            if index < math.count {
                for formula in math[index] {
                    let range = NSRange(location: Int(formula.block.displayStart), length: Int(formula.block.displayLength))
                    guard NSMaxRange(range) <= text.length else { continue }
                    let color = (text.attribute(.foregroundColor, at: range.location, effectiveRange: nil) as? UIColor) ?? .label
                    let attachment = InlineMathAttachment(NoteMath(range: range, prepared: formula, offset: range.location),
                                                          width: max(40, block.columns[index % block.columns.count] - 20),
                                                          size: size, foreground: color)
                    text.addAttribute(.attachment, value: attachment, range: range)
                }
            }
            let paragraph = NSMutableParagraphStyle()
            paragraph.lineSpacing = 4
            let alignment = block.alignments[index % block.columns.count]
            paragraph.alignment = alignment == 1 ? .center : alignment == 2 ? .right : .left
            paragraph.lineBreakMode = .byWordWrapping
            text.addAttribute(.paragraphStyle, value: paragraph, range: NSRange(location: 0, length: text.length))
            return text
        }
        let heights: [Double] = (0..<Int(block.rows)).map { row in
            (0..<block.columns.count).map { col in
                let attr = texts[row * block.columns.count + col]
                return Double(ceil(attr.boundingRect(with: CGSize(width: max(1, block.columns[col] - 20), height: .greatestFiniteMagnitude),
                                                     options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil).height)) + 16.0
            }.max() ?? 36.0
        }
        return Measured(texts: texts, heights: heights, columns: block.columns)
    }
}

final class TableCanvasUIView: UIView {
    private struct CellLayout {
        let storage: NSTextStorage
        let layoutManager: NSLayoutManager
        let textContainer: NSTextContainer
        let attachments: [(range: NSRange, attachment: InlineMathAttachment)]
    }

    let measured: TableLayoutMetrics.Measured
    private var cellLayouts: [Int: CellLayout] = [:]

    init(measured: TableLayoutMetrics.Measured) {
        self.measured = measured
        var layouts: [Int: CellLayout] = [:]
        if !measured.columns.isEmpty {
            for (index, attr) in measured.texts.enumerated() {
                var attachments: [(range: NSRange, attachment: InlineMathAttachment)] = []
                attr.enumerateAttribute(.attachment, in: NSRange(location: 0, length: attr.length)) { value, range, _ in
                    if let inlineMath = value as? InlineMathAttachment {
                        attachments.append((range: range, attachment: inlineMath))
                    }
                }
                if !attachments.isEmpty {
                    let storage = NSTextStorage(attributedString: attr)
                    let layoutManager = NSLayoutManager()
                    let colWidth = measured.columns[index % measured.columns.count]
                    let container = NSTextContainer(size: CGSize(width: max(1, colWidth - 20), height: .greatestFiniteMagnitude))
                    container.lineFragmentPadding = 0
                    layoutManager.addTextContainer(container)
                    storage.addLayoutManager(layoutManager)
                    layouts[index] = CellLayout(storage: storage, layoutManager: layoutManager, textContainer: container, attachments: attachments)
                }
            }
        }
        self.cellLayouts = layouts
        super.init(frame: CGRect(x: 0, y: 0, width: measured.totalWidth, height: measured.totalHeight))
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
        registerForTraitChanges([UITraitUserInterfaceStyle.self, UITraitDisplayScale.self]) { (self: Self, _) in
            self.setNeedsDisplay()
        }
    }
    required init?(coder: NSCoder) { nil }

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext() else { return }
        let deviceScale = Double(traitCollection.displayScale > 0 ? traitCollection.displayScale : 2)
        var y: CGFloat = 0
        UIColor.separator.resolvedColor(with: traitCollection).setStroke()
        let lines = UIBezierPath()
        lines.lineWidth = 0.5
        for (row, height) in measured.heights.enumerated() {
            var x: CGFloat = 0
            for (col, width) in measured.columns.enumerated() {
                let cellIndex = row * measured.columns.count + col
                let cellRect = CGRect(x: x + 10, y: y + 8, width: width - 20, height: height - 8)
                if let cellLayout = cellLayouts[cellIndex] {
                    cellLayout.layoutManager.ensureLayout(for: cellLayout.textContainer)
                    let glyphRange = cellLayout.layoutManager.glyphRange(for: cellLayout.textContainer)
                    cellLayout.layoutManager.drawBackground(forGlyphRange: glyphRange, at: cellRect.origin)
                    cellLayout.layoutManager.drawGlyphs(forGlyphRange: glyphRange, at: cellRect.origin)
                    for item in cellLayout.attachments {
                        let charGlyphs = cellLayout.layoutManager.glyphRange(forCharacterRange: item.range, actualCharacterRange: nil)
                        guard charGlyphs.location != NSNotFound, charGlyphs.length > 0 else { continue }
                        let lineRect = cellLayout.layoutManager.lineFragmentRect(forGlyphAt: charGlyphs.location, effectiveRange: nil)
                        let glyphLocation = cellLayout.layoutManager.location(forGlyphAt: charGlyphs.location)
                        let boxOrigin = CGPoint(
                            x: cellRect.origin.x + lineRect.origin.x + glyphLocation.x,
                            y: cellRect.origin.y + lineRect.origin.y + glyphLocation.y - CGFloat(item.attachment.ascent)
                        )
                        let resolved = item.attachment.foreground.resolvedColor(with: traitCollection)
                        let foreground = PlatformGraphics.sRGBCGColor(resolved)
                        context.saveGState()
                        context.clip(to: cellRect)
                        if let math = item.attachment.item.prepared.math {
                            MathRenderer.draw(math, in: context, origin: boxOrigin, foreground: foreground, deviceScale: deviceScale)
                        } else if let line = item.attachment.item.prepared.fallbackLines.first {
                            UIColor.quaternaryLabel.withAlphaComponent(0.10).resolvedColor(with: traitCollection).setFill()
                            UIRectFill(CGRect(origin: boxOrigin, size: item.attachment.bounds.size))
                            context.setFillColor(foreground)
                            context.translateBy(x: boxOrigin.x, y: boxOrigin.y + CGFloat(item.attachment.ascent))
                            context.scaleBy(x: 1, y: -1)
                            context.textMatrix = .identity
                            context.textPosition = .zero
                            CTLineDraw(line, context)
                        }
                        context.restoreGState()
                    }
                } else {
                    let text = measured.texts[cellIndex]
                    text.draw(with: cellRect, options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil)
                }
                x += width
            }
            y += height
            lines.move(to: CGPoint(x: 0, y: y))
            lines.addLine(to: CGPoint(x: bounds.width, y: y))
        }
        lines.stroke()
    }
}

final class MediaBlockHostUIView: UIView {
    private static let titleAreaHeight: CGFloat = 34
    private static let maxEmbeddedNoteHeight: CGFloat = 1400

    private let attachment: MediaBlockAttachment
    private var imageView: UIImageView?
    private let accentBar = UIView()
    private var embedMessageLabel: UILabel?
    private var embedTitleButton: UIButton?
    private var embedBodyView: UIView?
    private var embedTask: Task<Void, Never>?

    private(set) var embedKind: String?
    private(set) var resolvedEmbedPath: String?
    private(set) var resolvedEmbedHeading: String?

    init(attachment: MediaBlockAttachment) {
        self.attachment = attachment
        super.init(frame: CGRect(origin: .zero, size: attachment.preferredSize))
        isOpaque = false
        backgroundColor = .clear
        clipsToBounds = true

        let block = attachment.media.prepared.block
        if !block.columns.isEmpty {
            let measured = TableLayoutMetrics.measure(block: block, size: attachment.fontSize, math: attachment.media.prepared.math)
            let canvas = TableCanvasUIView(measured: measured)
            let scroll = UIScrollView(frame: bounds)
            scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            scroll.showsVerticalScrollIndicator = false
            scroll.showsHorizontalScrollIndicator = true
            scroll.contentSize = canvas.frame.size
            scroll.addSubview(canvas)
            addSubview(scroll)
        } else if block.imageWiki && attachment.media.prepared.reason.hasPrefix("embed.") {
            setupEmbedCard()
        } else if attachment.media.prepared.reason.isEmpty && attachment.media.prepared.imageSize.width > 0 {
            let iv = UIImageView(frame: bounds)
            iv.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            iv.contentMode = .scaleAspectFit
            addSubview(iv)
            self.imageView = iv
            let path = block.imagePath
            let vault = attachment.vault
            Task.detached(priority: .userInitiated) { [weak iv] in
                let cgImage = await LocalImageCache.shared.decode(path, vault: vault)
                await MainActor.run {
                    if let cgImage { iv?.image = UIImage(cgImage: cgImage) }
                }
            }
        } else {
            setupNotice(text: L10n.string(attachment.media.prepared.reason.isEmpty ? "image.decode_failed" : attachment.media.prepared.reason))
        }
    }

    required init?(coder: NSCoder) { nil }

    deinit {
        embedTask?.cancel()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard let titleButton = embedTitleButton else { return }
        let w = bounds.width
        let h = bounds.height
        accentBar.frame = CGRect(x: 2, y: 6, width: 3, height: max(0, h - 12))
        titleButton.frame = CGRect(x: 12, y: 6, width: max(0, w - 24), height: 24)
        let bodyY = Self.titleAreaHeight
        let bodyW = max(0, w - 16)
        let bodyH = max(0, h - bodyY - 8)
        if let body = embedBodyView {
            body.frame = CGRect(x: 8, y: bodyY, width: bodyW, height: bodyH)
        }
        if let msg = embedMessageLabel {
            msg.frame = CGRect(x: 14, y: bodyY, width: max(0, w - 28), height: max(20, bodyH))
        }
    }

    private func setupNotice(text: String) {
        backgroundColor = UIColor.secondarySystemBackground
        layer.cornerRadius = 8
        let label = UILabel()
        label.text = text
        label.font = .systemFont(ofSize: 13)
        label.textColor = .secondaryLabel
        label.numberOfLines = 2
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    private func setupEmbedCard() {
        backgroundColor = UIColor.secondarySystemBackground
        layer.cornerRadius = 10
        layer.borderWidth = 0.5
        layer.borderColor = UIColor.separator.resolvedColor(with: traitCollection).cgColor
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: Self, _) in
            self.layer.borderColor = UIColor.separator.resolvedColor(with: self.traitCollection).cgColor
        }

        accentBar.backgroundColor = tintColor.withAlphaComponent(0.42)
        accentBar.layer.cornerRadius = 1.5
        addSubview(accentBar)

        let button = UIButton(type: .system)
        button.contentHorizontalAlignment = .leading
        var config = UIButton.Configuration.plain()
        config.title = initialEmbedTitle()
        config.image = UIImage(systemName: attachment.embedTarget.lowercased().hasSuffix(".mmd") ? "chart.xyaxis.line" : "doc.text")?
            .withConfiguration(UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold))
        config.imagePadding = 6
        config.contentInsets = .zero
        config.titleLineBreakMode = .byTruncatingTail
        config.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { incoming in
            var outgoing = incoming
            outgoing.font = UIFont.systemFont(ofSize: 13, weight: .semibold)
            return outgoing
        }
        button.configuration = config
        button.isEnabled = false
        button.accessibilityIdentifier = "embed-title-\(attachment.embedTarget)"
        button.addAction(UIAction { [weak self] _ in
            guard let self, let path = self.resolvedEmbedPath else { return }
            self.attachment.onOpenLink(path, self.resolvedEmbedHeading)
        }, for: .touchUpInside)
        addSubview(button)
        self.embedTitleButton = button

        let msg = UILabel()
        msg.font = .systemFont(ofSize: 13)
        msg.textColor = .secondaryLabel
        msg.numberOfLines = 0
        msg.text = L10n.string("Loading notes")
        addSubview(msg)
        self.embedMessageLabel = msg

        guard let handle = attachment.handle else {
            showEmbedMessage(L10n.string("Embedded note could not be read"))
            return
        }

        let sourcePath = attachment.notePath
        let target = attachment.embedTarget
        let heading = attachment.embedHeading
        let alias = attachment.embedAlias
        let ancestors = attachment.ancestors
        let vault = attachment.vault
        let fontSize = attachment.fontSize
        let dark = attachment.dark
        let cardWidth = attachment.width
        let noteContentWidth = max(120.0, cardWidth - 44.0)
        let diagramWidth = max(120.0, cardWidth - 16.0)
        let diagramService = attachment.diagramService

        embedTask = Task.detached(priority: .userInitiated) { [weak self] in
            let info = try? handle.resolveEmbed(
                sourcePath: sourcePath,
                rawTarget: target,
                heading: heading,
                alias: alias,
                ancestors: ancestors,
                maxDepth: 3,
                width: noteContentWidth,
                size: fontSize
            )
            guard !Task.isCancelled else { return }
            guard let info else {
                await MainActor.run {
                    self?.showEmbedMessage(L10n.string("Embedded note could not be read"))
                }
                return
            }

            if info.kind == "diagram" {
                let block = NoteDiagram(
                    ordinal: 1,
                    sourceStart: 0,
                    range: NSRange(location: 0, length: (info.content as NSString).length),
                    collapsed: [],
                    tailStart: 0,
                    documentIdentity: "embed:" + (info.resolvedPath ?? target),
                    revision: 1
                )
                let rendered = await diagramService.render(
                    source: info.content,
                    mmd: info.isMmd,
                    blocks: [block],
                    width: diagramWidth,
                    dark: dark,
                    fontGeneration: fontSize.bitPattern
                )
                guard !Task.isCancelled else { return }
                await MainActor.run {
                    self?.applyResolvedEmbed(info: info, media: [], math: [], diagrams: rendered)
                }
            } else if info.kind == "note" {
                let media = await LocalImageCache.shared.prepare(
                    info.media, vault: vault, size: fontSize, mathService: MathService.shared
                )
                guard !Task.isCancelled else { return }
                let math: [PreparedMathBlock] = info.math.isEmpty ? [] : await MathService.shared.prepare(
                    info.math, source: info.content, size: fontSize, width: noteContentWidth, displayText: info.text
                )
                guard !Task.isCancelled else { return }
                var diagrams: [NoteDiagram] = []
                if !info.diagrams.isEmpty {
                    let blocks = info.diagrams.map { item in
                        NoteDiagram(
                            ordinal: item.ordinal,
                            sourceStart: item.startUtf16,
                            range: NSRange(location: Int(item.startUtf16), length: Int(item.lengthUtf16)),
                            collapsed: [],
                            tailStart: 0,
                            documentIdentity: "embed:" + (info.resolvedPath ?? target),
                            revision: 1
                        )
                    }
                    diagrams = await diagramService.render(
                        source: info.content,
                        mmd: info.isMmd,
                        blocks: blocks,
                        width: noteContentWidth,
                        dark: dark,
                        fontGeneration: fontSize.bitPattern
                    )
                }
                guard !Task.isCancelled else { return }
                await MainActor.run {
                    self?.applyResolvedEmbed(info: info, media: media, math: math, diagrams: diagrams)
                }
            } else {
                await MainActor.run {
                    self?.applyResolvedEmbed(info: info, media: [], math: [], diagrams: [])
                }
            }
        }
    }

    private func initialEmbedTitle() -> String {
        let base = attachment.embedAlias?.isEmpty == false ? (attachment.embedAlias ?? attachment.embedTarget) : attachment.embedTarget
        if let heading = attachment.embedHeading, !heading.isEmpty {
            return "\(base) #\(heading)"
        }
        return base
    }

    private func formattedEmbedTitle(for info: EmbedContentInfo) -> String {
        let base = info.displayTitle.isEmpty ? attachment.embedTarget : info.displayTitle
        if let heading = info.heading, !heading.isEmpty {
            return "\(base) #\(heading)"
        }
        return base
    }

    private func applyResolvedEmbed(info: EmbedContentInfo, media: [PreparedMedia], math: [PreparedMathBlock], diagrams: [NoteDiagram]) {
        embedKind = info.kind
        resolvedEmbedPath = info.resolvedPath
        resolvedEmbedHeading = info.heading

        if var config = embedTitleButton?.configuration {
            config.title = formattedEmbedTitle(for: info)
            config.image = UIImage(systemName: info.isMmd ? "chart.xyaxis.line" : "doc.text")?
                .withConfiguration(UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold))
            embedTitleButton?.configuration = config
        }
        embedTitleButton?.isEnabled = info.resolvedPath != nil

        switch info.kind {
        case "diagram":
            if let diagram = diagrams.first, diagram.prepared != nil {
                let canvas = EmbedDiagramCanvasUIView(
                    info: info,
                    initialDiagram: diagram,
                    width: max(120, attachment.width - 16),
                    dark: attachment.dark,
                    fontSize: attachment.fontSize,
                    handle: attachment.handle,
                    fallbackSourcePath: attachment.notePath,
                    diagramService: attachment.diagramService,
                    onOpenLink: attachment.onOpenLink,
                    onHeightChange: { [weak self] diagramHeight in
                        self?.setEmbedBodyHeight(diagramHeight)
                    }
                )
                installEmbedBody(canvas, height: diagram.prepared?.scene.height ?? 120)
            } else {
                installEmbedFallbackSource(info.content, reason: diagrams.first?.reason ?? "")
            }

        case "note":
            let bodyWidth = max(120.0, attachment.width - 16.0)
            let reading = EmbeddedNoteReadingUIView(
                info: info,
                media: media,
                math: math,
                diagrams: diagrams,
                attachment: attachment,
                bodyWidth: bodyWidth,
                onHeightChange: { [weak self] measuredHeight in
                    let clamped = min(Self.maxEmbeddedNoteHeight, max(32.0, measuredHeight))
                    self?.updateScrollStateForNote(measuredHeight: measuredHeight, maxHeight: Self.maxEmbeddedNoteHeight)
                    self?.setEmbedBodyHeight(clamped)
                }
            )
            let measured = reading.measureHeight(forWidth: bodyWidth)
            reading.setScrollable(measured > Self.maxEmbeddedNoteHeight)
            installEmbedBody(reading, height: min(Self.maxEmbeddedNoteHeight, max(32.0, measured)))

        case "cycle":
            let name = info.displayTitle.isEmpty ? attachment.embedTarget : info.displayTitle
            showEmbedMessage(L10n.string("Circular embed: %@", name))
        case "depth_exceeded":
            let name = info.displayTitle.isEmpty ? attachment.embedTarget : info.displayTitle
            showEmbedMessage(L10n.string("Embedding is too deep: %@", name))
        case "heading_missing":
            showEmbedMessage(L10n.string("Heading not found: %@", info.heading ?? attachment.embedHeading ?? ""))
        case "ambiguous":
            showEmbedMessage(L10n.string("Several notes match “%@”: %@", attachment.embedTarget,
                                         info.candidates.prefix(3).joined(separator: ", ")))
        case "broken":
            showEmbedMessage(L10n.string("No note found for “%@”", attachment.embedTarget))
        default:
            showEmbedMessage(L10n.string("Embedding is unavailable: %@", attachment.embedTarget))
        }
    }

    private func updateScrollStateForNote(measuredHeight: Double, maxHeight: CGFloat) {
        (embedBodyView as? EmbeddedNoteReadingUIView)?.setScrollable(measuredHeight > Double(maxHeight))
    }

    private func installEmbedBody(_ view: UIView, height: Double) {
        embedBodyView?.removeFromSuperview()
        embedBodyView = view
        embedMessageLabel?.isHidden = true
        addSubview(view)
        setEmbedBodyHeight(height)
    }

    private func installEmbedFallbackSource(_ source: String, reason: String) {
        let tv = UITextView()
        tv.isEditable = false
        tv.isSelectable = true
        tv.backgroundColor = .clear
        tv.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        tv.textColor = .secondaryLabel
        tv.text = reason.isEmpty ? source : L10n.string("Diagram could not be rendered") + "\n" + reason + "\n\n" + source
        tv.textContainerInset = UIEdgeInsets(top: 4, left: 6, bottom: 4, right: 6)
        let measured = Double(ceil(tv.sizeThatFits(CGSize(width: max(120, attachment.width - 16), height: .greatestFiniteMagnitude)).height))
        tv.isScrollEnabled = measured > 260
        installEmbedBody(tv, height: min(260, max(44, measured)))
    }

    private func showEmbedMessage(_ text: String) {
        embedBodyView?.removeFromSuperview()
        embedBodyView = nil
        embedMessageLabel?.text = text
        embedMessageLabel?.isHidden = false
        let width = max(120, attachment.width - 28)
        let measured = embedMessageLabel?.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height ?? 20
        setEmbedBodyHeight(Double(max(24, ceil(measured))))
    }

    private func setEmbedBodyHeight(_ bodyHeight: Double) {
        let total = Double(Self.titleAreaHeight) + max(24.0, bodyHeight) + 8.0
        attachment.updatePreferredHeight(total)
        setNeedsLayout()
        layoutIfNeeded()
    }
}

private final class EmbedDiagramCanvasUIView: UIView {
    private let info: EmbedContentInfo
    private var diagram: NoteDiagram
    private let width: Double
    private let dark: Bool
    private let fontSize: Double
    private let handle: VaultHandle?
    private let fallbackSourcePath: String
    private let diagramService: DiagramService
    private let onOpenLink: (String, String?) -> Void
    private let onHeightChange: (Double) -> Void
    private var collapsed: Set<String> = []
    private var rerenderTask: Task<Void, Never>?

    init(info: EmbedContentInfo, initialDiagram: NoteDiagram, width: Double, dark: Bool, fontSize: Double,
         handle: VaultHandle?, fallbackSourcePath: String, diagramService: DiagramService,
         onOpenLink: @escaping (String, String?) -> Void, onHeightChange: @escaping (Double) -> Void) {
        self.info = info
        self.diagram = initialDiagram
        self.width = width
        self.dark = dark
        self.fontSize = fontSize
        self.handle = handle
        self.fallbackSourcePath = fallbackSourcePath
        self.diagramService = diagramService
        self.onOpenLink = onOpenLink
        self.onHeightChange = onHeightChange
        let height = initialDiagram.prepared?.scene.height ?? 120
        super.init(frame: CGRect(x: 0, y: 0, width: width, height: height))
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
        clipsToBounds = true
        let tap = UITapGestureRecognizer(target: self, action: #selector(handleTap(_:)))
        addGestureRecognizer(tap)
        accessibilityIdentifier = "embed-diagram-canvas"
    }

    required init?(coder: NSCoder) { nil }

    deinit {
        rerenderTask?.cancel()
    }

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext(), let prepared = diagram.prepared else { return }
        SceneRenderer.draw(prepared, in: context)
    }

    @objc private func handleTap(_ recognizer: UITapGestureRecognizer) {
        let point = recognizer.location(in: self)
        guard let prepared = diagram.prepared else { return }
        guard let hit = prepared.scene.hitRegions.last(where: { $0.rect.cgRect.contains(point) }) else {
            if let path = info.resolvedPath {
                onOpenLink(path, info.heading)
            }
            return
        }
        if let target = hit.linkTarget, !target.isEmpty {
            let source = info.resolvedPath ?? fallbackSourcePath
            if let handle {
                Task { [onOpenLink] in
                    let resolved = await Task.detached {
                        handle.resolveLink(sourcePath: source, rawTarget: target).resolvedPath
                    }.value
                    onOpenLink(resolved ?? target, hit.linkHeading)
                }
            } else {
                onOpenLink(target, hit.linkHeading)
            }
            return
        }
        if hit.canCollapse {
            if collapsed.contains(hit.nodeId) { collapsed.remove(hit.nodeId) } else { collapsed.insert(hit.nodeId) }
            rerender()
        }
    }

    private func rerender() {
        var block = diagram
        block.collapsed = collapsed
        rerenderTask?.cancel()
        rerenderTask = Task.detached(priority: .userInitiated) { [weak self, info, width, dark, fontSize, diagramService] in
            let rendered = await diagramService.render(
                source: info.content, mmd: info.isMmd, blocks: [block],
                width: width, dark: dark, fontGeneration: fontSize.bitPattern
            )
            guard !Task.isCancelled, let updated = rendered.first, let scene = updated.prepared else { return }
            await MainActor.run {
                guard let self else { return }
                self.diagram = updated
                self.setNeedsDisplay()
                self.onHeightChange(scene.scene.height)
            }
        }
    }
}

private final class EmbeddedNoteReadingUIView: UIView {
    private let info: EmbedContentInfo
    private var diagrams: [NoteDiagram]
    private let media: [PreparedMedia]
    private let math: [PreparedMathBlock]
    private let outerAttachment: MediaBlockAttachment
    private let onHeightChange: (Double) -> Void

    private let textView = HortusReadingUITextView()
    private let layoutDelegate = ReadingLayoutDelegate()
    private var displayRanges: [NSRange] = []
    private var fallbackBlocks: Set<Int> = []
    private var runs: [SourceStyleRun] = []
    private var displayedNS: NSString = ""
    private var diagramTasks: [UInt32: Task<Void, Never>] = [:]
    private var bodyWidth: Double

    init(info: EmbedContentInfo, media: [PreparedMedia], math: [PreparedMathBlock], diagrams: [NoteDiagram],
         attachment: MediaBlockAttachment, bodyWidth: Double, onHeightChange: @escaping (Double) -> Void) {
        self.info = info
        self.media = media
        self.math = math
        self.diagrams = diagrams
        self.outerAttachment = attachment
        self.bodyWidth = bodyWidth
        self.onHeightChange = onHeightChange
        super.init(frame: CGRect(x: 0, y: 0, width: bodyWidth, height: 40))

        textView.frame = bounds
        textView.isEditable = false
        textView.isSelectable = true
        textView.isScrollEnabled = false
        textView.backgroundColor = .clear
        textView.textContainerInset = UIEdgeInsets(top: 4, left: 14, bottom: 6, right: 12)
        textView.textContainer.lineFragmentPadding = 0
        layoutDelegate.view = textView
        layoutDelegate.baseDelegate = textView.textLayoutManager?.delegate
        textView.textLayoutManager?.delegate = layoutDelegate
        textView.onTapAtOffset = { [weak self] offset in
            self?.handleTap(at: offset) ?? false
        }
        textView.onAttachmentLayoutChanged = { [weak self] in
            guard let self else { return }
            let h = Double(self.textView.measureContentHeight(forWidth: CGFloat(self.bodyWidth)))
            self.onHeightChange(h)
        }
        textView.accessibilityIdentifier = "embed-reading-text"
        addSubview(textView)

        buildContent()
    }

    required init?(coder: NSCoder) { nil }

    deinit {
        diagramTasks.values.forEach { $0.cancel() }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        bodyWidth = max(120, Double(bounds.width))
        if textView.frame != bounds {
            textView.frame = bounds
            textView.setNeedsLayout()
        }
    }

    func setScrollable(_ scrollable: Bool) {
        textView.isScrollEnabled = scrollable
    }

    func measureHeight(forWidth width: Double) -> Double {
        bodyWidth = max(120, width)
        return Double(textView.measureContentHeight(forWidth: CGFloat(bodyWidth)))
    }

    private func buildContent() {
        let source = info.content as NSString
        let display = NSMutableString(string: info.text)
        let unresolved = info.links.compactMap { link -> Range<Int>? in
            guard !link.inDiagram, !link.attachment, link.resolvedPath == nil else { return nil }
            return Int(link.startUtf16)..<(Int(link.startUtf16) + Int(link.lengthUtf16))
        }

        var nextRanges: [NSRange] = []
        var nextFallbacks: Set<Int> = []
        var nextRuns: [SourceStyleRun] = []
        var diagramByLocation: [Int: NoteDiagram] = [:]
        var shift = 0
        var ordinal: UInt32 = 0

        for (index, block) in info.blocks.enumerated() {
            let original = NSRange(location: Int(block.displayStart) + shift, length: Int(block.displayLength))
            var range = original
            if block.diagram {
                ordinal += 1
                if let diagram = diagrams.first(where: { $0.ordinal == ordinal }), diagram.prepared != nil,
                   NSMaxRange(original) <= display.length {
                    let slice = display.substring(with: original)
                    let fffcOffset = slice.utf16.firstIndex(of: 0xfffc).map { slice.utf16.distance(from: slice.utf16.startIndex, to: $0) } ?? 0
                    diagramByLocation[original.location + fffcOffset] = diagram
                } else if NSMaxRange(original) <= display.length {
                    let srcRange = NSRange(location: Int(block.sourceStartUtf16), length: Int(block.sourceLengthUtf16))
                    let replacement = NSMaxRange(srcRange) <= source.length ? source.substring(with: srcRange) : ""
                    display.replaceCharacters(in: original, with: replacement)
                    range.length = (replacement as NSString).length
                    shift += range.length - original.length
                    nextFallbacks.insert(index)
                }
            }
            nextRanges.append(range)
            if nextFallbacks.contains(index) {
                nextRuns.append(SourceStyleRun(start: UInt64(range.location), length: UInt64(range.length), mask: 32))
            } else {
                for run in block.runs {
                    let srcStart = Int(block.sourceStartUtf16 + run.sourceStartUtf16)
                    var mask = run.mask
                    if mask & (1 << 18) != 0, unresolved.contains(where: { $0.contains(srcStart) }) {
                        mask |= 1 << 19
                    }
                    nextRuns.append(SourceStyleRun(start: UInt64(range.location) + run.start, length: run.length, mask: mask))
                }
            }
        }

        self.displayRanges = nextRanges
        self.fallbackBlocks = nextFallbacks
        self.runs = nextRuns
        self.displayedNS = (display as String) as NSString
        layoutDelegate.runs = nextRuns
        layoutDelegate.displayedNS = self.displayedNS

        let fontSize = outerAttachment.fontSize
        let attributed = NSMutableAttributedString(
            string: display as String,
            attributes: SourceStylePalette.attributes(for: 0, fontSize: fontSize, contrast: false)
        )
        for run in nextRuns {
            let range = NSRange(location: Int(run.start), length: Int(run.length))
            if NSMaxRange(range) <= attributed.length {
                attributed.setAttributes(SourceStylePalette.attributes(for: run.mask, fontSize: fontSize, contrast: false), range: range)
            }
        }

        let contentWidth = max(120.0, bodyWidth - Double(textView.textContainerInset.left + textView.textContainerInset.right))
        var copyItems: [(range: NSRange, source: String)] = []
        for item in math {
            guard let range = displayRange(start: item.block.displayStart, length: item.block.displayLength),
                  NSMaxRange(range) <= attributed.length else { continue }
            let noteMath = NoteMath(range: range, prepared: item, offset: range.location)
            copyItems.append((range: range, source: item.block.displaySource))
            if item.block.block {
                attributed.addAttribute(.attachment, value: BlockMathAttachment(noteMath, width: contentWidth, size: fontSize), range: range)
            } else {
                let color = (attributed.attribute(.foregroundColor, at: range.location, effectiveRange: nil) as? UIColor) ?? .label
                attributed.addAttribute(.attachment, value: InlineMathAttachment(noteMath, width: contentWidth, size: fontSize, foreground: color), range: range)
            }
        }

        let notePath = info.resolvedPath ?? outerAttachment.notePath
        for (location, diagram) in diagramByLocation {
            let range = NSRange(location: location, length: 1)
            guard NSMaxRange(range) <= attributed.length else { continue }
            let att = DiagramBlockAttachment(
                diagram,
                width: contentWidth,
                showFocusButton: false,
                onFocus: { [weak self] _ in
                    guard let self, let path = self.info.resolvedPath else { return }
                    self.outerAttachment.onOpenLink(path, self.info.heading)
                },
                onToggle: { [weak self] ord, nodeId in
                    self?.toggleDiagram(ordinal: ord, nodeId: nodeId)
                },
                onOpenLink: { [weak self] target, heading in
                    self?.resolveAndOpenLink(target: target, heading: heading, from: notePath)
                }
            )
            attributed.addAttribute(.attachment, value: att, range: range)
        }

        let nestedEmbeds = info.links.filter { $0.embed && !$0.attachment && !$0.inDiagram }
        for item in media {
            guard let range = displayRange(start: item.block.displayStart, length: item.block.displayLength),
                  NSMaxRange(range) <= attributed.length else { continue }
            if item.block.imageWiki && item.reason.hasPrefix("embed.") && outerAttachment.depth >= 3 {
                continue
            }
            var embedTarget = item.block.imageTarget.isEmpty ? item.block.imageAlt : item.block.imageTarget
            var embedHeading: String?
            var embedAlias: String? = item.block.imageAlt.isEmpty ? nil : item.block.imageAlt
            if item.block.imageWiki, item.reason.hasPrefix("embed."),
               let offset = sourceUTF16(byte: Int(item.block.sourceStart)),
               let link = nestedEmbeds.first(where: {
                   UInt64(offset) >= $0.startUtf16 && UInt64(offset) < $0.startUtf16 + $0.lengthUtf16
               }) {
                embedTarget = link.target
                embedHeading = link.heading
                embedAlias = link.alias
            }
            var nextAncestors = outerAttachment.ancestors
            if let path = info.resolvedPath { nextAncestors.append(path) }
            let att = MediaBlockAttachment(
                NoteMediaItem(range: range, prepared: item),
                width: contentWidth,
                fontSize: fontSize,
                vault: outerAttachment.vault,
                notePath: notePath,
                dark: outerAttachment.dark,
                handle: outerAttachment.handle,
                diagramService: outerAttachment.diagramService,
                embedTarget: embedTarget,
                embedHeading: embedHeading,
                embedAlias: embedAlias,
                ancestors: nextAncestors,
                depth: outerAttachment.depth + 1,
                onOpenLink: outerAttachment.onOpenLink,
                onShowTag: outerAttachment.onShowTag,
                onSizeChange: { [weak self] mediaAtt in
                    self?.textView.invalidateAttachmentLayout(for: mediaAtt)
                }
            )
            let slice = (display as NSString).substring(with: range)
            if let fffcOffset = slice.utf16.firstIndex(of: 0xfffc).map({ slice.utf16.distance(from: slice.utf16.startIndex, to: $0) }) {
                attributed.addAttribute(.attachment, value: att, range: NSRange(location: range.location + fffcOffset, length: 1))
            }
        }

        textView.mathCopyItems = copyItems
        textView.attributedText = attributed
        textView.setNeedsLayout()
    }

    private func toggleDiagram(ordinal: UInt32, nodeId: String) {
        guard let idx = diagrams.firstIndex(where: { $0.ordinal == ordinal }) else { return }
        var block = diagrams[idx]
        if block.collapsed.contains(nodeId) { block.collapsed.remove(nodeId) } else { block.collapsed.insert(nodeId) }
        diagrams[idx] = block
        let width = max(120.0, bodyWidth - Double(textView.textContainerInset.left + textView.textContainerInset.right))
        let service = outerAttachment.diagramService
        let fontSize = outerAttachment.fontSize
        let dark = outerAttachment.dark
        diagramTasks[ordinal]?.cancel()
        diagramTasks[ordinal] = Task.detached(priority: .userInitiated) { [weak self, info, block] in
            let rendered = await service.render(
                source: info.content, mmd: info.isMmd, blocks: [block],
                width: width, dark: dark, fontGeneration: fontSize.bitPattern
            )
            guard !Task.isCancelled, let updated = rendered.first, updated.prepared != nil else { return }
            await MainActor.run {
                guard let self, let currentIdx = self.diagrams.firstIndex(where: { $0.ordinal == ordinal }) else { return }
                self.diagrams[currentIdx] = updated
                self.buildContent()
                let measured = Double(self.textView.measureContentHeight(forWidth: CGFloat(self.bodyWidth)))
                self.onHeightChange(measured)
            }
        }
    }

    private func displayRange(start: UInt64, length: UInt64) -> NSRange? {
        guard let index = info.blocks.firstIndex(where: {
            start >= $0.displayStart && start < $0.displayStart + $0.displayLength
        }), !fallbackBlocks.contains(index), index < displayRanges.count else { return nil }
        let location = displayRanges[index].location + Int(start - info.blocks[index].displayStart)
        guard location + Int(length) <= displayedNS.length else { return nil }
        return NSRange(location: location, length: Int(length))
    }

    private func sourceUTF16(byte: Int) -> Int? {
        guard byte >= 0, byte <= info.content.utf8.count else { return nil }
        let utf8 = info.content.utf8
        guard let end = utf8.index(utf8.startIndex, offsetBy: byte, limitedBy: utf8.endIndex) else { return nil }
        return String(decoding: utf8[utf8.startIndex..<end], as: UTF8.self).utf16.count
    }

    private func sourcePosition(at position: Int) -> Int? {
        guard position >= 0, position <= displayedNS.length,
              let index = displayRanges.firstIndex(where: { NSLocationInRange(position, $0) }) else { return nil }
        let block = info.blocks[index]
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

    private func handleTap(at position: Int) -> Bool {
        guard let sourcePos = sourcePosition(at: position) else { return false }
        let mask = layoutDelegate.maskAt(position: position)
        if mask & (1 << 18) != 0, let link = info.links.first(where: {
            !$0.inDiagram && !$0.attachment && UInt64(sourcePos) >= $0.startUtf16
                && UInt64(sourcePos) < $0.startUtf16 + $0.lengthUtf16
        }) {
            if let path = link.resolvedPath {
                outerAttachment.onOpenLink(path, link.heading)
            } else {
                resolveAndOpenLink(target: link.target, heading: link.heading, from: info.resolvedPath ?? outerAttachment.notePath)
            }
            return true
        }
        if mask & (1 << 18) != 0 {
            let ns = info.content as NSString
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
        if mask & 512 != 0, let tag = info.tags.first(where: {
            UInt64(sourcePos) >= $0.startUtf16 && UInt64(sourcePos) < $0.startUtf16 + $0.lengthUtf16
        }) {
            outerAttachment.onShowTag(tag.tag)
            return true
        }
        return false
    }

    private func resolveAndOpenLink(target: String, heading: String?, from sourcePath: String) {
        guard let handle = outerAttachment.handle else {
            outerAttachment.onOpenLink(target, heading)
            return
        }
        Task { [onOpenLink = outerAttachment.onOpenLink] in
            let resolved = await Task.detached {
                handle.resolveLink(sourcePath: sourcePath, rawTarget: target).resolvedPath
            }.value
            onOpenLink(resolved ?? target, heading)
        }
    }
}
