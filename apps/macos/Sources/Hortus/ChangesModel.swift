import Foundation
import Darwin
import HortusCore

/// 一个窗口的 Changes 面板状态。会话、扫描与详情都只属于当前窗口打开的库；
/// 所有磁盘扫描在后台任务执行，主线程只接收最终结果。
@Observable @MainActor
final class ChangesModel {
    var sessions: [AgentSessionInfo] = []
    var selectedSessionID: String?
    var summary: SessionReviewSummary?
    var selectedEntryID: String?
    var detail: ReviewEntryDetail?
    var loadingSessions = false
    var loadingSummary = false
    var loadingDetail = false
    var error: String?
    var refreshedAt: Date?
    var rollbackBusy = false
    var notice: String?
    var noticeIsError = false
    private(set) var lastReport: SessionRollbackReportInfo?
    private(set) var lastSummaryMainThread: Bool?

    @ObservationIgnored weak var workspace: Workspace?
    @ObservationIgnored private var summaryGeneration = 0
    @ObservationIgnored private var detailGeneration = 0
    @ObservationIgnored private var summaryCancellation: ReviewCancellation?
    @ObservationIgnored private var detailCancellation: ReviewCancellation?

    var root: String? { workspace?.root }
    var selectedSession: AgentSessionInfo? {
        sessions.first { $0.id == selectedSessionID }
    }
    var selectedEntry: ReviewEntryInfo? {
        summary?.entries.first { $0.id == selectedEntryID }
    }
    var counts: ReviewCounts? { summary?.counts }
    var hasChanges: Bool { summary?.entries.isEmpty == false }
    var runningSessionIDs: [String] {
        sessions.filter { $0.state == "running" || $0.state == "preparing" }.map(\.id)
    }

    var canOpenSelectedNote: Bool {
        guard let entry = selectedEntry, let path = entry.newPath,
              entry.after?.kind == "file", root != nil else { return false }
        if let detail, detail.entry.id == entry.id { return detail.text }
        let suffix = (path as NSString).pathExtension.lowercased()
        return ["md", "markdown", "mmd", "mermaid"].contains(suffix)
    }

    /// 只有当前详情仍匹配选中条目、且是文本文件时才提供按处回滚。
    var canRevertHunks: Bool {
        guard let entry = selectedEntry, let detail, detail.entry.id == entry.id else { return false }
        return detail.text && !detail.binary && !detail.large && !detail.hunks.isEmpty
            && entry.change != "type_changed" && entry.after?.kind == "file"
    }

    var sessionIsActive: Bool {
        guard let state = selectedSession?.state else { return false }
        return state == "running" || state == "preparing"
    }

    var canRollBackAll: Bool {
        hasChanges && !sessionIsActive && !rollbackBusy && selectedSession?.usable == true
    }

    var canDeleteSnapshot: Bool {
        selectedSession != nil && !sessionIsActive && !rollbackBusy
    }

    /// 打开面板或切换会话时的入口：先刷新会话列表，再计算当前会话摘要。
    func activate(preferred: String? = nil) async {
        guard root != nil, workspace?.core != nil else {
            reset()
            return
        }
        await refreshSessions(preferred: preferred ?? workspace?.currentAgentSessionID)
        if selectedSession?.usable == true {
            await refreshSummary()
        }
    }

    func refreshSessions(preferred: String? = nil) async {
        guard let workspace, let root = workspace.root, let core = workspace.core else {
            reset()
            return
        }
        let wanted = preferred ?? selectedSessionID
        loadingSessions = true
        error = nil
        let result = await Task.detached {
            Result { try core.listAgentSessions(vault: root) }
        }.value
        guard workspace.root == root else {
            loadingSessions = false
            return
        }
        loadingSessions = false
        switch result {
        case .success(let records):
            sessions = records
            let current = wanted.flatMap { id in records.first { $0.id == id } }
            let chosen = current ?? records.first(where: \.usable) ?? records.first
            if selectedSessionID != chosen?.id {
                selectedSessionID = chosen?.id
                summary = nil
                selectedEntryID = nil
                detail = nil
            }
        case .failure(let failure):
            error = L10n.error(failure)
        }
    }

