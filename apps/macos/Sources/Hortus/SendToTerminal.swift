import AppKit

/// 发送到终端的纯数据；格式化与 PTY 写入分离，便于单元测试。
struct TerminalPayload: Equatable {
    let text: String
    let bracketedPaste: Bool

    /// 相对库根的路径：安全转义后附一个空格方便继续输入，绝不附换行。
    static func notePath(relativePath: String) -> TerminalPayload {
        TerminalPayload(text: quoted(relativePath) + " ", bracketedPaste: false)
    }

    /// 选区正文带紧凑来源提示；多字符或多行使用 bracketed paste，不自动执行。
    static func selection(relativePath: String, text: String, lineRange: ClosedRange<Int>?) -> TerminalPayload {
        let header: String
        if relativePath.isEmpty {
            header = ""
        } else if let lineRange {
            let lines = lineRange.lowerBound == lineRange.upperBound
                ? "第 \(lineRange.lowerBound) 行"
                : "第 \(lineRange.lowerBound)-\(lineRange.upperBound) 行"
            header = "\(relativePath)（\(lines)）：\n"
        } else {
            header = "\(relativePath)：\n"
        }
        return TerminalPayload(text: header + text,
                               bracketedPaste: text.contains("\n") || text.contains("\r") || text.count > 1)
    }

    /// POSIX 单引号：普通路径原样；含空白、引号或非 ASCII 时整体引用。
    static func quoted(_ path: String) -> String {
        let safe = path.unicodeScalars.allSatisfy { scalar in
            switch scalar.value {
            case 0x30...0x39, 0x41...0x5A, 0x61...0x7A, 0x2D, 0x2E, 0x2F, 0x5F:
                true
            default:
                false
            }
        }
        guard !path.isEmpty, safe else {
            return "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'"
        }
        return path
    }
}

/// 从阅读/编辑视图读取真实选区，并把显示位置映射回源文行号。
@MainActor
enum SendToTerminalSelection {
    typealias Selection = (text: String, lines: ClosedRange<Int>?)

    static func current(in workspace: Workspace, window: NSWindow?) -> Selection? {
        guard let document = workspace.current?.document else { return nil }
        if let responder = window?.firstResponder as? DiagramTextView,
           responder.representedDocument == ObjectIdentifier(document),
           let selection = selection(in: responder, document: document) {
            return selection
        }
        if let view = document.mountedTextView, let selection = selection(in: view, document: document) {
            return selection
        }
        return nil
    }

    static func selection(in view: DiagramTextView, document: NoteDocument) -> Selection? {
        let range = view.selectedRange()
        guard range.location != NSNotFound, range.length > 0 else { return nil }
        let displayed = view.string as NSString
        guard NSMaxRange(range) <= displayed.length else { return nil }
        let text = displayed.substring(with: range)
        guard !text.isEmpty else { return nil }
        if view.isReading {
            guard let coordinator = view.readingCoordinator,
                  let start = coordinator.sourcePosition(at: range.location) else {
                return (text, nil)
            }
            let endPosition = NSMaxRange(range)
            let last = coordinator.sourcePosition(at: endPosition - 1) ?? start
            let after = coordinator.sourcePosition(at: endPosition)
            let end = (after.map { $0 > last ? $0 : last + 1 }) ?? (last + 1)
            return (text, lineRange(in: document.text as NSString, start: start, end: end))
        }
        return (text, lineRange(in: displayed, start: range.location, end: NSMaxRange(range)))
    }

    static func lineRange(in text: NSString, start: Int, end: Int) -> ClosedRange<Int>? {
        guard text.length > 0, end > 0 else { return nil }
        let firstOffset = max(0, min(start, text.length - 1))
        let lastOffset = max(firstOffset, min(end - 1, text.length - 1))
        let first = lineNumber(in: text, offset: firstOffset)
        let last = lineNumber(in: text, offset: lastOffset)
        return first...max(first, last)
    }

    private static func lineNumber(in text: NSString, offset: Int) -> Int {
        let prefix = text.substring(to: offset)
        return 1 + prefix.reduce(into: 0) { count, character in
            if character == "\n" { count += 1 }
        }
    }
}

extension Workspace {
    /// 发送当前笔记的相对路径；无笔记或移动端以外状态时安全无操作。
    @discardableResult
    func sendActiveNotePathToTerminal() -> Bool {
        guard let path = selected, current != nil else { return false }
        return deliverToTerminal(TerminalPayload.notePath(relativePath: path))
    }

    /// 发送当前阅读/编辑视图的非空选区；没有选区时给一次克制提示。
    @discardableResult
    func sendCurrentSelectionToTerminal(window: NSWindow?) -> Bool {
        guard let path = selected, current?.document != nil else { return false }
        guard let selection = SendToTerminalSelection.current(in: self, window: window) else {
            showKnowledgeMessage(L10n.string("Select text to send to the terminal"))
            return false
        }
        return deliverToTerminal(TerminalPayload.selection(relativePath: path,
                                                           text: selection.text,
                                                           lineRange: selection.lines))
    }

    private func deliverToTerminal(_ payload: TerminalPayload) -> Bool {
        guard root != nil, let layout else { return false }
        guard let tab = terminals.targetSessionForSend() ?? addShell(to: nil) else { return false }
        layout.reveal(.terminal(tab.id))
        terminals.focusWhenReady(tab)
        return tab.session.sendText(payload.text, bracketedPaste: payload.bracketedPaste)
    }
}

/// 菜单可用性跟随当前窗口的笔记与真实选区；`NSTextView` 选区变化会触发刷新。
@MainActor
enum SendToTerminalMenuState {
    private static var scheduled = false

    static func scheduleRefresh() {
        guard !scheduled else { return }
        scheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
            scheduled = false
            refresh()
        }
    }

    static func refresh() {
        guard let workspace = (NSApp.delegate as? AppDelegate)?.application?.active?.workspace else { return }
        sync(workspace: workspace)
    }

    static func sync(workspace: Workspace) {
        let noteOpen = workspace.current != nil
        let window = NSApp.keyWindow
            ?? workspace.application?.records.first(where: { $0.workspace === workspace })?.window
        let hasSelection = SendToTerminalSelection.current(in: workspace, window: window) != nil
        setEnabled(noteOpen, item: pathItem())
        setEnabled(noteOpen && hasSelection, item: selectionItem())
    }

    static func pathItem() -> NSMenuItem? {
        item(key: "l", modifiers: [.control, .command])
    }

    static func selectionItem() -> NSMenuItem? {
        item(key: "l", modifiers: [.control, .command, .shift])
    }

    private static func item(key: String, modifiers: NSEvent.ModifierFlags) -> NSMenuItem? {
        func visit(_ menu: NSMenu) -> NSMenuItem? {
            for item in menu.items {
                let itemModifiers = item.keyEquivalentModifierMask.intersection(.deviceIndependentFlagsMask)
                if item.keyEquivalent.lowercased() == key, itemModifiers == modifiers {
                    return item
                }
                if let submenu = item.submenu, let found = visit(submenu) { return found }
            }
            return nil
        }
        return NSApp.mainMenu.flatMap(visit)
    }

    private static func setEnabled(_ enabled: Bool, item: NSMenuItem?) {
        guard let item, item.isEnabled != enabled else { return }
        item.isEnabled = enabled
    }
}
