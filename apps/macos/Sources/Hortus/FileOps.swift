import AppKit
import HortusCore

/// 文件命名弹窗：NSAlert + 文本框附件，一次提交并就地回显校验错误；
/// 取消不触碰磁盘，确认后由调用方在后台执行 I/O。
@MainActor
enum FileNaming {
    /// 与内核 `vault.invalid_name` 同口径的本地校验；返回可向用户展示的错误文案。
    static func localError(_ name: String) -> String? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty || trimmed == "." || trimmed == ".."
            || trimmed.hasPrefix(".") || trimmed == "node_modules"
            || trimmed.contains("/") || trimmed.contains("\\")
            || trimmed.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) {
            return L10n.string("The name is invalid")
        }
        return nil
    }

    /// 笔记默认高亮主干：主干为主文件名（不含扩展名），便于直接改名。
    private static func stemLength(_ name: String) -> Int {
        guard let dot = name.lastIndex(of: "."), dot != name.startIndex else { return name.count }
        return name.utf16.distance(from: name.startIndex, to: dot)
    }
    static func present(window: NSWindow?, title: String, button: String, initial: String,
                        note: Bool, error: String) async -> String? {
        let alert = NSAlert()
        alert.messageText = title
        if !error.isEmpty { alert.informativeText = error }
        let field = NSTextField(string: initial)
        field.frame = NSRect(x: 0, y: 0, width: 264, height: 24)
        field.font = .systemFont(ofSize: 13, weight: .medium)
        alert.accessoryView = field
        alert.addButton(withTitle: button)
        alert.addButton(withTitle: L10n.string("Cancel"))
        alert.buttons[1].keyEquivalent = "\u{1b}"
        alert.window.initialFirstResponder = field
        let selection = note ? stemLength(initial) : initial.utf16.count
        let response: NSApplication.ModalResponse
        if let window {
            // 回调式弹层让出主线程：弹层出现后按延迟重试预选主干。
            response = await withCheckedContinuation { continuation in
                alert.beginSheetModal(for: window) { response in
                    continuation.resume(returning: response)
                }
                for delay in [0.1, 0.3] {
                    DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                        if let editor = field.window?.fieldEditor(false, for: field) as? NSTextView {
                            editor.selectedRange = NSRange(location: 0, length: selection)
                        }
                    }
                }
            }
        } else {
            response = alert.runModal()
        }
        guard response == .alertFirstButtonReturn else { return nil }
        return field.stringValue
    }
}

/// 文件管理交互：新建笔记/文件夹、重命名与移到废纸篓。
/// I/O 在后台线程执行；树缓存、标签与搜索索引立即同步。
extension Workspace {
    /// 目标父目录：文件树选中文件夹→其下；选中笔记→父目录；无选中→库根。
    func parentForNewEntry() -> String {
        guard let focus = folderSelection ?? selected, let entry = entries[focus] else { return "" }
        return entry.directory ? focus : entry.parent
    }

    /// 默认建议名的自动编号：与既有条目（大小写不敏感）冲突时追加 “ 2”“ 3”。
    func suggestedName(base: String, ext: String, parent: String) -> String {
        let names = Set(entries.values
            .filter { $0.parent == parent }
            .map { $0.name.lowercased() })
        var candidate = ext.isEmpty ? base : "\(base).\(ext)"
        var index = 2
        while names.contains(candidate.lowercased()) {
            candidate = ext.isEmpty ? "\(base) \(index)" : "\(base) \(index).\(ext)"
            index += 1
        }
        return candidate
    }

    private func namingWindow() -> NSWindow? {
        application?.records.first(where: { $0.workspace === self })?.window
    }

    private func runFileOp<T: Sendable>(_ operation: @escaping @Sendable (VaultHandle) throws -> T) async throws -> T {
        guard let handle = context?.handle else { throw CocoaError(.fileReadUnknown) }
        return try await Task.detached { try operation(handle) }.value
    }

    /// 新建笔记核心流程：磁盘创建、树缓存更新、编辑视图打开并聚焦。
    func createNote(parent: String, name: String) async throws -> TreeEntry {
        let entry = try await runFileOp { try $0.createNote(parent: parent, name: name) }
        finishNewNote(entry)
        return entry
    }

    /// 新建文件夹核心流程：磁盘创建、树缓存更新并选中该文件夹。
    func createFolder(parent: String, name: String) async throws -> TreeEntry {
        let entry = try await runFileOp { try $0.createFolder(parent: parent, name: name) }
        finishNewFolder(entry)
        return entry
    }

    /// 重命名核心流程：磁盘重命名并跨全部同库窗口迁移标签、会话与索引。
    func renameEntry(path: String, newName: String) async throws -> TreeEntry {
        let entry = try await runFileOp { try $0.renameEntry(path: path, newName: newName) }
        finishRename(oldPath: path, entry: entry)
        return entry
    }

    /// 确认后的移到废纸篓核心流程：安全解析、系统废纸篓并同步树与索引。
    /// 返回系统废纸篓返回的 `resultingItemURL` 路径。
    @discardableResult
    func trashEntry(path: String) async throws -> String? {
        guard let handle = context?.handle, let entry = entries[path] else { return nil }
        let absolute = try await Task.detached { try handle.resolveTrashTarget(path: path) }.value
        let resulting = try await Task.detached { () -> String? in
            var moved: NSURL?
            try FileManager.default.trashItem(at: URL(fileURLWithPath: absolute), resultingItemURL: &moved)
            return moved?.path
        }.value
        context?.removeTrashed(path, directory: entry.directory)
        if let focus = folderSelection, focus == path || (entry.directory && focus.hasPrefix(path + "/")) {
            folderSelection = nil
        }
        FileOpsOutcome.lastTrashResult = ["path": absolute, "resulting": resulting]
        return resulting
    }

