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
        dirty = initial.dirty; readOnlyReason = initial.readOnlyReason
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
        io.submit({ try $0.refreshAndAccept() }, completion: { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success(update):
                refreshIOMilliseconds = (ProcessInfo.processInfo.systemUptime - refreshStartedAt) * 1000
                if let text = update.text, !marked {
                    revision = update.revision
                    dirty = update.dirty; readOnlyReason = update.readOnlyReason
                    issue = update.issue; message = update.message
                    self.text = text
                    onReload?(text)
                    onStateChange?()
                } else if update.text != nil {
                    refreshAgain = true
                } else {
                    apply(update, external: true)
                }
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
        } catch { message = "document.save_failed" }
    }
    func checkpoint() async {
        do { try await io.perform { try $0.checkpoint() } }
        catch { message = "document.save_failed"; issue = .saveFailed }
    }
    func prepareToClose() async {
        closed = true; autoSave?.cancel(); outlineTask?.cancel()
        onClosing?()
        // 组合文本同样留在恢复稿中，但未提交的组合不写进用户笔记。
        await checkpoint()
        await saveNow()
    }
    func stopForShutdown() { closed = true; autoSave?.cancel(); outlineTask?.cancel(); onClosing?() }
    private func apply(_ update: DocumentUpdate, external: Bool = false) {
        if update.revision != revision, update.issue == .none, update.text == nil { return }
        saveMilliseconds = Double(update.saveMicros) / 1000
        if let text = update.text {
            if external || update.copyPath != nil {
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
            } else { revision = update.revision; dirty = update.dirty; issue = update.issue; message = update.message }
            self.text = text; readOnlyReason = update.readOnlyReason
            onReload?(text)
        } else {
            if update.revision == revision { dirty = update.dirty; readOnlyReason = update.readOnlyReason }
            issue = update.issue; message = update.message
        }
        if let copy = update.copyPath { onCopy?(copy) }
        onStateChange?()
    }
}
