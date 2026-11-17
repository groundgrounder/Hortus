#if DEBUG
import AppKit
import Foundation
import HortusCore

/// Changes 面板回滚自检：按处、按文件、按会话、stale、打开文档保护、
/// 快照清理与多窗口隔离。全部通过真实窗口的模型入口执行，不直接改磁盘绕过 UI。
@MainActor
enum RollbackInspection {
    private static func argument(_ name: String, in arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }.map { String($0.dropFirst(name.count + 3)) }
    }

    private static func require(_ value: Bool, _ message: String) async throws {
        guard value else {
            throw NSError(domain: "Hortus.RollbackInspection", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: message])
        }
    }

    private static func wait(
        seconds: Double,
        until description: String,
        _ condition: () -> Bool
    ) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else {
                throw NSError(domain: "Hortus.RollbackInspection", code: 2,
                              userInfo: [NSLocalizedDescriptionKey: "等待超时（\(seconds) s）：\(description)"])
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    private static func waitAgent(_ tab: TerminalTab, seconds: Double = 180) async throws {
        try await wait(seconds: seconds, until: "Agent 结束") {
            switch tab.agentPhase {
            case .exited, .failed: true
            default: false
            }
        }
        if case .failed(let message) = tab.agentPhase {
            throw NSError(domain: "Hortus.RollbackInspection", code: 3,
                          userInfo: [NSLocalizedDescriptionKey: "Agent 启动失败：\(message)"])
        }
    }

    private static func show(_ workspace: Workspace, _ layout: Layout) {
        workspace.showStaticTool(.changes, in: .right)
        _ = layout.reveal(.tool(.changes))
    }

    private static func prepare(
        _ workspace: Workspace, _ layout: Layout, session: String?
    ) async throws -> ChangesModel {
        show(workspace, layout)
        await workspace.changes.activate(preferred: session)
        try await wait(seconds: 120, until: "会话改动摘要") {
            workspace.changes.summary != nil && !workspace.changes.loadingSummary
        }
        return workspace.changes
    }

    private static func launch(
        _ workspace: Workspace, _ layout: Layout
    ) async throws -> (String, ChangesModel) {
        guard let tab = workspace.addAgent(to: layout.defaultTerminalGroupID) else {
            throw NSError(domain: "Hortus.RollbackInspection", code: 4,
                          userInfo: [NSLocalizedDescriptionKey: "无法建立 Agent 标签"])
        }
        try await waitAgent(tab)
        let sessionID = tab.agentSessionID ?? ""
        try await require(!sessionID.isEmpty, "Agent 会话缺少 ID")
        return (sessionID, try await prepare(workspace, layout, session: sessionID))
    }

    private static func summary(_ model: ChangesModel) throws -> SessionReviewSummary {
        guard let summary = model.summary else {
            throw NSError(domain: "Hortus.RollbackInspection", code: 5,
                          userInfo: [NSLocalizedDescriptionKey: "没有会话改动摘要"])
        }
        return summary
    }

    private static func entry(_ summary: SessionReviewSummary, _ id: String) throws -> ReviewEntryInfo {
        guard let entry = summary.entries.first(where: { $0.id == id }) else {
            throw NSError(domain: "Hortus.RollbackInspection", code: 6,
                          userInfo: [NSLocalizedDescriptionKey: "找不到改动 \(id)"])
        }
        return entry
    }

    private static func select(
        _ model: ChangesModel, entry id: String, seconds: Double = 60
    ) async throws {
        await model.selectEntry(id)
        try await wait(seconds: seconds, until: "条目详情 \(id)") {
            model.detail?.entry.id == id && !model.loadingDetail
        }
    }

    private static func url(_ workspace: Workspace, _ path: String) -> URL? {
        workspace.root.map { URL(fileURLWithPath: $0).appendingPathComponent(path) }
    }

    private static func fsData(_ workspace: Workspace, _ path: String) -> Data {
        guard let url = url(workspace, path) else { return Data() }
        return (try? Data(contentsOf: url)) ?? Data()
    }

    private static func fsText(_ workspace: Workspace, _ path: String) -> String {
        String(decoding: fsData(workspace, path), as: UTF8.self)
    }

    private static func fsWrite(_ workspace: Workspace, _ path: String, _ text: String) {
        guard let url = url(workspace, path) else { return }
        try? Data(text.utf8).write(to: url)
    }

    private static func exists(_ workspace: Workspace, _ path: String) -> Bool {
        guard let url = url(workspace, path) else { return false }
        return FileManager.default.fileExists(atPath: url.path)
    }

    private static func openDocument(
        _ workspace: Workspace, _ path: String, expected: String
    ) async throws -> NoteDocument {
        workspace.openNote(path)
        try await wait(seconds: 30, until: "打开 \(path)") {
            workspace.tabs.first(where: { $0.path == path })?.document != nil
        }
        guard let document = workspace.tabs.first(where: { $0.path == path })?.document else {
            throw CocoaError(.coderValueNotFound)
        }
        try await wait(seconds: 30, until: "\(path) 载入磁盘正文") { document.text == expected }
        return document
    }

    static func run(
        workspace: Workspace,
        layout: Layout,
        arguments: [String],
        mode: String
    ) async throws -> [String: Any] {
        switch mode {
        case "rollback-hunks":
            return try await hunks(workspace: workspace, layout: layout)
        case "rollback-files":
            return try await files(workspace: workspace, layout: layout)
        case "rollback-session":
            return try await session(workspace: workspace, layout: layout)
        case "rollback-cleanup":
            return try await cleanup(workspace: workspace, layout: layout)
        case "rollback-large":
            return try await large(workspace: workspace, layout: layout)
        case "rollback-two-windows":
            return try await twoWindows(workspace: workspace, layout: layout, arguments: arguments)
        case "rollback-idle":
            return try await idle(workspace: workspace, layout: layout)
        case "rollback-screenshot":
            return try await screenshot(workspace: workspace, layout: layout, result: false)
        case "rollback-screenshot-result":
            return try await screenshot(workspace: workspace, layout: layout, result: true)
        default:
            throw NSError(domain: "Hortus.RollbackInspection", code: 7,
                          userInfo: [NSLocalizedDescriptionKey: "未知回滚模式 \(mode)"])
        }
    }

    private static func hunks(workspace: Workspace, layout: Layout) async throws -> [String: Any] {
        let (sessionID, model) = try await launch(workspace, layout)
        let initial = try summary(model)
        try await select(model, entry: "Hunks.md")
        guard let detail = model.detail else { throw CocoaError(.coderValueNotFound) }
        try await require(detail.hunks.count == 2, "Hunks.md 应有两个独立 hunk")
        try await require(await model.rollbackHunk(detail.hunks[0], index: 0), "按处回滚失败")
        let bytes = fsData(workspace, "Hunks.md")
        let expected = "one\r\nTWO\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nCHANGED10"
        try await require(String(decoding: bytes, as: UTF8.self) == expected, "第 1 个 hunk 未精确回退")
        try await require(bytes.contains(13) && bytes.contains(10), "CRLF 未保留")
        try await require(bytes.last != 10, "无末尾换行状态未保留")

        try await wait(seconds: 60, until: "摘要刷新") {
            model.summary != nil && !model.loadingSummary
        }
        try await select(model, entry: "Hunks.md")
        try await require(model.detail?.hunks.count == 1, "其余 hunk 未保留")

        // 防静默覆盖：审阅后外部改写同文件，按处回滚必须拒绝且内容完好。
        try await select(model, entry: "Edit.md")
        guard let editHunk = model.detail?.hunks.first else { throw CocoaError(.coderValueNotFound) }
        let external = "alpha\nZETA\ngamma\nexternal\n"
        fsWrite(workspace, "Edit.md", external)
        let staleApplied = await model.rollbackHunk(editHunk, index: 0)
        try await require(!staleApplied, "stale hunk 被静默覆盖")
        let staleNotice = model.notice ?? ""
        try await require(
            staleNotice.lowercased().contains("stale") || staleNotice.lowercased().contains("disk changed"),
            "stale 提示不明确：\(staleNotice)"
        )
        try await require(fsText(workspace, "Edit.md") == external, "外部追加的正文被改写")

        // 干净已打开标签页：回滚后同步磁盘。
        try await select(model, entry: "OpenClean.md")
        let clean = try await openDocument(workspace, "OpenClean.md", expected: "agent changed clean\n")
        try await require(await model.rollbackSelectedEntry(), "干净标签页按文件回滚失败")
        try await wait(seconds: 30, until: "干净标签页刷新") { clean.text == "clean baseline\n" }
        try await require(!clean.dirty, "干净标签页刷新后变脏")

        // 未保存脏编辑 + 组合输入：回滚必须被阻止，正文与磁盘都保留。
        try await select(model, entry: "OpenDirty.md")
        let dirty = try await openDocument(workspace, "OpenDirty.md", expected: "agent changed dirty\n")
        dirty.edited(range: NSRange(location: 0, length: 0), replacement: "UNSAVED ")
        dirty.compositionChanged(true)
        try await require(dirty.dirty && dirty.marked, "脏编辑/组合输入状态未建立")
        let unsavedText = dirty.text
        let dirtyApplied = await model.rollbackSelectedEntry()
        try await require(!dirtyApplied, "脏编辑标签页被回滚覆盖")
        try await require(dirty.text == unsavedText && dirty.dirty, "未保存文字丢失")
        try await require(
            (model.notice ?? "").lowercased().contains("unsaved"),
            "未保存保护提示不明确：\(model.notice ?? "")"
        )
        try await require(fsText(workspace, "OpenDirty.md") == "agent changed dirty\n", "脏编辑保护期间磁盘被改写")

        return [
            "mode": "rollback-hunks",
            "session": sessionID,
            "initial_entries": initial.entries.count,
            "initial_hunks": detail.hunks.count,
            "remaining_hunks": model.summary?.entries.first(where: { $0.id == "Hunks.md" }) == nil ? 0 : 1,
            "stale_rejected": !staleApplied,
            "stale_notice": staleNotice,
            "open_clean_text": clean.text,
            "dirty_preserved": dirty.text == unsavedText,
        ]
    }

    private static func files(workspace: Workspace, layout: Layout) async throws -> [String: Any] {
        let (sessionID, model) = try await launch(workspace, layout)
        let initial = try summary(model)

        func revert(_ id: String) async throws {
            try await select(model, entry: id)
            try await require(await model.rollbackSelectedEntry(), "按文件回滚失败：\(id)")
        }

        try await revert("Edit.md")
        try await require(fsText(workspace, "Edit.md") == "alpha\nbeta\ngamma\n", "修改未还原")
        try await revert("Added/New note.md")
        try await revert("Added")
        try await require(!exists(workspace, "Added"), "新增目录未删除")
        try await revert("AddedZero.bin")
        try await require(!exists(workspace, "AddedZero.bin"), "0 字节新增未删除")
        try await revert(".hidden-added")
        try await require(!exists(workspace, ".hidden-added"), "隐藏新增未删除")
        try await revert("EmptyAdded")
        try await require(!exists(workspace, "EmptyAdded"), "新增空目录未删除")
        try await revert("Delete.md")
        try await require(fsText(workspace, "Delete.md") == "delete this\n", "删除未还原")
        try await revert("Renamed.md")
        try await require(fsText(workspace, "Rename.md") == "stable rename\n", "纯重命名未还原")
        try await require(!exists(workspace, "Renamed.md"), "重命名的新路径未删除")
        try await revert("Shifted.md")
        try await require(fsText(workspace, "Shift.md") == "one\ntwo\nthree\nfour\nfive\nsix\n", "重命名并编辑未还原")
        try await require(!exists(workspace, "Shifted.md"), "重命名并编辑的新路径未删除")
        // 类型变化目录内的新增文件先回滚，目录变空后才允许还原为文件。
        try await revert("Type.md/inside.md")
        try await revert("Type.md")
        let typeAfter = try FileManager.default.attributesOfItem(
            atPath: url(workspace, "Type.md")!.path
        )[.type] as? FileAttributeType
        try await require(typeAfter == FileAttributeType.typeRegular, "类型变化未还原为文件")
        try await require(fsText(workspace, "Type.md") == "type changes\n", "类型变化内容错误")
        try await revert("LinkEdit")
        try await require(
            FileManager.default.destinationOfSymbolicLink(atPath: url(workspace, "LinkEdit")!.path)
                .hasSuffix("Edit.md"),
            "符号链接目标未还原"
        )
        try await revert("Mode.md")
        let mode = (try FileManager.default.attributesOfItem(atPath: url(workspace, "Mode.md")!.path)[.posixPermissions] as? NSNumber)?.uint16Value ?? 0
        try await require(mode & 0o777 == 0o644, "权限未还原：\(String(mode, radix: 8))")
        try await revert("Assets/picture.png")
        let png = fsData(workspace, "Assets/picture.png")
        try await require(png.count == 256 && png.first == 0 && png.last == 255, "二进制附件未还原")

        let remaining = model.summary?.entries.map(\.id) ?? []
        return [
            "mode": "rollback-files",
            "session": sessionID,
            "initial_entries": initial.entries.count,
            "remaining": remaining,
            "png_bytes": png.count,
            "mode_after": String(mode & 0o777, radix: 8),
        ]
    }

    private static func session(workspace: Workspace, layout: Layout) async throws -> [String: Any] {
        let (sessionID, model) = try await launch(workspace, layout)
        let initial = try summary(model)
        let shifted = fsText(workspace, "Shifted.md")
        let external = shifted + "external\n"
        fsWrite(workspace, "Shifted.md", external)

        let applied = await model.rollbackAll()
        try await require(applied, "整会话回滚返回失败")
        guard let report = model.lastReport else { throw CocoaError(.coderValueNotFound) }
        try await require(report.skippedStale.count == 1, "stale 条目数错误：\(report.skippedStale.count)")
        try await require(report.failed.isEmpty, "出现未预期失败：\(report.failed)")
        try await require(report.revertedPaths.count == initial.entries.count - 1, "还原条目数错误")
        try await require(fsText(workspace, "Shifted.md") == external, "stale 外部内容被覆盖")
        try await require(fsText(workspace, "Type.md") == "type changes\n", "类型变化未按顺序还原")
        try await require(!exists(workspace, "Type.md/inside.md"), "类型变化子项残留")
        try await require(fsText(workspace, "Delete.md") == "delete this\n", "删除未还原")
        try await require(fsText(workspace, "Rename.md") == "stable rename\n", "重命名未还原")
        try await require(!exists(workspace, "Added"), "新增目录残留")
        try await require(fsText(workspace, "Edit.md") == "alpha\nbeta\ngamma\n", "修改未还原")
        try await require(fsText(workspace, "Hunks.md").hasPrefix("one\r\nTWO\r\n"), "按会话未还原 hunk 文件")
        let snapshot = model.selectedSession?.snapshotPath ?? ""
        try await require(FileManager.default.fileExists(atPath: snapshot), "部分回滚后快照被误删")
        let remaining = model.summary?.entries.map(\.id) ?? []
        try await require(remaining == ["Shifted.md"], "回滚后剩余条目错误：\(remaining)")
        return [
            "mode": "rollback-session",
            "session": sessionID,
            "initial_entries": initial.entries.count,
            "reverted": report.revertedPaths.count,
            "skipped_stale": report.skippedStale.count,
            "failed": report.failed.count,
            "remaining": remaining,
            "notice": model.notice ?? "",
        ]
    }

    private static func cleanup(workspace: Workspace, layout: Layout) async throws -> [String: Any] {
        let (sessionID, model) = try await launch(workspace, layout)
        guard let session = model.selectedSession else { throw CocoaError(.coderValueNotFound) }
        let snapshotDirectory = session.snapshotPath
        let lockedSource = url(workspace, "Locked.md")!
        let sourceBefore = fsData(workspace, "Locked.md")
        let sourceFlags = (try FileManager.default.attributesOfItem(atPath: lockedSource.path)[.immutable] as? NSNumber)?.boolValue ?? false

        let snapshotLocked = URL(fileURLWithPath: snapshotDirectory).appendingPathComponent("Locked.md")
        let snapshotMode = (try FileManager.default.attributesOfItem(atPath: snapshotLocked.path)[.posixPermissions] as? NSNumber)?.intValue ?? 0
        try await require(snapshotMode & 0o777 == 0o400, "快照未保留 0400：\(String(snapshotMode, radix: 8))")
        let snapshotReadOnly = URL(fileURLWithPath: snapshotDirectory).appendingPathComponent("ReadOnly.md")
        let readOnlyMode = (try FileManager.default.attributesOfItem(atPath: snapshotReadOnly.path)[.posixPermissions] as? NSNumber)?.intValue ?? 0
        try await require(readOnlyMode & 0o777 == 0o444, "快照未保留 0444")

        let deleted = await model.deleteSelectedSnapshot()
        try await require(deleted, "删除快照失败")
        try await require(!FileManager.default.fileExists(atPath: snapshotDirectory), "快照目录残留")
        try await require(!model.sessions.contains(where: { $0.id == sessionID }), "会话未从列表移除")
        try await require(fsData(workspace, "Locked.md") == sourceBefore, "用户源文件被改动")
        let flagsAfter = (try FileManager.default.attributesOfItem(atPath: lockedSource.path)[.immutable] as? NSNumber)?.boolValue ?? false
        try await require(flagsAfter == sourceFlags, "用户源文件 uchg 被改动")

        // 活动会话拒绝删除：引擎返回明确错误，目录保留。
        guard let core = workspace.core, let root = workspace.root else { throw CocoaError(.coderValueNotFound) }
        let running = try await Task.detached { try core.beginAgentSession(vault: root) }.value
        try core.markAgentSession(id: running.id, state: "running", exitCode: nil)
        var runningError = ""
        do {
            _ = try await Task.detached { try core.deleteAgentSession(id: running.id) }.value
        } catch {
            runningError = String(describing: error)
        }
        let runningDirectory = URL(fileURLWithPath: snapshotDirectory)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent(running.id)
        try await require(runningError.contains("history.session_active"), "活动会话未拒绝：\(runningError)")
        try await require(FileManager.default.fileExists(atPath: runningDirectory.path), "活动会话目录被删除")

        return [
            "mode": "rollback-cleanup",
            "session": sessionID,
            "snapshot_deleted": !FileManager.default.fileExists(atPath: snapshotDirectory),
            "source_flags": sourceFlags,
            "running_error": runningError,
        ]
    }

    private static func large(workspace: Workspace, layout: Layout) async throws -> [String: Any] {
        let (sessionID, model) = try await launch(workspace, layout)
        let initial = try summary(model)
        let gap = RollbackGapRecorder()
        gap.start()
        let applied = await model.rollbackAll()
        gap.stop()
        try await require(applied, "大库整会话回滚失败")
        guard let report = model.lastReport else { throw CocoaError(.coderValueNotFound) }
        try await require(report.failed.isEmpty, "大库回滚出现失败")
        try await require(!exists(workspace, "large-added.md"), "大库新增文件残留")
        let notes = url(workspace, "notes/0000.md").flatMap { try? Data(contentsOf: $0) } ?? Data()
        let text = String(decoding: notes, as: UTF8.self)
        try await require(!text.hasSuffix("large edit\n"), "大库修改未还原")
        return [
            "mode": "rollback-large",
            "session": sessionID,
            "entries": initial.entries.count,
            "reverted": report.revertedPaths.count,
            "max_main_gap_ms": gap.maxGapMilliseconds,
            "failed": report.failed.count,
        ]
    }

    private static func idle(workspace: Workspace, layout: Layout) async throws -> [String: Any] {
        let (sessionID, model) = try await launch(workspace, layout)
        let entries = try summary(model).entries.count
        return [
            "mode": "rollback-idle",
            "session": sessionID,
            "entries": entries,
        ]
    }

    private static func twoWindows(
        workspace: Workspace,
        layout: Layout,
        arguments: [String]
    ) async throws -> [String: Any] {
        guard let secondVault = argument("rollback-second-vault", in: arguments),
              let application = workspace.application,
              let core = workspace.core else {
            throw CocoaError(.coderValueNotFound)
        }
        let first = try await launch(workspace, layout)
        let firstEntries = try summary(first.1).entries.map(\.id)

        await workspace.openFolder(secondVault)
        guard let secondRecord = application.records.first(where: {
            $0.workspace !== workspace && $0.workspace.root != nil
        }) else {
            throw NSError(domain: "Hortus.RollbackInspection", code: 8,
                          userInfo: [NSLocalizedDescriptionKey: "第二个库没有打开"])
        }
        let secondWorkspace = secondRecord.workspace
        application.activate(secondRecord)
        guard let secondRoot = secondWorkspace.root else { throw CocoaError(.coderValueNotFound) }
        let secondSession = try await Task.detached {
            try core.beginAgentSession(vault: secondRoot)
        }.value
        fsWrite(secondWorkspace, "SecondOnly.md", "# Second only\n")
        let secondModel = try await prepare(secondWorkspace, secondRecord.layout, session: secondSession.id)
        let secondEntries = try summary(secondModel).entries.map(\.id)
        try await require(secondEntries == ["SecondOnly.md"], "第二窗口改动错误")
        try await require(await secondModel.rollbackAll(), "第二窗口回滚失败")
        try await require(!exists(secondWorkspace, "SecondOnly.md"), "第二窗口新增未删除")
        try await require(fsText(secondWorkspace, "SecondOnly.md") == "", "第二窗口删除失败")
        try await require(
            first.1.summary?.entries.map(\.id) == firstEntries,
            "第一窗口的改动被第二窗口回滚"
        )
        guard let firstRecord = application.records.first(where: { $0.workspace === workspace }) else {
            throw CocoaError(.coderValueNotFound)
        }
        application.activate(firstRecord)
        return [
            "mode": "rollback-two-windows",
            "first_root": workspace.root ?? "",
            "second_root": secondRoot,
            "first_entries": firstEntries.count,
            "second_entries": secondEntries.count,
            "isolated": true,
        ]
    }

    private static func screenshot(
        workspace: Workspace,
        layout: Layout,
        result: Bool
    ) async throws -> [String: Any] {
        let (sessionID, model) = try await launch(workspace, layout)
        if result {
            try await require(await model.rollbackAll(), "截图用整会话回滚失败")
        } else {
            try await select(model, entry: "Hunks.md")
        }
        layout.visible[.bottom] = false
        try await Task.sleep(for: .milliseconds(400))
        return [
            "mode": result ? "rollback-screenshot-result" : "rollback-screenshot",
            "session": sessionID,
            "entries": model.summary?.entries.count ?? 0,
            "selected": model.selectedEntryID ?? "",
            "notice": model.notice ?? "",
            "title": L10n.string("Session Changes"),
        ]
    }
}

/// 主线程响应间隔：回滚在后台执行时不应阻塞界面。
@MainActor
private final class RollbackGapRecorder {
    private var task: Task<Void, Never>?
    private var last = 0.0
    private(set) var maxGapMilliseconds = 0.0

    func start() {
        last = ProcessInfo.processInfo.systemUptime
        task = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(5))
                guard let self else { return }
                let now = ProcessInfo.processInfo.systemUptime
                self.maxGapMilliseconds = max(self.maxGapMilliseconds, (now - self.last) * 1000)
                self.last = now
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
    }
}
#endif
