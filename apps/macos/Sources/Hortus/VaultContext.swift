import AppKit
import HortusCore
import HortusApple

/// 库级资源不属于任何窗口。批次与索引只更新一次，文档刷新广播给弱订阅者。
@Observable @MainActor
final class VaultContext {
    let root: String
    let handle: VaultHandle
    var entries: [String: TreeEntry] = [:]
    var treeVersion = 0
    var knowledgeVersion = 0
    var knowledgeRewritePaths: Set<String> = []
    var renaming = false
    var scanning = true
    var scanMicros: UInt64 = 0
    var searchIndexReadyMilliseconds: Double?
    var searchIndexPeakRSSDeltaBytes: Int64?
    @ObservationIgnored private var subscription: VaultSubscription?
    @ObservationIgnored private var windows: [UUID: WeakWorkspace] = [:]
    @ObservationIgnored private let openedAt = ContinuousClock.now
    @ObservationIgnored private var baselineRSS: Int64 = 0

    private final class WeakWorkspace {
        weak var value: Workspace?
        init(_ value: Workspace) { self.value = value }
    }

    init(handle: VaultHandle) {
        self.handle = handle
        root = handle.root()
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        baselineRSS = Int64(usage.ru_maxrss)
    }

    func attach(_ workspace: Workspace) {
        windows[workspace.id] = WeakWorkspace(workspace)
        guard subscription == nil else { return }
        let handle = handle
        subscription = VaultSubscription(handle: handle) { [weak self] batches in
            DispatchQueue.main.async { [weak self] in self?.apply(batches) }
            let upserted = batches.flatMap { $0.upserted.filter { !$0.directory }.map(\.path) }
            let removed = batches.flatMap(\.removed)
            handle.updateSearchIndex(upserted: upserted, removed: removed)
            let changed = !upserted.isEmpty || !removed.isEmpty
            let scanFinished = batches.contains(where: \.scanComplete)
            DispatchQueue.main.async { [weak self] in
                self?.indexed(changed: changed, scanFinished: scanFinished)
            }
        }
    }

    var attachedWorkspaces: [Workspace] { windows.values.compactMap(\.value) }

    func detach(_ workspace: Workspace) { windows.removeValue(forKey: workspace.id) }
    var windowCount: Int { windows.values.filter { $0.value != nil }.count }
    var listenerCount: Int { subscription == nil ? 0 : 1 }

    private func apply(_ batches: [TreeBatch]) {
        for batch in batches {
            for path in batch.removed { entries.removeValue(forKey: path) }
            for item in batch.upserted { entries[item.path] = item }
            if batch.scanComplete { scanning = false; scanMicros = batch.scanMicros }
        }
        treeVersion += 1
        for workspace in windows.values.compactMap(\.value) { workspace.applyVaultEvents(batches) }
    }

    /// 新建文件成功后的立即更新：不等文件监听批次，树缓存先行反映磁盘。
    func upsert(_ entry: TreeEntry) {
        entries[entry.path] = entry
        treeVersion += 1
        if !entry.directory { updateIndices(upserted: [entry.path]) }
    }

    /// 重命名成功后的立即迁移：条目、父目录与搜索索引按新旧路径同步搬家，
    /// 避免文件监听批次到达前出现闪烁或误触 Missing 提示条。
    func entryRenamed(oldPath: String, entry: TreeEntry) {
        let prefix = entry.directory ? oldPath + "/" : nil
        let affected = entries.filter { key, _ in
            key == oldPath || prefix.map({ key.hasPrefix($0) }) == true
        }
        var movedNotes: [String] = []
        let removedNotes = affected.filter { !$0.value.directory }.map(\.key)
        for (key, value) in affected {
            entries.removeValue(forKey: key)
            var item = value
            if key == oldPath {
                // 同目录重命名：父目录不变，路径与名称一起更新。
                item.path = entry.path
                item.name = entry.name
            } else if let prefix {
                item.path = entry.path + "/" + key.dropFirst(prefix.count)
                item.name = (item.path as NSString).lastPathComponent
                if value.parent == oldPath { item.parent = entry.path }
                else if value.parent.hasPrefix(prefix) {
                    item.parent = entry.path + "/" + value.parent.dropFirst(prefix.count)
                }
            }
            if !item.directory { movedNotes.append(item.path) }
            entries[item.path] = item
        }
        treeVersion += 1
        updateIndices(upserted: movedNotes, removed: removedNotes)
    }

    /// 移到废纸篓成功后的立即移除：条目与搜索索引同步清理。
    /// 返回索引任务，调用方可在关闭标签前等索引真正去掉该路径。
    @discardableResult
    func removeTrashed(_ path: String, directory: Bool) -> Task<Void, Never>? {
        let prefix = directory ? path + "/" : nil
        let affected = entries.keys.filter { key in key == path || prefix.map({ key.hasPrefix($0) }) == true }
        let removedNotes = affected.filter { entries[$0]?.directory == false }
        for key in affected { entries.removeValue(forKey: key) }
        treeVersion += 1
        return updateIndices(removed: removedNotes)
    }

    func refreshSavedNote(_ path: String) { updateIndices(upserted: [path]) }

    @discardableResult
    private func updateIndices(upserted: [String] = [], removed: [String] = []) -> Task<Void, Never>? {
        guard !upserted.isEmpty || !removed.isEmpty else { return nil }
        let handle = handle
        return Task { [weak self] in
            await Task.detached { handle.updateSearchIndex(upserted: upserted, removed: removed) }.value
            self?.indexed(changed: true, scanFinished: false)
        }
    }

    func didRewriteLinks(_ paths: Set<String>) {
        indexed(changed: true, scanFinished: false)
        for workspace in attachedWorkspaces { workspace.refreshAfterRollback(paths) }
    }

    private func indexed(changed: Bool, scanFinished: Bool) {
        if changed || scanFinished { knowledgeVersion &+= 1 }
        if scanFinished {
            let elapsed = openedAt.duration(to: .now).components
            searchIndexReadyMilliseconds = Double(elapsed.seconds) * 1000 + Double(elapsed.attoseconds) / 1e15
            var usage = rusage()
            getrusage(RUSAGE_SELF, &usage)
            searchIndexPeakRSSDeltaBytes = max(0, Int64(usage.ru_maxrss) - baselineRSS)
        }
        for workspace in windows.values.compactMap(\.value) {
            workspace.searchIndexUpdated(changed: changed, scanFinished: scanFinished)
        }
    }

    /// 仅在应用确认最后一个使用者已脱离后调用。
    func close(completion: @escaping @Sendable () -> Void) {
        let old = subscription
        subscription = nil
        DispatchQueue.global(qos: .userInitiated).async { old?.close(); completion() }
    }

    func close() async {
        await withCheckedContinuation { continuation in close { continuation.resume() } }
    }
}
