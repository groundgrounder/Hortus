import AppKit
import HortusCore

/// 每篇文档一条串行队列；主线程只提交原生 UTF-16 范围，不复制全文。
final class DocumentIO: Sendable {
    let session: DocumentSession
    let url: URL
    let queue = DispatchQueue(label: "app.hortus.document", qos: .userInteractive)

    init(session: DocumentSession, url: URL) { self.session = session; self.url = url }
    func perform<T: Sendable>(_ operation: @escaping @Sendable (DocumentSession) throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { continuation.resume(with: Result { try operation(self.session) }) }
        }
    }
    func submit<T: Sendable>(_ operation: @escaping @Sendable (DocumentSession) throws -> T,
                             completion: @escaping @MainActor @Sendable (Result<T, Error>) -> Void) {
        queue.async {
            let result = Result { try operation(self.session) }
            Task { @MainActor in completion(result) }
        }
    }
    func save(_ choice: SaveChoice = .normal) async throws -> DocumentUpdate {
        try await perform { _ in self.saveOnQueue(choice) }
    }
    func saveOnQueue(_ choice: SaveChoice = .normal) -> DocumentUpdate {
        do {
            let directory = try FileManager.default.url(for: .itemReplacementDirectory, in: .userDomainMask,
                                                        appropriateFor: url, create: true)
            defer { try? FileManager.default.removeItem(at: directory) }
            return try session.save(temporary: directory.path, choice: choice)
        } catch { return session.failedSave(message: String(describing: error)) }
    }
    /// 关闭与退出共用：先写恢复稿，允许时再保存笔记；返回最新正文的持久化状态。
    func prepareForCloseOnQueue(noteWrite: Bool) -> DocumentUpdate {
        guard noteWrite else { return session.prepareClose(temporary: "", writeNote: false) }
        do {
            let directory = try FileManager.default.url(for: .itemReplacementDirectory, in: .userDomainMask,
                                                        appropriateFor: url, create: true)
            defer { try? FileManager.default.removeItem(at: directory) }
            return session.prepareClose(temporary: directory.path, writeNote: true)
        } catch { return session.failedSave(message: String(describing: error)) }
    }
    func prepareForClose(noteWrite: Bool) async -> DocumentUpdate {
        await withCheckedContinuation { continuation in
            queue.async {
                continuation.resume(returning: self.prepareForCloseOnQueue(noteWrite: noteWrite))
            }
        }
    }
    func edit(range: NSRange, replacement: String, failed: @escaping @Sendable (String) -> Void) {
        queue.async {
            do { _ = try self.session.replace(start: UInt64(range.location), length: UInt64(range.length), replacement: replacement) }
            catch { failed(String(describing: error)) }
        }
    }
}

@Observable @MainActor
final class NoteDocument {
    let io: DocumentIO
    // 只在打开、外部载入和恢复时改变；日常输入保留在原生存储与 Rust 镜像中。
    var text: String
    var revision: UInt64 { didSet { if revision != oldValue { scheduleOutline() } } }
    var headings: [DocumentHeading] = []
    var outlineRevision: UInt64?
    @ObservationIgnored var outlineReadyAt = 0.0
    @ObservationIgnored var outlineDisplayedAt = 0.0
    @ObservationIgnored var outlineMilliseconds = 0.0
    var dirty: Bool
    /// 最新正文是否已安全落盘（笔记文件或恢复稿）。
    var persistence: DocumentPersistence
    var readOnlyReason: String?
    var issue: DocumentIssue
    var message: String
    var saveMilliseconds = 0.0
    var reading = true
    var sourceAnchor = 0
    var sourceSelectionLength = 0
    var marked = false
    @ObservationIgnored weak var viewOwner: AnyObject?
    @ObservationIgnored var refreshStartedAt = 0.0
    @ObservationIgnored var refreshIOMilliseconds = 0.0
    @ObservationIgnored var externalAcceptMilliseconds = 0.0
    @ObservationIgnored var onReload: ((String) -> Void)?
    @ObservationIgnored var onClosing: (() -> Void)?
    @ObservationIgnored var onStateChange: (() -> Void)?
    @ObservationIgnored var onCopy: ((String) -> Void)?
    @ObservationIgnored private var autoSave: Task<Void, Never>?
    @ObservationIgnored private var outlineTask: Task<Void, Never>?
    @ObservationIgnored private var refreshing = false
    @ObservationIgnored private var refreshAgain = false
    @ObservationIgnored private var closed = false

