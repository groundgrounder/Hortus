import AppKit

/// 编辑辅助只生成一次原生文本替换，撤销、选择和输入法仍由 NSTextView 管理。
@MainActor
final class EditorActions {
    private let list = try! NSRegularExpression(pattern: #"^([ \t]*)([-+*]|[0-9]+[.)])([ \t]+)(\[[ xX]\](?:[ \t]+|$))?(.*)$"#)
    private func item(in view: DiagramTextView, at position: Int) -> (NSString, NSRange, NSTextCheckingResult)? {
        guard !view.compositionActive, let storage = view.textStorage, position <= storage.length else { return nil }
        if storage.length > 0, let mask = view.sourceStyling?.mask(at: min(position, storage.length - 1)),
           mask & (16 | 65536) != 0 { return nil }
        let source = storage.string as NSString
        let range = source.lineRange(for: NSRange(location: position, length: 0))
        let line = source.substring(with: range).trimmingCharacters(in: .newlines) as NSString
        guard let match = list.firstMatch(in: line as String, range: NSRange(location: 0, length: line.length)) else { return nil }
        return (line, range, match)
    }
    func newline(in view: DiagramTextView) -> Bool {
        let selection = view.selectedRange()
        guard selection.length == 0, let (line, range, match) = item(in: view, at: selection.location),
              selection.location == range.location + line.length else { return false }
        let indent = line.substring(with: match.range(at: 1))
        let marker = line.substring(with: match.range(at: 2))
        let spacing = line.substring(with: match.range(at: 3))
        let task = match.range(at: 4).location != NSNotFound
        let body = line.substring(with: match.range(at: 5))
        if body.trimmingCharacters(in: .whitespaces).isEmpty {
            replace(view, range: NSRange(location: range.location, length: line.length), text: "")
        } else {
            let next: String
            if task { next = "- [ ] " }
            else if let number = UInt64(marker.dropLast()), number < UInt64.max { next = "\(number + 1)\(marker.suffix(1))" + spacing }
            else { next = marker + spacing }
            replace(view, range: selection, text: "\n" + indent + next)
        }
        return true
    }
    func indent(in view: DiagramTextView, backwards: Bool) -> Bool {
        let selection = view.selectedRange()
        guard item(in: view, at: selection.location) != nil, let storage = view.textStorage else { return false }
        let source = storage.string as NSString
        var range = source.lineRange(for: selection)
        if selection.length > 0, NSMaxRange(selection) < NSMaxRange(range),
           NSMaxRange(selection) > range.location, source.character(at: NSMaxRange(selection) - 1) == 10 {
            range = source.lineRange(for: NSRange(location: selection.location, length: selection.length - 1))
        }
        let original = source.substring(with: range)
        var lines = original.components(separatedBy: "\n")
        for i in lines.indices where !(i == lines.count - 1 && lines[i].isEmpty) {
            if backwards {
                if lines[i].hasPrefix("\t") { lines[i].removeFirst() }
                else { let n = min(4, lines[i].prefix(while: { $0 == " " }).count); lines[i].removeFirst(n) }
            } else { lines[i] = "    " + lines[i] }
        }
        let replacement = lines.joined(separator: "\n")
        guard replacement != original else { return true }
        replace(view, range: range, text: replacement)
        if selection.length == 0 {
            let firstDelta = (lines[0] as NSString).length - (original.components(separatedBy: "\n")[0] as NSString).length
            view.setSelectedRange(NSRange(location: max(range.location, selection.location + firstDelta), length: 0))
        } else { view.setSelectedRange(NSRange(location: range.location, length: (replacement as NSString).length)) }
        return true
    }
    @discardableResult
    func toggleTask(in view: DiagramTextView, at position: Int? = nil) -> Bool {
        let selection = view.selectedRange()
        guard let (line, range, match) = item(in: view, at: position ?? selection.location), match.range(at: 4).location != NSNotFound else { return false }
        let check = range.location + match.range(at: 4).location + 1
        let checked = line.character(at: match.range(at: 4).location + 1) != 32
        replace(view, range: NSRange(location: check, length: 1), text: checked ? " " : "x")
        view.setSelectedRange(selection)
        return true
    }
    func clickTask(in view: DiagramTextView, event: NSEvent) -> Bool {
        let point = view.convert(event.locationInWindow, from: nil)
        let position = view.characterIndexForInsertion(at: point)
        guard let (_, lineRange, match) = item(in: view, at: position), match.range(at: 4).location != NSNotFound else { return false }
        let check = NSRange(location: lineRange.location + match.range(at: 4).location, length: 3)
        let rect = view.firstRect(forCharacterRange: check, actualRange: nil)
        guard let window = view.window, rect.insetBy(dx: -2, dy: -2).contains(window.convertPoint(toScreen: event.locationInWindow)) else { return false }
        window.makeFirstResponder(view)
        return toggleTask(in: view, at: position)
    }
    private func replace(_ view: DiagramTextView, range: NSRange, text: String) {
        guard view.isEditable else { return }
        view.breakUndoCoalescing()
        view.insertText(text, replacementRange: range)
        view.breakUndoCoalescing()
    }
}

extension DiagramTextView {
    @objc func alignTableSource(_ sender: Any?) { editingCoordinator?.alignTable() }
    override func insertNewline(_ sender: Any?) {
        if !editorActions.newline(in: self) { super.insertNewline(sender) }
    }
    override func insertTab(_ sender: Any?) {
        if !editorActions.indent(in: self, backwards: false) { super.insertTab(sender) }
    }
    override func insertBacktab(_ sender: Any?) {
        if !editorActions.indent(in: self, backwards: true) { super.insertBacktab(sender) }
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection(.deviceIndependentFlagsMask) == [.option, .command],
           event.charactersIgnoringModifiers?.lowercased() == "t", isEditable {
            alignTableSource(nil); return true
        }
        if event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
           event.charactersIgnoringModifiers == "\r", editorActions.toggleTask(in: self) { return true }
        return super.performKeyEquivalent(with: event)
    }
    override func mouseDown(with event: NSEvent) {
        if isReading, readingCoordinator?.clickTask(event) == true { return }
        if !editorActions.clickTask(in: self, event: event) { super.mouseDown(with: event) }
    }
    override func menu(for event: NSEvent) -> NSMenu? {
        let menu = super.menu(for: event)
        if isReading {
            let item = NSMenuItem(title: L10n.string("Copy Markdown Source"), action: #selector(copyMarkdown(_:)), keyEquivalent: "")
            item.target = self; menu?.addItem(item)
        }
        return menu
    }
    @objc func copyMarkdown(_ sender: Any?) { readingCoordinator?.copyMarkdown() }
    override func paste(_ sender: Any?) { _ = readSelection(from: .general, type: .string) }
    override func pasteAsPlainText(_ sender: Any?) { paste(sender) }
    override func pasteAsRichText(_ sender: Any?) { paste(sender) }
    override func readSelection(from pboard: NSPasteboard, type: NSPasteboard.PasteboardType) -> Bool {
        let plain = pboard.string(forType: .string) ?? pboard.data(forType: .rtf).flatMap {
            try? NSAttributedString(data: $0, options: [.documentType: NSAttributedString.DocumentType.rtf], documentAttributes: nil).string
        }
        guard isEditable, let text = plain else { return false }
        breakUndoCoalescing(); insertText(text, replacementRange: selectedRange()); breakUndoCoalescing()
        return true
    }
}
