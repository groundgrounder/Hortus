import Foundation
import HortusApple
import HortusCore

@Observable @MainActor
final class IOSNoteDocument {
    let diagramIdentity = UUID().uuidString
    let io: DocumentIO
    var text: String
    private(set) var textRevision: UInt64
    var revision: UInt64 { didSet { if revision != oldValue { scheduleOutline() } } }
    var headings: [DocumentHeading] = []
    var outlineRevision: UInt64?
    var dirty: Bool
    var persistence: DocumentPersistence
    var readOnlyReason: String?
    var issue: DocumentIssue
    var message: String
    var reading = true
    var sourceAnchor = 0
    var sourceSelectionLength = 0
    var marked = false
    var pendingReveal: (source: Int, length: Int)?

    @ObservationIgnored var onReload: ((String) -> Void)?
    @ObservationIgnored var onSaved: (() -> Void)?
    @ObservationIgnored var onConflictCopy: ((String) -> Void)?
    @ObservationIgnored private var autoSave: Task<Void, Never>?
    @ObservationIgnored private var outlineTask: Task<Void, Never>?
    @ObservationIgnored private var refreshing = false
    @ObservationIgnored private var refreshAgain = false
    @ObservationIgnored private var closed = false

    init(io: DocumentIO, initial: DocumentUpdate) {
        self.io = io
        self.text = initial.text ?? ""
        self.revision = initial.revision
        self.textRevision = initial.revision
        self.dirty = initial.dirty
        self.persistence = initial.persistence
        self.readOnlyReason = initial.readOnlyReason
        self.issue = initial.issue
        self.message = initial.message
        scheduleOutline(delay: .zero)
    }

    private func scheduleOutline(delay: Duration = .milliseconds(120)) {
        outlineTask?.cancel()
        guard !closed else { return }
        let io = io
        let expected = revision
        outlineTask = Task { [weak self] in
            do {
                if delay != .zero { try await Task.sleep(for: delay) }
                let value = try await io.perform { session in try session.outline() }
                guard !Task.isCancelled, let self, !closed, revision == expected, value.revision == expected else { return }
                headings = value.headings
                outlineRevision = expected
            } catch {}
        }
    }

    func adoptReadingText(_ text: String, revision: UInt64) {
        guard revision == self.revision else { return }
        if self.text != text { self.text = text }
        textRevision = revision
    }

    func edited(range: NSRange, replacement: String) {
        revision += 1
        if !dirty { dirty = true }
        let ns = text as NSString
        if range.location <= ns.length, NSMaxRange(range) <= ns.length {
            text = ns.replacingCharacters(in: range, with: replacement)
            textRevision = revision
        }
        io.edit(range: range, replacement: replacement) { [weak self] _ in
            Task { @MainActor in
                self?.message = "document.save_failed"
                self?.issue = .saveFailed
            }
        }
        scheduleSave()
    }

    func compositionChanged(_ marked: Bool) {
        self.marked = marked
        if marked { autoSave?.cancel() }
        else {
            scheduleSave()
            if refreshAgain { refresh() }
        }
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
        guard !marked, dirty || issue == .missing || choice != .normal else { return }
        do {
            let update = try await io.save(choice)
            apply(update)
            if update.issue == .none && !update.dirty { onSaved?() }
            if issue == .saveFailed { scheduleSave(delay: .seconds(2)) }
        } catch {
            message = "document.save_failed"
            issue = .saveFailed
            scheduleSave(delay: .seconds(2))
        }
    }

    func refresh() {
        if marked || refreshing { refreshAgain = true; return }
        refreshing = true
        refreshAgain = false
        io.submit({ try $0.refresh() }, completion: { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let update):
                apply(update, external: true)
            case .failure:
                message = "document.save_failed"
                issue = .saveFailed
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
        } catch {
            message = "document.save_failed"
            issue = .saveFailed
        }
    }

    func recover(_ restore: Bool) async {
        guard !marked else { return }
        do {
            let update = try await io.perform { try $0.recover(restore: restore) }
            apply(update)
            if restore { scheduleSave() }
            else { refresh() }
        } catch {
            message = "document.save_failed"
        }
    }

    func prepareToClose() async -> DocumentPersistence {
        let update = await io.prepareForClose(noteWrite: !marked)
        apply(update)
        if update.persistence == .unsaved, dirty, !marked { scheduleSave(delay: .seconds(2)) }
        return update.persistence
    }

    func abandon() {
        stopForShutdown()
        let session = io.session
        io.queue.async { _ = try? session.recover(restore: false) }
    }

    func stopForShutdown() {
        closed = true
        autoSave?.cancel()
        outlineTask?.cancel()
    }

    private func apply(_ update: DocumentUpdate, external: Bool = false) {
        persistence = update.persistence
        guard let text = update.text else {
            readOnlyReason = update.readOnlyReason
            if update.revision == revision { dirty = update.dirty }
            if update.issue != .none || update.revision == revision {
                issue = update.issue
                message = update.message
            }
            return
        }
        if external || update.copyPath != nil {
            readOnlyReason = update.readOnlyReason
            guard update.revision == revision, !marked else { refresh(); return }
            let expected = revision
            let io = io
            io.queue.async { [weak self] in
                do {
                    var accepted = try io.session.acceptExternal(revision: expected)
                    accepted.text = nil
                    let status = accepted
                    Task { @MainActor in self?.apply(status) }
                } catch {
                    Task { @MainActor in
                        self?.issue = .saveFailed
                        self?.message = "document.save_failed"
                    }
                }
            }
            revision += 1
            dirty = false
            issue = .none
            message = ""
        } else {
            revision = update.revision
            dirty = update.dirty
            readOnlyReason = update.readOnlyReason
            issue = update.issue
            message = update.message
        }
        self.text = text
        textRevision = revision
        onReload?(text)
        if let copy = update.copyPath { onConflictCopy?(copy) }
    }
}
