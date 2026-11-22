#if DEBUG
import AppKit
import CryptoKit
import HortusCore

/// 显式验收：真实窗口里发送路径与选区，等待真实 PTY 收到字节并核对终端界面。
@MainActor
enum SendToTerminalInspection {
    static let argumentPrefix = "--send-to-terminal-inspection="
    static let requested = ProcessInfo.processInfo.arguments.contains { $0.hasPrefix(argumentPrefix) }

    private static func argument(_ name: String, in arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }.map { String($0.dropFirst(name.count + 3)) }
    }

    private static func failure(_ detail: String) -> NSError {
        NSError(domain: "Hortus.SendToTerminal", code: 1, userInfo: [NSLocalizedDescriptionKey: detail])
    }

    private static func wait(_ label: String, seconds: Double = 20, _ check: () -> Bool) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while !check() {
            guard ProcessInfo.processInfo.systemUptime < deadline else { throw failure("timeout: " + label) }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    private static func hashes(_ root: URL) throws -> [String: String] {
        guard let entries = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isDirectoryKey]) else {
            throw failure("vault listing")
        }
        var result: [String: String] = [:]
        for case let url as URL in entries {
            let path = String(url.path.dropFirst(root.path.count + 1))
            result[path] = try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true ? "directory"
                : SHA256.hash(data: try Data(contentsOf: url)).map { String(format: "%02x", $0) }.joined()
        }
        return result
    }

    private static func screen(_ session: TerminalSession) -> String {
        guard let view = session.inspectionView else { return "" }
        return view.visibleRowsText(0..<view.terminalDimensions.rows).joined(separator: "\n")
    }

    private static func captureData(_ path: String) -> Data {
        (try? Data(contentsOf: URL(fileURLWithPath: path))) ?? Data()
    }

    private static func waitForCapture(_ path: String, suffix: Data) async throws {
        try await wait("PTY capture reaches \(suffix.count) bytes") {
            let data = captureData(path)
            return data.count >= suffix.count && data.suffix(suffix.count) == suffix
        }
    }

    private static func framed(_ payload: TerminalPayload) -> Data {
        var data = Data()
        if payload.bracketedPaste { data.append(Data("\u{1b}[200~".utf8)) }
        data.append(Data(payload.text.utf8))
        if payload.bracketedPaste { data.append(Data("\u{1b}[201~".utf8)) }
        return data
    }

    private static func sendRecord(_ session: TerminalSession, after index: Int) async throws -> TerminalSession.InspectionSend {
        try await wait("terminal records send #\(index + 1)") {
            session.inspectionSends.count > index
        }
        return session.inspectionSends[index]
    }

    private static func describe(_ record: TerminalSession.InspectionSend, expected: TerminalPayload? = nil) -> [String: Any] {
        var value: [String: Any] = [
            "text": record.text,
            "bracketed_paste": record.bracketedPaste,
            "tab_kind": record.tabKind,
            "queued": record.queued,
            "auto_enter": record.text.hasSuffix("\n") || record.text.hasSuffix("\r"),
        ]
        if let expected {
            value["matches_expected"] = record.text == expected.text
                && record.bracketedPaste == expected.bracketedPaste
        }
        return value
    }

    private static func screenContains(_ session: TerminalSession, _ text: String, seconds: Double = 10) async -> Bool {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while ProcessInfo.processInfo.systemUptime < deadline {
            if screen(session).contains(text) { return true }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return false
    }

    private static func menuSnapshot() -> [String: Any] {
        func item(_ value: NSMenuItem?) -> [String: Any] {
            guard let value else { return ["present": false] }
            return [
                "present": true,
                "title": value.title,
                "key": value.keyEquivalent,
                "modifiers": Int(value.keyEquivalentModifierMask.intersection(.deviceIndependentFlagsMask).rawValue),
                "enabled": value.isEnabled,
            ]
        }
        return ["path": item(SendToTerminalMenuState.pathItem()),
                "selection": item(SendToTerminalMenuState.selectionItem())]
    }

    private static func range(of text: String, in view: DiagramTextView, from startMarker: String, to endMarker: String) -> NSRange? {
        let displayed = view.string
        guard let start = displayed.range(of: startMarker),
              let end = displayed.range(of: endMarker, range: start.upperBound..<displayed.endIndex) else { return nil }
        return NSRange(start.lowerBound..<end.upperBound, in: displayed)
    }

    static func run(workspace: Workspace, layout: Layout, arguments: [String]) async throws -> [String: Any] {
        guard let note = argument("send-to-terminal-note", in: arguments),
              let selectionText = argument("send-to-terminal-selection", in: arguments),
              let root = workspace.root,
              let application = workspace.application,
              let window = application.records.first(where: { $0.workspace === workspace })?.window
        else { throw failure("missing note, selection or window") }
        let capturePath = argument("send-to-terminal-capture", in: arguments)
        let readyPath = argument("send-to-terminal-ready", in: arguments)
        let snapshotPath = argument("send-to-terminal-snapshot", in: arguments)
        let rootURL = URL(fileURLWithPath: root)
        let before = try hashes(rootURL)
        var result: [String: Any] = [
            "note": note,
            "selection_text": selectionText,
            "terminal_was_hidden": !layout.isShown(.bottom),
        ]

        workspace.openNote(note)
        try await wait("note document") { workspace.current?.document != nil }
        guard let document = workspace.current?.document else { throw failure("note document") }
        let reading = try await ReadingInspection.settled(true, document: document)
        reading.window?.makeFirstResponder(reading)
        layout.setTerminalVisible(false)
        try await wait("terminal panel hidden") { !layout.isShown(.bottom) }

        // 记录型 shell 先就绪，再发送，避免 stty 切换模式时丢弃已排队字节。
        if let readyPath {
            workspace.showTerminal()
            try await wait("terminal starts for the recorder") {
                workspace.currentTerminalSession?.state == .running
                    && workspace.currentTerminalSession?.inspectionShellPid ?? 0 > 0
            }
            try await wait("recorder is ready") { FileManager.default.fileExists(atPath: readyPath) }
        }

        // 1. 路径：终端收起时自动打开、聚焦，并只发送一次。
        let pathPayload = TerminalPayload.notePath(relativePath: note)
        let baseline = workspace.currentTerminalSession?.inspectionSends.count ?? 0
        result["path_sent"] = workspace.sendActiveNotePathToTerminal()
        try await wait("terminal starts after sending") {
            workspace.currentTerminalSession?.state == .running
                && workspace.currentTerminalSession?.inspectionShellPid ?? 0 > 0
                && workspace.currentTerminalSession?.inspectionViewAttached == true
        }
        guard let session = workspace.currentTerminalSession, let terminalView = session.inspectionView else {
            throw failure("terminal session")
        }
        try await wait("terminal focused") { window.firstResponder === terminalView }
        let pathRecord = try await sendRecord(session, after: baseline)
        result["terminal_opened"] = layout.isShown(.bottom) && layout.terminalVisible
        result["terminal_focused"] = window.firstResponder === terminalView
        result["path"] = describe(pathRecord, expected: pathPayload)
        result["path_has_trailing_newline"] = pathRecord.text.hasSuffix("\n") || pathRecord.text.hasSuffix("\r")
        if let capturePath {
            try await waitForCapture(capturePath, suffix: framed(pathPayload))
            result["path_capture_bytes"] = framed(pathPayload).count
        } else {
            result["path_screen"] = await screenContains(session, TerminalPayload.quoted(note))
        }

        // 2. 编辑视图多行中文/Emoji 选区：bracketed paste，绝不自动执行。
        workspace.toggleView()
        let editing = try await ReadingInspection.settled(false, document: document)
        editing.window?.makeFirstResponder(editing)
        let displayed = editing.string as NSString
        let editingRange = displayed.range(of: selectionText)
        guard editingRange.location != NSNotFound, editingRange.length > 1 else {
            throw failure("selection text is not displayed in the editor")
        }
        editing.setSelectedRange(editingRange)
        let editingBaseline = session.inspectionSends.count
        let editingLines = SendToTerminalSelection.lineRange(in: displayed,
                                                             start: editingRange.location,
                                                             end: NSMaxRange(editingRange))
        let editingPayload = TerminalPayload.selection(relativePath: note, text: selectionText,
                                                       lineRange: editingLines)
        result["editing_line_range"] = editingLines.map { [$0.lowerBound, $0.upperBound] } ?? []
        result["selection_sent"] = workspace.sendCurrentSelectionToTerminal(window: window)
        let selectionRecord = try await sendRecord(session, after: editingBaseline)
        result["selection"] = describe(selectionRecord, expected: editingPayload)
        result["selection_contains_han"] = selectionRecord.text.unicodeScalars.contains {
            (0x4E00...0x9FFF).contains($0.value)
        }
        result["selection_contains_emoji"] = selectionRecord.text.unicodeScalars.contains { $0.value > 0x1F000 }
        result["selection_has_trailing_newline"] = selectionRecord.text.hasSuffix("\n")
            || selectionRecord.text.hasSuffix("\r")
        result["selection_bracketed"] = selectionRecord.bracketedPaste
        if let header = selectionRecord.text.split(separator: "\n", maxSplits: 1).first {
            result["selection_header"] = String(header)
        }
        let selectionBytes = framed(TerminalPayload(text: selectionRecord.text,
                                                    bracketedPaste: selectionRecord.bracketedPaste))
        if let capturePath {
            try await waitForCapture(capturePath, suffix: selectionBytes)
            result["selection_capture_bytes"] = selectionBytes.count
            result["selection_capture_matches"] = captureData(capturePath).suffix(selectionBytes.count) == selectionBytes
        } else {
            let firstLine = selectionText.components(separatedBy: "\n").first ?? selectionText
            result["selection_screen"] = await screenContains(session, firstLine)
        }
        if let snapshotPath {
            try await Task.sleep(for: .milliseconds(300))
            try await AppDelegate.snapshot(to: snapshotPath, window: window)
            result["snapshot"] = snapshotPath
        }

        // 3. 阅读视图的显示选区也走同一映射，不吞多字节字符。
        workspace.toggleView()
        let rereading = try await ReadingInspection.settled(true, document: document)
        rereading.window?.makeFirstResponder(rereading)
        let firstLine = selectionText.components(separatedBy: "\n").first ?? selectionText
        let lastLine = selectionText.components(separatedBy: "\n").last ?? selectionText
        let readingBaseline = session.inspectionSends.count
        if let range = range(of: selectionText, in: rereading, from: firstLine, to: lastLine) {
            rereading.setSelectedRange(range)
            result["reading_selection_sent"] = workspace.sendCurrentSelectionToTerminal(window: window)
            if workspace.currentTerminalSession?.inspectionSends.count ?? 0 > readingBaseline {
                let record = try await sendRecord(session, after: readingBaseline)
                result["reading_selection"] = describe(record)
                result["reading_selection_header"] = record.text.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? ""
                result["reading_selection_bracketed"] = record.bracketedPaste
                result["reading_selection_has_han"] = record.text.unicodeScalars.contains {
                    (0x4E00...0x9FFF).contains($0.value)
                }
                let payload = framed(TerminalPayload(text: record.text, bracketedPaste: record.bracketedPaste))
                if let capturePath {
                    try await waitForCapture(capturePath, suffix: payload)
                    result["reading_capture_bytes"] = payload.count
                }
            }
        } else {
            result["reading_selection_missing_markers"] = true
        }

        // 4. 空选区：不发送、不写 PTY，只给一次克制提示。
        rereading.setSelectedRange(NSRange(location: 0, length: 0))
        let emptyBaseline = session.inspectionSends.count
        result["empty_selection_sent"] = workspace.sendCurrentSelectionToTerminal(window: window)
        try await Task.sleep(for: .milliseconds(120))
        result["empty_selection_no_send"] = session.inspectionSends.count == emptyBaseline
        result["empty_selection_message"] = workspace.knowledgeMessage ?? ""

        // 5. 菜单：有选区时两条命令都可用，清空后只禁用选区命令。
        rereading.setSelectedRange(range(of: selectionText, in: rereading, from: firstLine, to: lastLine)
            ?? NSRange(location: 0, length: 0))
        SendToTerminalMenuState.sync(workspace: workspace)
        result["menu_with_selection"] = menuSnapshot()
        rereading.setSelectedRange(NSRange(location: 0, length: 0))
        SendToTerminalMenuState.sync(workspace: workspace)
        result["menu_without_selection"] = menuSnapshot()
        result["menu_path_title"] = L10n.string("Send Note Path to Terminal")
        result["menu_selection_title"] = L10n.string("Send Selection to Terminal")

        result["terminal_pid"] = Int(session.inspectionShellPid)
        result["vault_unchanged"] = before == (try hashes(rootURL))
        result["app_pid"] = Int(getpid())
        return result
    }
}
#endif
