#if DEBUG
import Foundation
import HortusCore

/// Changes 面板真实窗口自检：只读审阅、逐行 diff、二进制/符号链接元数据、
/// Preparing 隔离、重启后历史会话与双库多窗口。
@MainActor
enum ReviewInspection {
    private static func argument(_ name: String, in arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }.map { String($0.dropFirst(name.count + 3)) }
    }

    private static func wait(
        seconds: Double,
        until description: String,
        _ condition: () -> Bool
    ) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else {
                throw NSError(domain: "Hortus.ReviewInspection", code: 1,
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
            throw NSError(domain: "Hortus.ReviewInspection", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: "Agent 启动失败：\(message)"])
        }
    }

    private static func require(_ value: Bool, _ message: String) async throws {
        guard value else {
            throw NSError(domain: "Hortus.ReviewInspection", code: 3,
                          userInfo: [NSLocalizedDescriptionKey: message])
        }
    }

    private static func show(_ workspace: Workspace, _ layout: Layout) {
        workspace.showStaticTool(.changes, in: .right)
        _ = layout.reveal(.tool(.changes))
    }

    private static func waitSummary(_ model: ChangesModel, seconds: Double = 120) async throws {
        try await wait(seconds: seconds, until: "会话改动摘要") {
            model.summary != nil && !model.loadingSummary
        }
    }

    private static func select(
        _ model: ChangesModel, entry id: String, seconds: Double = 60
    ) async throws {
        await model.selectEntry(id)
        try await wait(seconds: seconds, until: "条目详情 \(id)") {
            model.detail?.entry.id == id && !model.loadingDetail
        }
    }

    private static func prepare(
        _ workspace: Workspace, _ layout: Layout, session: String?
    ) async throws -> ChangesModel {
        show(workspace, layout)
        await workspace.changes.activate(preferred: session)
        try await waitSummary(workspace.changes)
        return workspace.changes
    }

    private static func session(
        _ model: ChangesModel, _ id: String
    ) throws -> AgentSessionInfo {
        guard let session = model.sessions.first(where: { $0.id == id }) else {
            throw NSError(domain: "Hortus.ReviewInspection", code: 4,
                          userInfo: [NSLocalizedDescriptionKey: "找不到会话 \(id)"])
        }
        return session
    }

    private static func entry(
        _ summary: SessionReviewSummary, _ id: String
    ) throws -> ReviewEntryInfo {
        guard let entry = summary.entries.first(where: { $0.id == id }) else {
            throw NSError(domain: "Hortus.ReviewInspection", code: 5,
                          userInfo: [NSLocalizedDescriptionKey: "找不到改动 \(id)"])
        }
        return entry
    }

    static func run(
        workspace: Workspace,
        layout: Layout,
        arguments: [String],
        mode: String
    ) async throws -> [String: Any] {
        switch mode {
        case "review-changes":
            return try await changes(workspace: workspace, layout: layout, arguments: arguments)
        case "review-history":
            return try await history(workspace: workspace, layout: layout, arguments: arguments)
        case "review-two-windows":
            return try await twoWindows(workspace: workspace, layout: layout, arguments: arguments)
        case "review-large":
            return try await large(workspace: workspace, layout: layout, arguments: arguments)
        case "review-screenshot":
            return try await screenshot(workspace: workspace, layout: layout, arguments: arguments)
        default:
            throw NSError(domain: "Hortus.ReviewInspection", code: 6,
                          userInfo: [NSLocalizedDescriptionKey: "未知审阅模式 \(mode)"])
        }
    }

    private static func changes(
        workspace: Workspace,
        layout: Layout,
        arguments: [String]
    ) async throws -> [String: Any] {
        show(workspace, layout)
        guard let tab = workspace.addAgent(to: layout.defaultTerminalGroupID) else {
            throw NSError(domain: "Hortus.ReviewInspection", code: 7,
                          userInfo: [NSLocalizedDescriptionKey: "无法建立 Agent 标签"])
        }
        try await waitAgent(tab)
        let sessionID = tab.agentSessionID ?? ""
        try await require(!sessionID.isEmpty, "Agent 会话缺少 ID")
        let model = try await prepare(workspace, layout, session: sessionID)
        try await require(model.lastSummaryMainThread == false, "摘要不在后台线程执行")
        let summary = try requireSummary(model)

        var result: [String: Any] = [
            "mode": "review-changes",
            "session": sessionID,
            "state": model.selectedSession?.state ?? "",
            "exit_code": Int(model.selectedSession?.exitCode ?? -1),
            "counts": [
                "added": summary.counts.added,
                "modified": summary.counts.modified,
                "deleted": summary.counts.deleted,
                "renamed": summary.counts.renamed,
                "type_changed": summary.counts.typeChanged,
            ],
            "summary_ms": summary.elapsedMs,
            "summary_main_thread": model.lastSummaryMainThread ?? true,
            "entries": summary.entries.map(\.id),
        ]

        let required = [
            "Added/New note.md", "Edit.md", "Renamed.md", "Shifted.md", "Delete.md",
            "Assets/added.png", "Assets/picture.png", "Mode.md", "Type.md",
            "AddedZero.bin", ".hidden-added", "EmptyAdded", "LinkEdit", "LinkNew",
        ]
        for id in required {
            _ = try entry(summary, id)
        }
        try await require(!summary.entries.contains { $0.id.contains(".git") }, ".git 内容进入改动")
        try await require(!summary.entries.contains { $0.id == "Keep.md" }, "未变化文件进入改动")
        let renamed = try entry(summary, "Renamed.md")
        try await require(renamed.change == "renamed" && renamed.oldPath == "Rename.md",
                          "纯重命名识别错误")
        try await require(renamed.contentIdentical, "纯重命名内容不一致")
        let shifted = try entry(summary, "Shifted.md")
        try await require(shifted.change == "renamed" && !shifted.contentIdentical,
                          "重命名并编辑识别错误")
        let typeChanged = try entry(summary, "Type.md")
        try await require(typeChanged.change == "type_changed", "类型变化识别错误")
        let modeEntry = try entry(summary, "Mode.md")
        try await require(modeEntry.change == "modified" && modeEntry.contentIdentical,
                          "权限变化识别错误")
        result["required"] = required.count

        try await select(model, entry: "Edit.md")
        guard let textDetail = model.detail else { throw CocoaError(.coderValueNotFound) }
        let textLines = textDetail.hunks.flatMap(\.lines)
        try await require(textDetail.text && !textDetail.binary, "文本条目被当成二进制")
        try await require(
            textLines.contains { $0.kind == "removed" && $0.text == "beta" && $0.oldLine == 2 },
            "逐行 diff 缺少删除行与旧行号"
        )
        try await require(
            textLines.contains { $0.kind == "added" && $0.text == "BETA" && $0.newLine == 2 },
            "逐行 diff 缺少新增行与新行号"
        )
        result["text_hunks"] = textDetail.hunks.count
        result["text_lines"] = textLines.count

        try await select(model, entry: "Assets/picture.png")
        guard let binary = model.detail else { throw CocoaError(.coderValueNotFound) }
        try await require(!binary.text && binary.binary, "PNG 未按二进制处理")
        try await require(binary.entry.before?.size ?? 0 > 0, "二进制缺少前元数据")
        try await require(binary.entry.after?.size ?? 0 > 0, "二进制缺少后元数据")
        result["binary_sizes"] = [binary.entry.before?.size ?? 0, binary.entry.after?.size ?? 0]

        try await select(model, entry: "LinkEdit")
        guard let link = model.detail else { throw CocoaError(.coderValueNotFound) }
        try await require(link.entry.before?.link == "Edit.md", "符号链接前目标错误")
        try await require(link.entry.after?.link == "Keep.md", "符号链接后目标错误")
        result["link_targets"] = [link.entry.before?.link ?? "", link.entry.after?.link ?? ""]

        try await select(model, entry: "EmptyAdded")
        guard let folder = model.detail else { throw CocoaError(.coderValueNotFound) }
        try await require(folder.entry.after?.kind == "directory", "空目录未识别")

        // 打开笔记只产生一个同库标签；再次点击不重复打开。
        try await select(model, entry: "Added/New note.md")
        try await require(model.canOpenSelectedNote, "新增笔记没有打开入口")
        model.openSelectedNote()
        try await wait(seconds: 20, until: "新增笔记打开") {
            workspace.tabs.contains { $0.path == "Added/New note.md" }
        }
        let firstOpenCount = workspace.tabs.filter { $0.path == "Added/New note.md" }.count
        model.openSelectedNote()
        try await Task.sleep(for: .milliseconds(300))
        let secondOpenCount = workspace.tabs.filter { $0.path == "Added/New note.md" }.count
        try await require(firstOpenCount == 1 && secondOpenCount == 1, "打开笔记重复创建标签")
        result["open_note_tabs"] = secondOpenCount

        // Preparing 会话：写入一个未完成记录，只显示状态，不进入审阅。
        guard let core = workspace.core else { throw CocoaError(.coderValueNotFound) }
        let preparingID = "99999999-9999-4999-8999-999999999999"
        let dataRoot = URL(fileURLWithPath: core.dataDirectory())
            .appendingPathComponent("agent-sessions")
            .appendingPathComponent(preparingID)
        let snapshot = dataRoot.appendingPathComponent("snapshot")
        try FileManager.default.createDirectory(at: snapshot, withIntermediateDirectories: true)
        let record: [String: Any] = [
            "format": 1,
            "id": preparingID,
            "vault": workspace.root ?? "",
            "created_at_ms": 1,
            "state": "preparing",
            "snapshot": "snapshot",
        ]
        try JSONSerialization.data(withJSONObject: record)
            .write(to: dataRoot.appendingPathComponent("session.json"))
        defer { try? FileManager.default.removeItem(at: dataRoot) }
        await model.refreshSessions(preferred: sessionID)
        guard let preparing = model.sessions.first(where: { $0.id == preparingID }) else {
            throw NSError(domain: "Hortus.ReviewInspection", code: 8,
                          userInfo: [NSLocalizedDescriptionKey: "没有 Preparing 会话"])
        }
        try await require(preparing.state == "preparing", "Preparing 状态未保留")
        try await require(!preparing.usable, "Preparing 会话被标为可审阅")
        await model.select(sessionID: preparing.id)
        try await Task.sleep(for: .milliseconds(150))
        try await require(model.summary == nil, "Preparing 会话伪造了审阅结果")
        await model.select(sessionID: sessionID)
        try await waitSummary(model)
        result["preparing_isolated"] = true
        let restored = try session(model, sessionID)
        result["session_state"] = restored.state
        result["session_exit_code"] = Int(restored.exitCode ?? -1)
        result["snapshot_at_ms"] = restored.snapshotAtMs
        result["after_preparing_entries"] = model.summary?.entries.count ?? 0
        return result
    }

    private static func requireSummary(_ model: ChangesModel) throws -> SessionReviewSummary {
        guard let summary = model.summary else {
            throw NSError(domain: "Hortus.ReviewInspection", code: 9,
                          userInfo: [NSLocalizedDescriptionKey: "没有会话改动摘要"])
        }
        return summary
    }

    private static func history(
        workspace: Workspace,
        layout: Layout,
        arguments: [String]
    ) async throws -> [String: Any] {
        let sessionID = argument("review-session", in: arguments)
        let model = try await prepare(workspace, layout, session: sessionID)
        let summary = try requireSummary(model)
        try await require(!model.sessions.isEmpty, "重启后没有历史会话")
        try await require(model.selectedSession?.usable == true, "历史会话不可审阅")
        try await require(!summary.entries.isEmpty, "历史会话没有改动")
        resultChecks(summary)
        return [
            "mode": "review-history",
            "sessions": model.sessions.count,
            "selected": model.selectedSessionID ?? "",
            "state": model.selectedSession?.state ?? "",
            "entries": summary.entries.count,
            "summary_main_thread": model.lastSummaryMainThread ?? true,
        ]
    }

    private static func twoWindows(
        workspace: Workspace,
        layout: Layout,
        arguments: [String]
    ) async throws -> [String: Any] {
        guard let secondVault = argument("review-second-vault", in: arguments),
              let application = workspace.application,
              let firstRecord = application.records.first(where: { $0.workspace === workspace }),
              let core = workspace.core else {
            throw CocoaError(.coderValueNotFound)
        }
        let firstSession = argument("review-session", in: arguments)
        let firstModel = try await prepare(workspace, layout, session: firstSession)
        let firstSummary = try requireSummary(firstModel)

        await workspace.openFolder(secondVault)
        guard let secondRecord = application.records.first(where: {
            $0.workspace !== workspace && $0.workspace.root != nil
        }) else {
            throw NSError(domain: "Hortus.ReviewInspection", code: 10,
                          userInfo: [NSLocalizedDescriptionKey: "第二个库没有打开"])
        }
        let secondWorkspace = secondRecord.workspace
        application.activate(secondRecord)
        guard let secondRoot = secondWorkspace.root else { throw CocoaError(.coderValueNotFound) }
        let session = try await Task.detached {
            try core.beginAgentSession(vault: secondRoot)
        }.value
        try await require(session.usable, "第二个库的会话不可用")
        fsWrite(secondWorkspace, "SecondOnly.md", "# Second only\n")
        fsWrite(secondWorkspace, "SecondEdit.md", "# Second edit\n")
        let secondModel = try await prepare(secondWorkspace, secondRecord.layout, session: session.id)
        let secondSummary = try requireSummary(secondModel)
        try await require(secondModel !== firstModel, "两个窗口共享同一个审阅模型")
        try await require(secondModel.selectedSession?.vault == secondRoot, "第二窗口会话不属于当前库")
        try await require(
            secondSummary.entries.contains { $0.id == "SecondOnly.md" },
            "第二窗口缺少自己的改动"
        )
        try await require(
            !secondSummary.entries.contains { $0.id == "Added/New note.md" },
            "第二窗口混入第一库改动"
        )
        try await require(
            firstSummary.entries.contains { $0.id == "Added/New note.md" },
            "第一窗口缺少自己的改动"
        )
        try await require(
            !firstSummary.entries.contains { $0.id == "SecondOnly.md" },
            "第一窗口混入第二库改动"
        )
        application.activate(firstRecord)
        return [
            "mode": "review-two-windows",
            "first_root": workspace.root ?? "",
            "second_root": secondRoot,
            "first_entries": firstSummary.entries.count,
            "second_entries": secondSummary.entries.count,
            "isolated": true,
        ]
    }

    private static func fsWrite(_ workspace: Workspace?, _ path: String, _ text: String) {
        guard let root = workspace?.root else { return }
        let url = URL(fileURLWithPath: root).appendingPathComponent(path)
        try? text.data(using: .utf8)?.write(to: url)
    }

    private static func large(
        workspace: Workspace,
        layout: Layout,
        arguments: [String]
    ) async throws -> [String: Any] {
        show(workspace, layout)
        guard let tab = workspace.addAgent(to: layout.defaultTerminalGroupID) else {
            throw CocoaError(.coderValueNotFound)
        }
        try await waitAgent(tab, seconds: 300)
        let sessionID = tab.agentSessionID ?? ""
        let model = try await prepare(workspace, layout, session: sessionID)
        let gap = ReviewGapRecorder()
        gap.start()
        await model.refreshSummary()
        try await waitSummary(model, seconds: 300)
        gap.stop()
        let summary = try requireSummary(model)
        try await require(model.lastSummaryMainThread == false, "大库摘要不在后台线程")
        try await require(gap.maxGapMilliseconds < 500, "大库扫描阻塞主线程 \(gap.maxGapMilliseconds)ms")
        resultChecks(summary)

        guard let core = workspace.core else { throw CocoaError(.coderValueNotFound) }
        let cancelled = ReviewCancellation()
        cancelled.cancel()
        var preCancelled = ""
        do {
            _ = try await Task.detached {
                try core.reviewAgentSession(id: sessionID, cancellation: cancelled)
            }.value
        } catch {
            preCancelled = String(describing: error)
        }
        try await require(preCancelled.contains("history.cancelled"), "取消令牌未生效")

        let midflight = ReviewCancellation()
        let task = Task.detached { () -> Result<SessionReviewSummary, Error> in
            Result { try core.reviewAgentSession(id: sessionID, cancellation: midflight) }
        }
        midflight.cancel()
        let midflightResult = await task.value
        let midflightCancelled: Bool
        switch midflightResult {
        case .success: midflightCancelled = false
        case .failure(let error): midflightCancelled = String(describing: error).contains("history.cancelled")
        }
        return [
            "mode": "review-large",
            "entries": summary.entries.count,
            "summary_ms": summary.elapsedMs,
            "max_main_gap_ms": gap.maxGapMilliseconds,
            "summary_main_thread": model.lastSummaryMainThread ?? true,
            "pre_cancelled": preCancelled,
            "midflight_cancelled": midflightCancelled,
            "files": summary.counts.added + summary.counts.modified,
        ]
    }

    private static func screenshot(
        workspace: Workspace,
        layout: Layout,
        arguments: [String]
    ) async throws -> [String: Any] {
        let model = try await prepare(
            workspace,
            layout,
            session: argument("review-session", in: arguments)
        )
        if let entry = argument("review-entry", in: arguments) {
            try await select(model, entry: entry)
            try await require(model.detail?.entry.id == entry, "截图条目详情未就绪")
        }
        let summary = try requireSummary(model)
        layout.visible[.bottom] = false
        try await Task.sleep(for: .milliseconds(400))
        return [
            "mode": "review-screenshot",
            "session": model.selectedSessionID ?? "",
            "entries": summary.entries.count,
            "selected": model.selectedEntryID ?? "",
            "text": model.detail?.text ?? false,
            "binary": model.detail?.binary ?? false,
            "title": L10n.string("Session Changes"),
        ]
    }

    private static func resultChecks(_ summary: SessionReviewSummary) {
        #if DEBUG
        assert(!summary.entries.isEmpty)
        #endif
    }
}

/// 主线程响应间隔：审阅扫描与 diff 在后台时不应出现长阻塞。
@MainActor
private final class ReviewGapRecorder {
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