    /// 取消上一条未完成扫描并重新计算；旧代次结果不会写回界面。
    func refreshSummary() async {
        summaryGeneration += 1
        let generation = summaryGeneration
        summaryCancellation?.cancel()
        summaryCancellation = nil
        detailCancellation?.cancel()
        detailGeneration += 1
        guard let workspace, let root = workspace.root, let core = workspace.core,
              let session = selectedSession, session.usable else {
            loadingSummary = false
            summary = nil
            selectedEntryID = nil
            detail = nil
            return
        }
        let cancellation = ReviewCancellation()
        summaryCancellation = cancellation
        loadingSummary = true
        error = nil
        let id = session.id
        let result = await Task.detached {
            let mainThread = pthread_main_np() != 0
            return (mainThread, Result { try core.reviewAgentSession(id: id, cancellation: cancellation) })
        }.value
        guard generation == summaryGeneration, workspace.root == root else { return }
        lastSummaryMainThread = result.0
        loadingSummary = false
        switch result.1 {
        case .success(let value):
            summary = value
            if let selectedEntryID,
               !value.entries.contains(where: { $0.id == selectedEntryID }) {
                self.selectedEntryID = nil
                detail = nil
            }
            refreshedAt = Date()
            if let selectedEntryID {
                await loadDetail(selectedEntryID)
            }
        case .failure(let failure):
            if Self.isCancellation(failure) { return }
            summary = nil
            detail = nil
            error = L10n.error(failure)
        }
    }

    func select(sessionID: String) async {
        guard selectedSessionID != sessionID else {
            if summary == nil { await refreshSummary() }
            return
        }
        selectedSessionID = sessionID
        summary = nil
        selectedEntryID = nil
        detail = nil
        error = nil
        await refreshSummary()
    }

    func selectEntry(_ id: String) async {
        selectedEntryID = id
        await loadDetail(id)
    }

    func loadDetail(_ id: String) async {
        detailGeneration += 1
        let generation = detailGeneration
        detailCancellation?.cancel()
        detailCancellation = nil
        guard let workspace, let root = workspace.root, let core = workspace.core,
              let entry = summary?.entries.first(where: { $0.id == id }) else {
            loadingDetail = false
            detail = nil
            return
        }
        let cancellation = ReviewCancellation()
        detailCancellation = cancellation
        loadingDetail = true
        error = nil
        let sessionID = selectedSessionID
        let path = entry.newPath ?? entry.id
        let oldPath = entry.oldPath
        let result = await Task.detached {
            Result {
                try core.reviewAgentSessionEntry(
                    id: sessionID ?? "",
                    path: path,
                    oldPath: oldPath,
                    cancellation: cancellation
                )
            }
        }.value
        guard generation == detailGeneration, workspace.root == root,
              selectedEntryID == id else { return }
        loadingDetail = false
        switch result {
        case .success(let value):
            detail = value
        case .failure(let failure):
            if Self.isCancellation(failure) { return }
            detail = nil
            error = L10n.error(failure)
        }
    }

