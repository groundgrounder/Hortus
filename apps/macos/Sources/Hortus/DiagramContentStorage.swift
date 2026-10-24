import AppKit

/// 保留原始文本与位置；仅在显示元素中使用等长的不可见字符，避免排版隐藏源码。
final class DiagramContentStorage: NSTextContentStorage, @preconcurrency NSTextContentStorageDelegate {
    weak var editor: DiagramTextView?
    var diagrams: [NSRange] = [] {
        didSet { if oldValue != diagrams { projections = [:] } }
    }
    var inlineMedia: [NSRange] = [] {
        didSet { if oldValue != inlineMedia { projections = [:] } }
    }
    private var projections: [NSRange: NSTextParagraph] = [:]
    override init() { super.init(); delegate = self }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    @MainActor
    func textContentStorage(_ storage: NSTextContentStorage, textParagraphWith range: NSRange) -> NSTextParagraph? {
        guard diagrams.contains(where: { NSLocationInRange(range.location, $0) }) else {
            guard let view = editor, let styling = view.sourceStyling, let storage = textStorage,
                  !styling.runs.isEmpty, NSMaxRange(range) <= storage.length else { return nil }
            let styled = styling.paragraph(storage.attributedSubstring(from: range), range: range, in: view)
            let hidden = inlineMedia.map { NSIntersectionRange($0, range) }.filter { $0.length > 0 }
            guard !hidden.isEmpty else { return NSTextParagraph(attributedString: styled) }
            let projected = NSMutableAttributedString(attributedString: styled)
            for item in hidden {
                let local = NSRange(location: item.location - range.location, length: item.length)
                projected.replaceCharacters(in: local, with: String(repeating: "\u{2060}", count: local.length))
                projected.setAttributes([.font: NSFont.systemFont(ofSize: 1)], range: local)
            }
            return NSTextParagraph(attributedString: projected)
        }
        if let paragraph = projections[range] { return paragraph }
        let paragraph = NSTextParagraph(attributedString: NSAttributedString(string: String(repeating: "\u{2060}", count: max(0, range.length - 1)) + "\n",
                                                                            attributes: [.font: NSFont.systemFont(ofSize: 1)]))
        projections[range] = paragraph
        return paragraph
    }

    /// 枚举直接越过图内源码；反向枚举仍返回图块首段，边界覆盖被跳过的范围。
    override func enumerateTextElements(from location: (any NSTextLocation)?, options: NSTextContentManager.EnumerationOptions = [],
                                        using block: (NSTextElement) -> Bool) -> (any NSTextLocation)? {
        guard !diagrams.isEmpty else { return super.enumerateTextElements(from: location, options: options, using: block) }
        let reverse = options.contains(.reverse)
        var position = location ?? (reverse ? documentRange.endLocation : documentRange.location)
        var edge: (any NSTextLocation)?
        while true {
            var next: (any NSTextLocation)?
            var stopped = false
            let currentEdge = super.enumerateTextElements(from: position, options: options) { element in
                guard let range = element.elementRange else { return block(element) }
                let offset = self.offset(from: self.documentRange.location, to: range.location)
                if let diagram = self.diagrams.first(where: { offset > $0.location && offset < NSMaxRange($0) }) {
                    let target: Int
                    if reverse, let text = self.textStorage?.string as NSString? {
                        target = NSMaxRange(text.paragraphRange(for: NSRange(location: diagram.location, length: 0)))
                    } else { target = NSMaxRange(diagram) }
                    next = self.location(self.documentRange.location, offsetBy: target)
                    return false
                }
                if !block(element) { stopped = true; return false }
                return true
            }
            if let currentEdge { edge = currentEdge }
            if stopped || next == nil { return edge }
            // 原生组合或跨围栏替换可能暂时使范围落在段落中间，不能在同一位置反复枚举。
            let direction = next!.compare(position)
            if direction != (reverse ? .orderedAscending : .orderedDescending) { return edge }
            position = next!
            edge = position
        }
    }
}