    init(io: DocumentIO, initial: DocumentUpdate) {
        self.io = io; text = initial.text ?? ""; revision = initial.revision
        dirty = initial.dirty; persistence = initial.persistence; readOnlyReason = initial.readOnlyReason
        issue = initial.issue; message = initial.message
        scheduleOutline(delay: .zero)
    }
    /// 输入只重置一次性防抖任务；解析与 FFI 都在文档原有队列上运行。
    private func scheduleOutline(delay: Duration = .milliseconds(120)) {
        outlineTask?.cancel()
        guard !closed else { return }
        let io = io, expected = revision
        outlineTask = Task { [weak self] in
            do {
                if delay != .zero { try await Task.sleep(for: delay) }
                let value = try await io.perform { session in
                    let start = ProcessInfo.processInfo.systemUptime
                    return (try session.outline(), (ProcessInfo.processInfo.systemUptime - start) * 1000)
                }
                guard !Task.isCancelled, let self, !closed, revision == expected, value.0.revision == expected else { return }
                headings = value.0.headings; outlineRevision = expected
                outlineMilliseconds = value.1; outlineReadyAt = ProcessInfo.processInfo.systemUptime
            } catch {}
        }
    }
    func edited(range: NSRange, replacement: String) {
        revision += 1
        if !dirty { dirty = true }
        io.edit(range: range, replacement: replacement) { [weak self] error in
            Task { @MainActor in self?.message = "document.save_failed"; self?.issue = .saveFailed }
        }
        scheduleSave()
    }
    func compositionChanged(_ marked: Bool) {
        self.marked = marked
        if marked { autoSave?.cancel() }
        else { scheduleSave(); if refreshAgain { refresh() } }
    }
    private func scheduleSave(delay: Duration = .seconds(1)) {
        autoSave?.cancel()
        guard dirty, !marked, !closed else { return }
        autoSave = Task { [weak self] in
            do { try await Task.sleep(for: delay) } catch { return }
            await self?.saveNow()
        }
    }
    func saveNow(_ choice: SaveChoice = .normal) async {
        autoSave?.cancel()
        guard !marked, dirty || issue == .missing else { return }
        do {
            let update = try await io.save(choice)
            apply(update)
            if issue == .saveFailed { scheduleSave(delay: .seconds(2)) }
        } catch { message = "document.save_failed"; issue = .saveFailed; scheduleSave(delay: .seconds(2)) }
    }
    func refresh() {
        if marked || refreshing { refreshAgain = true; return }
        refreshing = true; refreshAgain = false
        refreshStartedAt = ProcessInfo.processInfo.systemUptime
        // 只读取外部版本并提议；接受在 MainActor 确认没有新输入和组合后由 apply 排队。
        io.submit({ try $0.refresh() }, completion: { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let update):
                refreshIOMilliseconds = (ProcessInfo.processInfo.systemUptime - refreshStartedAt) * 1000
                apply(update, external: true)
            case .failure:
                message = "document.save_failed"; issue = .saveFailed
            }
            refreshing = false
            if refreshAgain, !marked { refresh() }
        })
    }
    func useDisk() async {
        autoSave?.cancel()
        guard !marked else { return }
        do {
            let update = try await io.perform { try $0.useDisk() }
            if update.revision != revision { refresh(); return }
            apply(update, external: true)
        } catch { message = "document.save_failed"; issue = .saveFailed }
    }
    func recover(_ restore: Bool) async {
        guard !marked else { return }
        do {
            let update = try await io.perform { try $0.recover(restore: restore) }
            apply(update)
            if restore { scheduleSave() }
            // 丢弃草稿后载入磁盘上的最新正文；若磁盘已删除，刷新会进入 Missing。
            else { refresh() }
        } catch { message = "document.save_failed" }
    }
    func checkpoint() async {
        do { try await io.perform { try $0.checkpoint() } }
        catch { message = "document.save_failed"; issue = .saveFailed }
    }
    /// 关闭与退出前的持久化。不先置为 closed：返回 `.unsaved` 时文档必须继续自动保存、可编辑。
    func prepareToClose() async -> DocumentPersistence {
        // 组合输入同样留在恢复稿中，但未提交的组合不写进用户笔记。
        let update = await io.prepareForClose(noteWrite: !marked)
        apply(update)
        if update.persistence == .unsaved, dirty, !marked { scheduleSave(delay: .seconds(2)) }
        return update.persistence
    }
    /// 用户明确选择放弃后：停止自动保存并删除该文档的恢复稿，不留垃圾。
    func abandon() {
        stopForShutdown()
        let session = io.session
        io.queue.async { _ = try? session.recover(restore: false) }
    }
    func stopForShutdown() { closed = true; autoSave?.cancel(); outlineTask?.cancel(); onClosing?() }
    private func apply(_ update: DocumentUpdate, external: Bool = false) {
        saveMilliseconds = Double(update.saveMicros) / 1000
        persistence = update.persistence
        guard let text = update.text else {
            // 权限独立于正文版本：即使刷新后又有输入使修订号过时，也要采用最新只读状态。
            readOnlyReason = update.readOnlyReason
            if update.revision == revision { dirty = update.dirty }
            if update.issue != .none || update.revision == revision {
                issue = update.issue; message = update.message
            }
            onStateChange?()
            return
        }
        if external || update.copyPath != nil {
            readOnlyReason = update.readOnlyReason
            guard update.revision == revision, !marked else { refresh(); return }
            let expected = revision
            let io = io
            // 主线程确认没有新输入后，先入队接受，再允许后续编辑进入队列。
            io.queue.async { [weak self] in
                do {
                    let started = ProcessInfo.processInfo.systemUptime
                    var accepted = try io.session.acceptExternal(revision: expected)
                    let elapsed = (ProcessInfo.processInfo.systemUptime - started) * 1000
                    accepted.text = nil
                    let status = accepted
                    Task { @MainActor in
                        self?.externalAcceptMilliseconds = elapsed
                        self?.apply(status)
                    }
                } catch {
                    Task { @MainActor in self?.issue = .saveFailed; self?.message = "document.save_failed" }
                }
            }
            revision += 1; dirty = false; issue = .none; message = ""
        } else {
            revision = update.revision; dirty = update.dirty; readOnlyReason = update.readOnlyReason
            issue = update.issue; message = update.message
        }
        self.text = text
        onReload?(text)
        if let copy = update.copyPath { onCopy?(copy) }
        onStateChange?()
    }
}