    /// 按处回滚当前 hunk；成功后刷新摘要与详情。
    @discardableResult
    func rollbackHunk(_ hunk: ReviewDiffHunk, index: Int) async -> Bool {
        guard !rollbackBusy, let core = workspace?.core, let session = selectedSession,
              let entry = selectedEntry, let detail, detail.entry.id == entry.id else { return false }
        let affected = Self.affectedPaths(entry)
        guard !blockIfUnsaved(affected) else { return false }
        let temporary = Self.replacementDirectory(for: root)
        let expectation = Self.expectation(entry)
        let cancellation = ReviewCancellation()
        let sessionID = session.id
        let path = entry.newPath ?? entry.id
        let oldPath = entry.oldPath
        rollbackBusy = true
        notice = nil
        let result = await Task.detached { () async -> Result<Void, Error> in
            defer { Self.removeReplacementDirectory(temporary) }
            return Result {
                try core.rollbackAgentSessionHunk(
                    id: sessionID,
                    path: path,
                    oldPath: oldPath,
                    expectation: expectation,
                    hunkIndex: UInt32(index),
                    hunk: hunk,
                    temporary: temporary,
                    cancellation: cancellation
                )
            }
        }.value
        rollbackBusy = false
        switch result {
        case .success:
            workspace?.refreshAfterRollback(affected)
            notice = L10n.string("Reverted %@", entry.id)
            noticeIsError = false
            await refreshSummary()
            return true
        case .failure(let failure):
            notice = L10n.error(failure)
            noticeIsError = true
            return false
        }
    }

    /// 按文件回滚当前选中条目。
    @discardableResult
    func rollbackSelectedEntry() async -> Bool {
        guard !rollbackBusy, let core = workspace?.core, let session = selectedSession, let entry = selectedEntry else {
            return false
        }
        let affected = Self.affectedPaths(entry)
        guard !blockIfUnsaved(affected) else { return false }
        let temporary = Self.replacementDirectory(for: root)
        let expectation = Self.expectation(entry)
        let cancellation = ReviewCancellation()
        let sessionID = session.id
        rollbackBusy = true
        notice = nil
        let result = await Task.detached { () async -> Result<Void, Error> in
            defer { Self.removeReplacementDirectory(temporary) }
            return Result {
                try core.rollbackAgentSessionEntry(
                    id: sessionID,
                    expectation: expectation,
                    temporary: temporary,
                    cancellation: cancellation
                )
            }
        }.value
        rollbackBusy = false
        switch result {
        case .success:
            workspace?.refreshAfterRollback(affected)
            notice = L10n.string("Reverted %@", entry.id)
            noticeIsError = false
            await refreshSummary()
            return true
        case .failure(let failure):
            notice = L10n.error(failure)
            noticeIsError = true
            return false
        }
    }

    /// 按会话回滚全部已审阅条目；部分失败或 stale 如实显示。
    @discardableResult
    func rollbackAll() async -> Bool {
        guard !rollbackBusy, let core = workspace?.core, let session = selectedSession, let summary else { return false }
        guard !sessionIsActive else {
            notice = L10n.string("Stop the agent before rolling back or deleting this snapshot")
            noticeIsError = true
            return false
        }
        guard !summary.entries.isEmpty else { return false }
        let affected = Set(summary.entries.flatMap { Self.affectedPaths($0) })
        guard !blockIfUnsaved(affected) else { return false }
        let expectations = summary.entries.map(Self.expectation)
        let temporary = Self.replacementDirectory(for: root)
        let cancellation = ReviewCancellation()
        let sessionID = session.id
        rollbackBusy = true
        notice = nil
        let result = await Task.detached { () async -> Result<SessionRollbackReportInfo, Error> in
            defer { Self.removeReplacementDirectory(temporary) }
            return Result {
                try core.rollbackAgentSession(
                    id: sessionID,
                    expectations: expectations,
                    temporary: temporary,
                    cancellation: cancellation
                )
            }
        }.value
        rollbackBusy = false
        switch result {
        case .success(let report):
            lastReport = report
            workspace?.refreshAfterRollback(affected)
            notice = Self.reportMessage(report)
            noticeIsError = !report.failed.isEmpty
            await refreshSummary()
            return report.failed.isEmpty
        case .failure(let failure):
            notice = L10n.error(failure)
            noticeIsError = true
            return false
        }
    }

