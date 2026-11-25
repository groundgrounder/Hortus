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
    let onFocus: (UInt32) -> Void
    let onToggle: (UInt32, String) -> Void
    let onOpenLink: (String, String?) -> Void

    init(_ diagram: NoteDiagram, width: Double, onFocus: @escaping (UInt32) -> Void,
         onToggle: @escaping (UInt32, String) -> Void, onOpenLink: @escaping (String, String?) -> Void) {
        let clampedWidth = max(120, width)
        self.diagram = diagram
        self.width = clampedWidth
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
    let onOpenLink: (String, String?) -> Void
    let preferredSize: CGSize

    init(_ media: NoteMediaItem, width: Double, fontSize: Double, vault: String, notePath: String,
         dark: Bool, handle: VaultHandle?, diagramService: DiagramService,
         onOpenLink: @escaping (String, String?) -> Void) {
        self.media = media
        self.width = max(120, width)
        self.fontSize = fontSize
        self.vault = vault
        self.notePath = notePath
        self.dark = dark
        self.handle = handle
        self.diagramService = diagramService
        self.onOpenLink = onOpenLink

        let block = media.prepared.block
        if !block.columns.isEmpty {
            let tableHeight = TableLayoutMetrics.measure(block: block, size: fontSize, math: media.prepared.math).totalHeight
            self.preferredSize = CGSize(width: self.width, height: max(44, tableHeight + 8))
        } else if media.prepared.block.imageWiki && media.prepared.reason.hasPrefix("embed.") {
            self.preferredSize = CGSize(width: self.width, height: 112)
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

private enum TableLayoutMetrics {
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

private final class TableCanvasUIView: UIView {
    let measured: TableLayoutMetrics.Measured
    init(measured: TableLayoutMetrics.Measured) {
        self.measured = measured
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
        var y: CGFloat = 0
        UIColor.separator.resolvedColor(with: traitCollection).setStroke()
        let lines = UIBezierPath()
        lines.lineWidth = 0.5
        for (row, height) in measured.heights.enumerated() {
            var x: CGFloat = 0
            for (col, width) in measured.columns.enumerated() {
                let text = measured.texts[row * measured.columns.count + col]
                let cellRect = CGRect(x: x + 10, y: y + 8, width: width - 20, height: height - 8)
                text.draw(with: cellRect, options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil)
                x += width
            }
            y += height
            lines.move(to: CGPoint(x: 0, y: y))
            lines.addLine(to: CGPoint(x: bounds.width, y: y))
        }
        lines.stroke()
    }
}

private final class MediaBlockHostUIView: UIView {
    private let attachment: MediaBlockAttachment
    private var imageView: UIImageView?
    private var embedLabel: UILabel?
    private var embedTitleButton: UIButton?
    private var resolvedEmbedPath: String?
    private var resolvedEmbedHeading: String?

    init(attachment: MediaBlockAttachment) {
        self.attachment = attachment
        super.init(frame: CGRect(origin: .zero, size: attachment.preferredSize))
        isOpaque = false
        backgroundColor = .clear

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
        layer.borderColor = UIColor.separator.cgColor

        let button = UIButton(type: .system)
        button.contentHorizontalAlignment = .leading
        button.titleLabel?.font = .systemFont(ofSize: 13, weight: .semibold)
        button.setTitle(attachment.media.prepared.block.imageAlt, for: .normal)
        button.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            if let path = self.resolvedEmbedPath {
                self.attachment.onOpenLink(path, self.resolvedEmbedHeading)
            }
        }, for: .touchUpInside)
        button.translatesAutoresizingMaskIntoConstraints = false

        let preview = UILabel()
        preview.font = .systemFont(ofSize: 13)
        preview.textColor = .secondaryLabel
        preview.numberOfLines = 3
        preview.text = L10n.string("Loading notes")
        preview.translatesAutoresizingMaskIntoConstraints = false

        addSubview(button)
        addSubview(preview)
        NSLayoutConstraint.activate([
            button.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            button.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            button.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            preview.topAnchor.constraint(equalTo: button.bottomAnchor, constant: 4),
            preview.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            preview.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            preview.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -8),
        ])
        self.embedTitleButton = button
        self.embedLabel = preview

        guard let handle = attachment.handle else { return }
        let sourcePath = attachment.notePath
        let target = attachment.media.prepared.block.imageAlt
        let width = attachment.width
        let size = attachment.fontSize
        Task.detached(priority: .userInitiated) { [weak self] in
            let info = try? handle.resolveEmbed(
                sourcePath: sourcePath, rawTarget: target, heading: nil, alias: nil,
                ancestors: [sourcePath], maxDepth: 3, width: width, size: size
            )
            await MainActor.run {
                guard let self, let info else { return }
                self.resolvedEmbedPath = info.resolvedPath
                self.resolvedEmbedHeading = info.heading
                let title = info.displayTitle.isEmpty ? target : info.displayTitle
                self.embedTitleButton?.setTitle(title, for: .normal)
                let body = info.text.trimmingCharacters(in: .whitespacesAndNewlines)
                self.embedLabel?.text = body.isEmpty ? (info.resolvedPath ?? target) : body
            }
        }
    }
}
