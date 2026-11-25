import Foundation
import HortusCore

/// 每篇文档一条串行队列；主线程只提交原生 UTF-16 范围，不复制全文。
/// `url` 由 NSLock 保护：文档队列写入，检查脚本从主线程读取。
public final class DocumentIO: @unchecked Sendable {
    public let session: DocumentSession
    public let queue = DispatchQueue(label: "app.hortus.document", qos: .userInteractive)
    private let lock = NSLock()
    private var storedURL: URL
    public var url: URL {
        lock.lock(); defer { lock.unlock() }
        return storedURL
    }

    public init(session: DocumentSession, url: URL) {
        self.session = session
        self.storedURL = url
    }
    /// 重命名后的路径迁移；在文档队列上执行会话迁移并更新磁盘路径。
    public func relocateOnQueue(root: String, newPath: String) throws -> DocumentUpdate {
        let update = try session.relocate(newRelativePath: newPath)
        lock.lock()
        storedURL = URL(fileURLWithPath: root).appendingPathComponent(newPath)
        lock.unlock()
        return update
    }
    public func performSelf<T: Sendable>(_ operation: @escaping @Sendable (DocumentIO) throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { continuation.resume(with: Result { try operation(self) }) }
        }
    }
    public func perform<T: Sendable>(_ operation: @escaping @Sendable (DocumentSession) throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { continuation.resume(with: Result { try operation(self.session) }) }
        }
    }
    public func submit<T: Sendable>(_ operation: @escaping @Sendable (DocumentSession) throws -> T,
                                    completion: @escaping @MainActor @Sendable (Result<T, Error>) -> Void) {
        queue.async {
            let result = Result { try operation(self.session) }
            Task { @MainActor in completion(result) }
        }
    }
    public func save(_ choice: SaveChoice = .normal) async throws -> DocumentUpdate {
        try await perform { _ in self.saveOnQueue(choice) }
    }
    public func saveOnQueue(_ choice: SaveChoice = .normal) -> DocumentUpdate {
        do {
            let directory = try FileManager.default.url(for: .itemReplacementDirectory, in: .userDomainMask,
                                                        appropriateFor: url, create: true)
            defer { try? FileManager.default.removeItem(at: directory) }
            return try session.save(temporary: directory.path, choice: choice)
        } catch { return session.failedSave(message: String(describing: error)) }
    }
    /// 关闭与退出共用：先写恢复稿，允许时再保存笔记；返回最新正文的持久化状态。
    public func prepareForCloseOnQueue(noteWrite: Bool) -> DocumentUpdate {
        guard noteWrite else { return session.prepareClose(temporary: "", writeNote: false) }
        do {
            let directory = try FileManager.default.url(for: .itemReplacementDirectory, in: .userDomainMask,
                                                        appropriateFor: url, create: true)
            defer { try? FileManager.default.removeItem(at: directory) }
            return session.prepareClose(temporary: directory.path, writeNote: true)
        } catch { return session.failedSave(message: String(describing: error)) }
    }
    public func prepareForClose(noteWrite: Bool) async -> DocumentUpdate {
        await withCheckedContinuation { continuation in
            queue.async {
                continuation.resume(returning: self.prepareForCloseOnQueue(noteWrite: noteWrite))
            }
        }
    }
    public func edit(range: NSRange, replacement: String, failed: @escaping @Sendable (String) -> Void) {
        queue.async {
            do { _ = try self.session.replace(start: UInt64(range.location), length: UInt64(range.length), replacement: replacement) }
            catch { failed(String(describing: error)) }
        }
    }
}