    /// 显式删除当前会话快照；活动会话只提示，不发请求。
    @discardableResult
    func deleteSelectedSnapshot() async -> Bool {
        guard !rollbackBusy, let core = workspace?.core, let session = selectedSession else { return false }
        guard !sessionIsActive else {
            notice = L10n.string("Stop the agent before rolling back or deleting this snapshot")
            noticeIsError = true
            return false
        }
        rollbackBusy = true
        notice = nil
        let sessionID = session.id
        let result = await Task.detached { () async -> Result<Void, Error> in
            Result { try core.deleteAgentSession(id: sessionID) }
        }.value
        rollbackBusy = false
        switch result {
        case .success:
            sessions.removeAll { $0.id == sessionID }
            summary = nil
            selectedEntryID = nil
            detail = nil
            let next = sessions.first(where: \.usable) ?? sessions.first
            selectedSessionID = next?.id
            notice = L10n.string("Snapshot deleted")
            noticeIsError = false
            await refreshSessions(preferred: next?.id)
            if selectedSession?.usable == true { await refreshSummary() }
            return true
        case .failure(let failure):
            notice = L10n.error(failure)
            noticeIsError = true
            return false
        }
    }

    private func blockIfUnsaved(_ paths: Set<String>) -> Bool {
        guard let workspace, workspace.hasUnsavedEdits(in: paths) else { return false }
        notice = L10n.string("Save or close unsaved edits in the affected notes before rolling back")
        noticeIsError = true
        return true
    }

    private static func expectation(_ entry: ReviewEntryInfo) -> RollbackExpectationInfo {
        RollbackExpectationInfo(
            path: entry.id,
            oldPath: entry.oldPath,
            newPath: entry.newPath,
            change: entry.change,
            expectedBefore: entry.beforeDigest,
            expectedAfter: entry.afterDigest
        )
    }

    private static func affectedPaths(_ entry: ReviewEntryInfo) -> Set<String> {
        var paths = Set<String>([entry.id])
        if let old = entry.oldPath { paths.insert(old) }
        if let new = entry.newPath { paths.insert(new) }
        return paths
    }

    private static func replacementDirectory(for root: String?) -> String? {
        guard let root else { return nil }
        return try? FileManager.default.url(
            for: .itemReplacementDirectory,
            in: .userDomainMask,
            appropriateFor: URL(fileURLWithPath: root),
            create: true
        ).path
    }

    nonisolated private static func removeReplacementDirectory(_ path: String?) {
        guard let path else { return }
        try? FileManager.default.removeItem(atPath: path)
    }

    private static func reportMessage(_ report: SessionRollbackReportInfo) -> String {
        let reverted = report.revertedPaths.count
        if report.failed.isEmpty, report.skippedStale.isEmpty {
            return L10n.string("Rolled back %d changes", reverted)
        }
        if report.failed.isEmpty {
            return L10n.string(
                "Rolled back %d changes; %d skipped because the disk changed",
                reverted, report.skippedStale.count
            )
        }
        return L10n.string(
            "Rolled back %d changes; %d skipped because the disk changed, %d failed",
            reverted, report.skippedStale.count, report.failed.count
        )
    }

    func openSelectedNote() {
        guard canOpenSelectedNote, let path = selectedEntry?.newPath else { return }
        workspace?.openNote(path)
    }

    func reset() {
        summaryGeneration += 1
        detailGeneration += 1
        summaryCancellation?.cancel()
        detailCancellation?.cancel()
        summaryCancellation = nil
        detailCancellation = nil
        sessions = []
        selectedSessionID = nil
        summary = nil
        selectedEntryID = nil
        detail = nil
        loadingSessions = false
        loadingSummary = false
        loadingDetail = false
        error = nil
        refreshedAt = nil
        notice = nil
        noticeIsError = false
        rollbackBusy = false
        lastReport = nil
    }

    private static func isCancellation(_ error: Error) -> Bool {
        String(describing: error).contains("history.cancelled")
    }
}
