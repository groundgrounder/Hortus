import Foundation
import HortusCore

/// 一个库只有一条阻塞批次消费者；关闭后等待线程退出。
public final class VaultSubscription: Sendable {
    /// 关闭后丢弃尚未索引的排队批次；已经取出的批次先完成，最多 128 个文件。
    private final class Closing: @unchecked Sendable {
        private let lock = NSLock()
        private var value = false
        func set() { lock.lock(); value = true; lock.unlock() }
        var isSet: Bool { lock.lock(); defer { lock.unlock() }; return value }
    }

    public let handle: VaultHandle
    private let finished = DispatchGroup()
    private let closing = Closing()

    public init(handle: VaultHandle, receive: @escaping @Sendable ([TreeBatch]) -> Void) {
        self.handle = handle
        let finished = finished
        let closing = closing
        finished.enter()
        let thread = Thread {
            defer { finished.leave() }
            while true {
                let batches = handle.waitForBatches()
                if closing.isSet { return }
                guard !batches.isEmpty else { return }
                // 一批最多 128 个目录项；关闭时只等当前小批完成，不再索引剩余队列。
                for batch in batches {
                    if closing.isSet { return }
                    receive([batch])
                }
            }
        }
        thread.name = "hortus-batches"
        thread.start()
    }

    public func close() {
        closing.set()
        handle.close()
        finished.wait()
    }
}
