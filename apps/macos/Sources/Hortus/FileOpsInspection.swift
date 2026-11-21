#if DEBUG
import AppKit
import HortusCore

/// 显式检查参数下验证文件管理闭环：新建、跨窗口重命名迁移、移到废纸篓与安全边界。
/// 只在 DEBUG 构建编译，不进入 Release 二进制。
@MainActor
enum FileOpsInspection {
    private static func require<T>(_ value: T?) throws -> T {
        guard let value else { throw CocoaError(.coderValueNotFound) }
        return value
    }

    private static func wait(_ condition: @MainActor () -> Bool) async throws {
        try await wait("condition", condition)
    }

    /// 带标签的等待：超时错误直接写出卡住的检查项，便于从自检日志定位。
    private static func wait(_ label: String, _ condition: @MainActor () -> Bool) async throws {
        for _ in 0..<600 {
            if condition() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        throw NSError(domain: "HortusInspection", code: 1,
                      userInfo: [NSLocalizedDescriptionKey: "timeout waiting for \(label)"])
    }

    private static func settled(_ application: ApplicationWorkspace) async throws {
        try await wait {
            application.records.allSatisfy {
                $0.window != nil && !$0.workspace.scanning && !$0.workspace.tabs.contains(where: \.loading)
            }
        }
        try await Task.sleep(for: .milliseconds(150))
    }

    private static func errorKey(_ error: Error) -> String {
        if let error = error as? CoreError, case let .Failure(kind) = error { return kind }
        return "unknown"
    }

    private static func failureKey(_ operation: () throws -> Any) -> String {
        do { _ = try operation(); return "unexpected-success" }
        catch { return errorKey(error) }
    }

    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        let application = try require(workspace.application)
        let context = try require(workspace.context)
        let handle = context.handle
        let root = try require(workspace.root)
        var result: [String: Any] = [:]
        try await settled(application)
        let record = try require(application.records.first { $0.workspace === workspace })
        let outline = try require(outlineView(of: record))
        let coordinator = try require(outline.delegate as? FileTree.Coordinator)

        // 1. 新建笔记：自动补 .md、直接进入编辑视图并可写入保存。
        let created = try await workspace.createNote(parent: "", name: "Created Note")
        let createdPath = root + "/Created Note.md"
        try await wait { workspace.current?.document?.reading == false && workspace.selected == created.path }
        let createdDocument = try require(workspace.current?.document)
        createdDocument.edited(range: NSRange(location: 0, length: 0), replacement: "Inspection draft")
        await createdDocument.saveNow()
        try await wait("new note tree name") { treeName(outline, coordinator: coordinator, path: created.path) == created.name }
        result["create_note"] = [
            "path": created.path,
            "disk": FileManager.default.fileExists(atPath: createdPath),
            "editing_view": !createdDocument.reading,
            "saved": (try? String(contentsOfFile: createdPath, encoding: .utf8)) == "Inspection draft",
            "tree_name": treeName(outline, coordinator: coordinator, path: created.path) == created.name,
        ] as [String: Any]

        // 2. 新建文件夹：库根与子目录各一例，树缓存立即更新并选中。
        let projects = try await workspace.createFolder(parent: "", name: "Projects")
        // memo 的打开会清空 folderSelection，选中状态须先快照。
        let folderSelected = workspace.folderSelection == projects.path
        try await wait("new folder tree node") { coordinator.nodes[projects.path] != nil }
        let projectsNode = try require(coordinator.nodes[projects.path])
        try await wait("new folder tree row") { outline.row(forItem: projectsNode) >= 0 }
        // 点选既有文件夹：新建目标跟随文件夹；取消选中清空目标；再选回后新建落入该文件夹。
        outline.selectRowIndexes(IndexSet(integer: outline.row(forItem: projectsNode)), byExtendingSelection: false)
        try await wait("folder selection") { workspace.folderSelection == projects.path }
        let selectionSync = workspace.folderSelection == projects.path
        let treeParent = workspace.parentForNewEntry()
        outline.deselectAll(nil)
        try await wait("folder selection cleared") { workspace.folderSelection == nil }
        let clearedOnDeselect = workspace.folderSelection == nil
        outline.selectRowIndexes(IndexSet(integer: outline.row(forItem: projectsNode)), byExtendingSelection: false)
        try await wait("folder selection") { workspace.folderSelection == projects.path }
        let picked = try await workspace.createNote(parent: workspace.parentForNewEntry(), name: "picked")
        result["folder_selection"] = [
            "folder": projects.path,
            "selected": selectionSync,
            "parent": treeParent,
            "cleared_on_deselect": clearedOnDeselect,
            "note_path": picked.path,
            "disk": FileManager.default.fileExists(atPath: root + "/Projects/picked.md"),
        ] as [String: Any]
        _ = try await workspace.createFolder(parent: "Projects", name: "Nested")
        let memo = try await workspace.createNote(parent: "Projects", name: "memo")
        result["create_folder"] = [
            "path": projects.path,
            "disk": FileManager.default.fileExists(atPath: root + "/Projects"),
            "selected": folderSelected,
            "nested_disk": FileManager.default.fileExists(atPath: root + "/Projects/Nested"),
        ] as [String: Any]
        try await wait { workspace.current?.document?.reading == false && workspace.selected == memo.path }
        let memoInFirst = try require(workspace.current?.document)
        result["create_note_subfolder"] = [
            "path": memo.path,
            "disk": FileManager.default.fileExists(atPath: root + "/Projects/memo.md"),
            "editing_view": !memoInFirst.reading,
        ] as [String: Any]

        // 3. 同库第二窗口：为跨窗口重命名迁移做准备。
        let second = WindowRecord(layout: application.makeLayout())
        application.configure(second)
        application.records.append(second)
        await second.workspace.attach(context, state: nil)
        application.showWindow?(second.id)
        try await settled(application)
        result["second_window_attached"] = second.window != nil && second.workspace.root == root

        // 4. 重命名含未保存改动的打开笔记：全部同库窗口的标签同步迁移。
        workspace.openNote("alpha.md")
        try await wait { workspace.current?.document != nil && !(workspace.current?.loading ?? true) }
        let alpha = try require(workspace.current?.document)
        alpha.edited(range: NSRange(location: 0, length: 0), replacement: "草稿 ")
        _ = try await alpha.io.perform { try $0.snapshot() }
        await alpha.checkpoint()
        let recovery = URL(fileURLWithPath: try require(ProcessInfo.processInfo.environment["HORTUS_CACHE_DIR"]))
            .appendingPathComponent("recovery")
        func recoveryCount() -> Int {
            (try? FileManager.default.contentsOfDirectory(atPath: recovery.path))?.count ?? -1
        }
        let beforeRename = recoveryCount()
        let renamed = try await second.workspace.renameEntry(path: "alpha.md", newName: "renamed.md")
        // 重命名后的迁移必须立即反映在标签路径、选中、文件树缓存与显示名上。
        let immediate = workspace.selected == renamed.path
            && workspace.tabs.contains { $0.path == renamed.path }
            && context.entries["renamed.md"] != nil && context.entries["alpha.md"] == nil
        let immediateName = context.entries[renamed.path]?.name == renamed.name
            && context.entries[renamed.path]?.path == renamed.path
        try await wait("renamed note tree name") { treeName(outline, coordinator: coordinator, path: renamed.path) == renamed.name }
        let treeNameAfterRename = treeName(outline, coordinator: coordinator, path: renamed.path)
        try await wait { workspace.current?.document?.io.url.lastPathComponent == "renamed.md" }
        let migrated = try require(workspace.current?.document)
        let dirtyPreserved = migrated.dirty
        // 恢复稿随重命名迁移：保存前仍应有一条，保存后才清理。
        let recoveryAfterRename = recoveryCount()
        await migrated.saveNow()
        let afterSave = try await migrated.io.perform { try $0.refresh() }
        result["rename_file_cross_window"] = [
            "path": renamed.path,
            "immediate": immediate,
            "immediate_name": immediateName,
            "tree_name": treeNameAfterRename == renamed.name,
            "selected": workspace.selected == "renamed.md",
            "title": workspace.title,
            "dirty_preserved": dirtyPreserved,
            "no_missing_or_conflict": afterSave.issue != .missing && afterSave.issue != .conflict,
            "saved_to_new": (try? String(contentsOfFile: root + "/renamed.md", encoding: .utf8)) == "草稿 # Alpha\n",
            "recovery_before": beforeRename,
            "recovery_after_rename": recoveryAfterRename,
            "recovery_after_save": recoveryCount(),
        ] as [String: Any]

        // 5. 仅改大小写的原地改名必须成功。
        let caseRenamed = try await workspace.renameEntry(path: "case.md", newName: "Case.md")
        let caseNameImmediate = context.entries[caseRenamed.path]?.name == caseRenamed.name
        try await wait("case-only tree name") { treeName(outline, coordinator: coordinator, path: caseRenamed.path) == caseRenamed.name }
        result["rename_case_only"] = [
            "path": caseRenamed.path,
            // 大小写不敏感文件系统上 fileExists 无法区分新旧名，按目录实际条目核对。
            "disk": ((try? FileManager.default.contentsOfDirectory(atPath: root)) ?? [])
                .contains("Case.md") && !((try? FileManager.default.contentsOfDirectory(atPath: root)) ?? [])
                .contains("case.md"),
            "immediate": context.entries["Case.md"] != nil && context.entries["case.md"] == nil,
            "immediate_name": caseNameImmediate,
            "tree_name": treeName(outline, coordinator: coordinator, path: caseRenamed.path) == caseRenamed.name,
        ] as [String: Any]

        // 6. 同目录与不同 inode 的既有文件同名必须拒绝，磁盘不动。
        let beforeConflict = (try? String(contentsOfFile: root + "/a.md", encoding: .utf8)) ?? ""
        let conflictKey = failureKey { _ = try handle.renameEntry(path: "a.md", newName: "B.md") }
        result["rename_conflict"] = [
            "error_key": conflictKey,
            "disk_unchanged": (try? String(contentsOfFile: root + "/a.md", encoding: .utf8)) == beforeConflict
                && FileManager.default.fileExists(atPath: root + "/b.md"),
        ] as [String: Any]

        // 7. 重命名含打开笔记的非空文件夹：跨窗口子路径同步迁移。
        // 同库文档唯一：memo 已在第一窗口打开；第二窗口打开另一篇子笔记。
        let journal = try await second.workspace.createNote(parent: "Projects", name: "journal")
        try await wait { second.workspace.current?.document?.reading == false && second.workspace.selected == journal.path }
        let journalInSecond = try require(second.workspace.current?.document)
        journalInSecond.edited(range: NSRange(location: 0, length: 0), replacement: "子目录草稿 ")
        _ = try await journalInSecond.io.perform { try $0.snapshot() }
        let papers = try await workspace.renameEntry(path: "Projects", newName: "Papers")
        let childNames = context.entries["Papers"]?.name == "Papers"
            && context.entries["Papers/Nested"]?.name == "Nested"
            && context.entries["Papers/Nested"]?.parent == "Papers"
            && context.entries["Papers/memo.md"]?.name == "memo.md"
            && context.entries["Papers/memo.md"]?.parent == "Papers"
            && context.entries["Papers/journal.md"]?.name == "journal.md"
            && context.entries["Papers/journal.md"]?.parent == "Papers"
        try await wait("renamed folder tree name") { treeName(outline, coordinator: coordinator, path: "Papers") == "Papers" }
        try await wait {
            second.workspace.current?.path == "Papers/journal.md"
                && second.workspace.current?.document?.io.url.path.hasSuffix("Papers/journal.md") == true
        }
        let migratedJournal = try require(second.workspace.current?.document)
        let journalDirty = migratedJournal.dirty
        await migratedJournal.saveNow()
        try await wait { (handle.quickOpen(query: "journal", limit: 10).map(\.path)).contains("Papers/journal.md") }
        result["rename_folder_cross_window"] = [
            "path": papers.path,
            "tab_second": second.workspace.selected == "Papers/journal.md",
            "dirty_preserved": journalDirty,
            "saved_to_new": (try? String(contentsOfFile: root + "/Papers/journal.md", encoding: .utf8)) == "子目录草稿 ",
            "no_missing": migratedJournal.issue != .missing && migratedJournal.issue != .conflict,
            "quick_open_hit": (handle.quickOpen(query: "journal", limit: 10).map(\.path)).contains("Papers/journal.md"),
            "entries_subtree": context.entries["Papers/Nested"] != nil && context.entries["Projects/Nested"] == nil,
            "children_names": childNames,
            "tree_name": treeName(outline, coordinator: coordinator, path: "Papers") == "Papers",
            "first_window_tab": workspace.tabs.contains { $0.path == "Papers/memo.md" },
        ] as [String: Any]

        // 8. 安全边界：非法名称、符号链接与越界一律拒绝。
        var invalid: [String: String] = [:]
        for name in ["", ".hidden", "node_modules", "a/b", ".."] {
            invalid[name] = failureKey { _ = try handle.createNote(parent: "", name: name) }
        }
        result["invalid_names"] = invalid
        result["symlink_rejects"] = [
            "create": failureKey { _ = try handle.createNote(parent: "linkdir", name: "x") },
            "rename": failureKey { _ = try handle.renameEntry(path: "link.md", newName: "moved.md") },
            "trash": failureKey { _ = try handle.resolveTrashTarget(path: "link.md") },
            "outside": failureKey { _ = try handle.resolveTrashTarget(path: "../outside.md") },
        ] as [String: String]

        // 9. 移到废纸篓与未保存保护：取消后文件、标签与未保存正文完好。
        var prompts = 0
        workspace.openNote("dup.md")
        try await wait { workspace.current?.document != nil && !(workspace.current?.loading ?? true) }
        let dup = try require(workspace.current?.document)
        dup.edited(range: NSRange(location: 0, length: 0), replacement: "未存草稿 ")
        _ = try await dup.io.perform { try $0.snapshot() }
        workspace.promptTrash = { _, _ in prompts += 1; return false }
        workspace.requestTrash(path: "dup.md")
        let cancelSnapshot = try await dup.io.perform { try $0.snapshot() }
        result["trash_cancel_unsaved"] = [
            "prompts": prompts,
            "tab_intact": workspace.selected == "dup.md",
            "file_intact": FileManager.default.fileExists(atPath: root + "/dup.md"),
            "unsaved_intact": cancelSnapshot.text.hasPrefix("未存草稿 "),
        ] as [String: Any]

        // 10. 确认移到废纸篓（单文件）：标签关闭、恢复稿清理、树与索引同步。
        FileOpsOutcome.lastTrashResult = nil
        workspace.promptTrash = { _, _ in prompts += 1; return true }
        workspace.requestTrash(path: "dup.md")
        try await wait { (FileOpsOutcome.lastTrashResult?["resulting"]) ?? nil != nil }
        let fileTrash = (FileOpsOutcome.lastTrashResult?["resulting"]) ?? nil
        try await wait { !FileManager.default.fileExists(atPath: root + "/dup.md") }
        result["trash_file_confirm"] = [
            "disk_removed": !FileManager.default.fileExists(atPath: root + "/dup.md"),
            "tab_closed": !workspace.tabs.contains { $0.path == "dup.md" },
            "entry_removed": context.entries["dup.md"] == nil,
            "quick_open_gone": !(handle.quickOpen(query: "dup", limit: 10).map(\.path)).contains("dup.md"),
            "resulting": fileTrash ?? "",
            "resulting_exists": fileTrash.map { FileManager.default.fileExists(atPath: $0) } == true,
            "recovery_clean": recoveryCount() == 0,
        ] as [String: Any]

        // 11. ⌘⌫ 触发的文件夹移到废纸篓：真实键盘路径 + 跨窗口关闭子笔记标签。
        FileOpsOutcome.lastTrashResult = nil
        try await wait { coordinator.nodes["Papers"].map { outline.row(forItem: $0) >= 0 } == true }
        let row = outline.row(forItem: coordinator.nodes["Papers"]!)
        _ = try require(record.window?.makeFirstResponder(outline))
        outline.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        let keyEvent = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.command],
                                        timestamp: ProcessInfo.processInfo.systemUptime,
                                        windowNumber: record.window?.windowNumber ?? 0, context: nil,
                                        characters: "\u{7f}", charactersIgnoringModifiers: "\u{7f}",
                                        isARepeat: false, keyCode: 51)
        let performed = keyEvent.map { outline.performKeyEquivalent(with: $0) } == true
        try await wait { (FileOpsOutcome.lastTrashResult?["resulting"]) ?? nil != nil }
        let folderTrash = (FileOpsOutcome.lastTrashResult?["resulting"]) ?? nil
        try await wait { !FileManager.default.fileExists(atPath: root + "/Papers") }
        result["keyboard_trash_folder"] = [
            "performed": performed,
            "disk_removed": !FileManager.default.fileExists(atPath: root + "/Papers"),
            "first_window_tab_closed": !workspace.tabs.contains { $0.path == "Papers/memo.md" },
            "cross_window_tab_closed": !second.workspace.tabs.contains { $0.path == "Papers/journal.md" },
            "entry_removed": context.entries["Papers/memo.md"] == nil && context.entries["Papers"] == nil,
            "resulting": folderTrash ?? "",
            "resulting_exists": folderTrash.map { FileManager.default.fileExists(atPath: $0) } == true,
        ] as [String: Any]
        workspace.promptTrash = nil

        // 12. 双语截图：主窗口（含面板新建按钮）与命名弹窗。
        if let mediaArgument = arguments.first(where: { $0.hasPrefix("--file-ops-media=") }) {
            let media = URL(fileURLWithPath: String(mediaArgument.dropFirst("--file-ops-media=".count)))
            let window = try require(record.window)
            try await AppDelegate.snapshot(to: media.appendingPathComponent("file-ops-window.png").path, window: window)
            result["screenshot_window"] = media.appendingPathComponent("file-ops-window.png").path
            // 命名弹窗真实呈现后再截图，随后立即收起。
            let alert = NSAlert()
            alert.messageText = L10n.string("New Note")
            let field = NSTextField(string: workspace.suggestedName(base: L10n.string("New Note"), ext: "md", parent: ""))
            field.frame = NSRect(x: 0, y: 0, width: 264, height: 24)
            alert.accessoryView = field
            alert.addButton(withTitle: L10n.string("Create"))
            alert.addButton(withTitle: L10n.string("Cancel"))
            alert.window.initialFirstResponder = field
            alert.beginSheetModal(for: window) { _ in }
            try await Task.sleep(for: .milliseconds(400))
            try await AppDelegate.snapshot(to: media.appendingPathComponent("file-ops-naming.png").path, window: window)
            result["screenshot_naming"] = media.appendingPathComponent("file-ops-naming.png").path
            window.endSheet(alert.window)
        }
        return result
    }

    private static func treeName(_ outline: NSOutlineView, coordinator: FileTree.Coordinator,
                                 path: String) -> String? {
        guard let node = coordinator.nodes[path] else { return nil }
        let row = outline.row(forItem: node)
        guard row >= 0 else { return nil }
        let cell = outline.view(atColumn: 0, row: row, makeIfNecessary: true) as? NSTableCellView
        return cell?.textField?.stringValue
    }

    private static func outlineView(of record: WindowRecord) -> DrawingOutlineView? {
        guard let content = record.window?.contentView else { return nil }
        return findOutline(content)
    }
    private static func findOutline(_ view: NSView) -> DrawingOutlineView? {
        if let outline = view as? DrawingOutlineView { return outline }
        return view.subviews.lazy.compactMap { findOutline($0) }.first
    }
}
#endif
