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
    }

    private static func isCancellation(_ error: Error) -> Bool {
        String(describing: error).contains("history.cancelled")
    }
}