    // MARK: 新建笔记与文件夹

    func requestNewNote(parent: String? = nil) {
        let target = parent ?? parentForNewEntry()
        let suggested = suggestedName(base: L10n.string("New Note"), ext: "md", parent: target)
        let window = namingWindow()
        Task { @MainActor [weak self] in
            guard let self else { return }
            await self.runNamingFlow(window: window, title: L10n.string("New Note"), target: target,
                                     suggested: suggested, note: false, button: L10n.string("Create")) { parent, name in
                _ = try await self.createNote(parent: parent, name: name)
            }
        }
    }

    func requestNewFolder(parent: String? = nil) {
        let target = parent ?? parentForNewEntry()
        let suggested = suggestedName(base: L10n.string("New Folder"), ext: "", parent: target)
        let window = namingWindow()
        Task { @MainActor [weak self] in
            guard let self else { return }
            await self.runNamingFlow(window: window, title: L10n.string("New Folder"), target: target,
                                     suggested: suggested, note: false, button: L10n.string("Create")) { parent, name in
                _ = try await self.createFolder(parent: parent, name: name)
            }
        }
    }

    /// 命名弹窗的提交-校验循环：本地与磁盘错误都在弹窗内就地回显，取消不触碰磁盘。
    private func runNamingFlow(window: NSWindow?, title: String, target: String, suggested: String,
                               note: Bool, button: String,
                               perform: @MainActor (String, String) async throws -> Void) async {
        var validation = ""
        var current = suggested
        while true {
            guard let name = await FileNaming.present(window: window, title: title,
                                                      button: button, initial: current,
                                                      note: note, error: validation) else { return }
            if let message = FileNaming.localError(name) { validation = message; continue }
            do {
                try await perform(target, name)
                return
            } catch {
                current = name
                validation = L10n.error(error)
            }
        }
    }

    /// 新建笔记成功：树缓存先行更新，编辑视图打开并聚焦正文。
    private func finishNewNote(_ entry: TreeEntry) {
        context?.upsert(entry)
        pendingEditingPaths.insert(entry.path)
        openNote(entry.path)
        Task { @MainActor [weak self] in
            guard let self else { return }
            for _ in 0..<100 {
                if let document = self.tabs.first(where: { $0.path == entry.path })?.document, !document.reading {
                    self.focusEditor(document)
                    return
                }
                try? await Task.sleep(for: .milliseconds(10))
            }
        }
    }

    /// 新建文件夹成功：树缓存更新并选中该文件夹。
    private func finishNewFolder(_ entry: TreeEntry) {
        context?.upsert(entry)
        folderSelection = entry.path
    }
    // MARK: 重命名

    func requestRename(path: String) {
        guard let entry = entries[path] else { return }
        let window = namingWindow()
        Task { @MainActor [weak self] in
            guard let self else { return }
            await self.runNamingFlow(window: window, title: L10n.string("Rename"), target: path,
                                      suggested: entry.name, note: !entry.directory,
                                      button: L10n.string("Rename")) { _, name in
                _ = try await self.renameEntry(path: path, newName: name)
            }
        }
    }



    /// 重命名成功：同库全部窗口的标签与会话迁移，文件树与搜索索引立即同步。
    private func finishRename(oldPath: String, entry: TreeEntry) {
        application?.entryRenamed(root: root ?? "", oldPath: oldPath, entry: entry)
    }

    // MARK: 移到废纸篓

    /// 移到废纸篓入口：先扫描同库全部受影响标签，未保存文档受保护；
    /// 确认后停止会话并跨窗口关闭标签，再经安全解析调用系统废纸篓。
    func requestTrash(path: String) {
        guard let root, let entry = entries[path] else { return }
        let directory = entry.directory
        let affected = application?.tabsUnder(root: root, path: path, directory: directory) ?? []
        var unsaved: [String] = []
        for (record, tabPath) in affected {
            guard let document = record.workspace.tabs.first(where: { $0.path == tabPath })?.document else { continue }
            if document.dirty || document.marked || document.issue == .conflict {
                unsaved.append(URL(fileURLWithPath: tabPath).lastPathComponent)
            }
        }
        guard decideTrash(unsaved: unsaved, target: entry.name) else { return }
        for (record, tabPath) in affected { record.workspace.discardTabForTrash(tabPath) }
        Task { @MainActor in
            do { _ = try await trashEntry(path: path) }
            catch { self.error = L10n.string("Moving to the Trash failed") }
        }
    }

    /// 未保存保护的确认提示；测试通过 promptTrash 注入决策。
    func decideTrash(unsaved: [String], target: String) -> Bool {
        if let promptTrash { return promptTrash(unsaved, target) }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = L10n.string("Move “%@” to Trash?", target)
        if unsaved.isEmpty {
            alert.informativeText = L10n.string("You can put it back from the Trash.")
        } else {
            alert.informativeText = L10n.string("These notes have unsaved changes and will be discarded:")
                + "\n\n" + Self.noteList(unsaved)
        }
        alert.addButton(withTitle: unsaved.isEmpty
            ? L10n.string("Move to Trash")
            : L10n.string("Discard Changes and Move to Trash"))
        alert.buttons[0].hasDestructiveAction = true
        alert.addButton(withTitle: L10n.string("Cancel"))
        return alert.runModal() == .alertFirstButtonReturn
    }

}

/// 自检与测试读取最后一次废纸篓结果；生产代码只在检查参数下使用。
@MainActor
enum FileOpsOutcome {
    static var lastTrashResult: [String: String?]?
}
