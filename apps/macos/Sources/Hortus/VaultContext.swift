import AppKit
import HortusCore

/// 一个库只有一条阻塞批次消费者；关闭后等待线程退出。
final class VaultSubscription: Sendable {
    let handle: VaultHandle
    private let finished = DispatchGroup()

    init(handle: VaultHandle, receive: @escaping @Sendable ([TreeBatch]) -> Void) {
        self.handle = handle
        let finished = finished
        finished.enter()
        let thread = Thread {
            defer { finished.leave() }
            while true {
                let batches = handle.waitForBatches()
                guard !batches.isEmpty else { return }
                receive(batches)
            }
        }
        thread.name = "hortus-batches"
        thread.start()
    }

    func close() {
        handle.close()
        finished.wait()
    }
}

/// 库级资源不属于任何窗口。批次与索引只更新一次，文档刷新广播给弱订阅者。
@Observable @MainActor
final class VaultContext {
    let root: String
    let handle: VaultHandle
    var entries: [String: TreeEntry] = [:]
    var treeVersion = 0
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

    private func indexed(changed: Bool, scanFinished: Bool) {
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
