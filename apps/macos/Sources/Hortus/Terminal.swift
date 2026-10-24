import AppKit
import Darwin
import SwiftTerm
import SwiftUI

/// The single terminal session owned by a workspace.
///
/// SwiftTerm owns the PTY and its terminal emulator. This object owns the
/// session boundary: a vault change always tears down the old shell before a
/// new one is started, and application shutdown waits for the child to exit.
@Observable @MainActor
final class TerminalSession {
    enum State: Equatable {
        case empty
        /// A vault root is set and the shell starts once the view is attached.
        /// This is the only state that starts a shell automatically, so a shell
        /// that exits on its own stays exited across SwiftUI updates.
        case starting
        case running
        case exited(Int32?)
        case failed
    }

    private(set) var state: State = .empty
    private(set) var root: String?
    private var view: HortusTerminalView?
    private var stopWaiters: [CheckedContinuation<Void, Never>] = []
    private var killTask: Task<Void, Never>?

    var statusText: String {
        switch state {
        case .empty: return L10n.string("Open a knowledge base to use the terminal")
        case .starting, .running: return ""
        case .exited: return L10n.string("Process exited")
        case .failed: return L10n.string("Unable to start the shell")
        }
    }

    func attach(_ view: HortusTerminalView, fontSize: Double) {
        self.view = view
        view.processDelegate = view.delegateProxy
        view.applyAppearance(fontSize: fontSize)
        startIfPossible()
    }

    /// The status and Restart overlay is only for a shell that exited or failed.
    var showsStatusOverlay: Bool {
        switch state {
        case .exited, .failed: true
        case .empty, .starting, .running: false
        }
    }

    func detach(_ view: HortusTerminalView) {
        // Keep the one PTY-backed view alive while SwiftUI moves the panel.
        // The next representable update will reuse it in its new hierarchy.
        guard self.view === view else { return }
    }

    func update(fontSize: Double) {
        view?.applyAppearance(fontSize: fontSize)
    }

    func focus() {
        guard let view, let window = view.window else { return }
        window.makeFirstResponder(view)
    }

    func clearScreen() {
        guard let view, view.process.running else { return }
        view.send([12])
    }

    func makeView(fontSize: Double) -> HortusTerminalView {
        if let view {
            view.removeFromSuperview()
            attach(view, fontSize: fontSize)
            return view
        }
        let view = HortusTerminalView(session: self)
        attach(view, fontSize: fontSize)
        return view
    }

    func setRoot(_ root: String?) {
        Task { @MainActor [weak self] in await self?.setRootAndWait(root) }
    }

    func setRootAndWait(_ root: String?) async {
        // Reopening the same vault keeps the running shell.
        if self.root == root, state == .running { return }
        if self.root != root {
            await stopAndWait()
        }
        self.root = root
        state = root == nil ? .empty : .starting
        startIfPossible()
    }

    func restart() {
        guard root != nil else { return }
        guard let view, !view.process.running, !view.process.windingDown else { return }
        state = .starting
        startIfPossible()
    }

    func stopAndWait() async {
        guard let view, view.process.running || view.process.windingDown else {
            killTask?.cancel()
            return
        }
        await withCheckedContinuation { continuation in
            stopWaiters.append(continuation)
            sendTerminationSignals(for: view)
        }
    }

    /// Start shutdown before AppKit enters terminateLater's nested run loop.
    /// Neither MainActor tasks nor SwiftTerm's main-queue exit monitor can be
    /// relied on there. Observe exit without taking over SwiftTerm's reaping.
    func stopForShutdown(completion: @escaping @Sendable () -> Void) {
        killTask?.cancel()
        killTask = nil
        guard let pid = view?.process.shellPid, pid > 0 else { completion(); return }
        Self.signalProcessGroup(pid, signal: SIGHUP)
        DispatchQueue.global(qos: .userInitiated).async {
            let deadline = ProcessInfo.processInfo.systemUptime + 1
            while !Self.hasExited(pid), ProcessInfo.processInfo.systemUptime < deadline {
                Thread.sleep(forTimeInterval: 0.01)
            }
            if !Self.hasExited(pid) { Self.signalProcessGroup(pid, signal: SIGKILL) }
            while !Self.hasExited(pid) { Thread.sleep(forTimeInterval: 0.01) }
            completion()
        }
    }

    nonisolated private static func hasExited(_ pid: pid_t) -> Bool {
        var info = siginfo_t()
        // kill(pid, 0) also succeeds for zombies. WNOWAIT leaves the wait
        // status available to SwiftTerm when its main-queue monitor resumes.
        let result = waitid(P_PID, id_t(pid), &info, WEXITED | WNOHANG | WNOWAIT)
        return result == 0 ? info.si_pid == pid : errno == ECHILD
    }

    private func startIfPossible() {
        guard state == .starting, let root, let view, !view.process.running, !view.process.windingDown else { return }
        let shell = ProcessInfo.processInfo.environment["SHELL"].flatMap { $0.isEmpty ? nil : $0 } ?? "/bin/zsh"
        var environment = ProcessInfo.processInfo.environment
        environment["TERM"] = "xterm-256color"
        environment["HORTUS_VAULT"] = root
        // Apps started from Finder or the Dock get no locale variables. Without
        // one the shell treats input as ASCII, so CJK file names and output break.
        if ["LANG", "LC_ALL", "LC_CTYPE"].allSatisfy({ environment[$0]?.isEmpty ?? true }) {
            environment["LANG"] = Self.utf8Locale()
        }
        let variables = environment.map { "\($0.key)=\($0.value)" }.sorted()
        state = .running
        view.startProcess(executable: shell, args: ["-l"], environment: variables, currentDirectory: root)
    }

    private func sendTerminationSignals(for view: HortusTerminalView) {
        let pid = view.process.shellPid
        guard pid > 0 else {
            view.terminate()
            return
        }
        killTask?.cancel()
        Self.signalProcessGroup(pid, signal: SIGHUP)
        killTask = Task { [weak self, weak view] in
            do { try await Task.sleep(for: .seconds(1)) } catch { return }
            guard let self, let view, view.process.running || view.process.windingDown else { return }
            Self.signalProcessGroup(pid, signal: SIGKILL)
            self.killTask = nil
        }
    }

    private func processTerminated(exitCode: Int32?) {
        killTask?.cancel()
        killTask = nil
        state = root == nil ? .empty : .exited(exitCode)
        let waiters = stopWaiters
        stopWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }

    private func processFailed() {
        killTask?.cancel()
        killTask = nil
        state = .failed
        let waiters = stopWaiters
        stopWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }

    /// The first installed UTF-8 locale that matches the user's language order,
    /// in the form Terminal.app uses, for example zh_CN.UTF-8.
    nonisolated static func utf8Locale(preferred: [String] = Locale.preferredLanguages,
                                       region: String? = Locale.current.region?.identifier,
                                       installed: (String) -> Bool = {
                                           FileManager.default.fileExists(atPath: "/usr/share/locale/\($0)")
                                       }) -> String {
        for identifier in preferred {
            let language = Locale.Language(identifier: identifier)
            guard let code = language.languageCode?.identifier else { continue }
            let script = language.script?.identifier
            var regions = [language.region?.identifier, region].compactMap { $0 }
            if code == "zh" { regions.append(script == "Hant" ? "TW" : "CN") }
            for candidate in regions.map({ "\(code)_\($0).UTF-8" }) where installed(candidate) {
                return candidate
            }
        }
        return "en_US.UTF-8"
    }

    fileprivate func didTerminate(exitCode: Int32?) { processTerminated(exitCode: exitCode) }
    fileprivate func didFailToStart() { processFailed() }

    nonisolated private static func signalProcessGroup(_ pid: pid_t, signal: Int32) {
        // forkpty creates a session for the child on macOS. Signal the process
        // group first so programs launched by the shell do not become orphans;
        // also signal the PID as a fallback for shells that changed groups.
        _ = kill(-pid, signal)
        _ = kill(pid, signal)
    }
}

// Used only by TerminalInspection, which runs behind explicit check arguments.
extension TerminalSession {
    var inspectionShellPid: pid_t { view?.process.running == true ? view?.process.shellPid ?? 0 : 0 }
    var inspectionViewAttached: Bool { view?.window != nil }
    func inspectionSend(_ text: String) {
        view?.send(Array(text.utf8))
    }
}

@MainActor
final class HortusTerminalView: LocalProcessTerminalView {
    let delegateProxy: DelegateProxy

    init(frame: CGRect = .zero, session: TerminalSession) {
        delegateProxy = DelegateProxy(session: session)
        super.init(frame: frame)
        processDelegate = delegateProxy
        applyNativeColors()
        caretViewTracksFocus = true
        cursorBlinkResetsOnInput = true
        suspendsRenderingWhenNotVisible = true
        translatesAutoresizingMaskIntoConstraints = false
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func applyAppearance(fontSize: Double) {
        font = NSFont.monospacedSystemFont(ofSize: CGFloat(fontSize), weight: .regular)
        applyNativeColors()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyNativeColors()
    }

    private func applyNativeColors() {
        // SwiftTerm converts the foreground to RGB outside AppKit drawing.
        // Resolve it with this view's appearance, which can differ from the OS.
        // The background must be the real panel color too: SwiftTerm reports it for OSC 11,
        // uses it for reverse video and derives the 256-color ramp from it. `.clear` is
        // black with zero alpha, so programs saw a black terminal. Only the alpha keeps
        // the view transparent over the glass panel.
        effectiveAppearance.performAsCurrentDrawingAppearance {
            nativeForegroundColor = NSColor.labelColor.usingColorSpace(.sRGB) ?? .labelColor
            nativeBackgroundColor = NSColor.textBackgroundColor.usingColorSpace(.sRGB) ?? .textBackgroundColor
        }
        backgroundOpacity = 0
    }

    final class DelegateProxy: NSObject, LocalProcessTerminalViewDelegate {
        weak var session: TerminalSession?

        init(session: TerminalSession) { self.session = session }

        func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) { }
        func setTerminalTitle(source: LocalProcessTerminalView, title: String) { }
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) { }
        func processTerminated(source: TerminalView, exitCode: Int32?) {
            session?.didTerminate(exitCode: exitCode)
        }
        func processFailedToStart(source: TerminalView, error: LocalProcessError) {
            session?.didFailToStart()
        }
    }
}

struct TerminalPanel: View {
    let session: TerminalSession
    let fontSize: Double

    var body: some View {
        ZStack {
            if session.root == nil {
                emptyState
            } else {
                TerminalRepresentable(session: session, fontSize: fontSize)
                    .clipShape(.rect(cornerRadius: 12, style: .continuous))
                if session.showsStatusOverlay {
                    exitedOverlay
                }
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(L10n.string("Terminal"))
    }

    private var emptyState: some View {
        VStack(spacing: 10) {
            Image(systemName: "terminal").font(.title2).foregroundStyle(.tertiary)
            Text(session.statusText).font(.callout).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var exitedOverlay: some View {
        VStack(spacing: 10) {
            Text(session.statusText).font(.callout).foregroundStyle(.secondary)
            Button(L10n.string("Restart")) { session.restart() }
                .buttonStyle(.borderedProminent)
                .disabled(session.state == .failed && session.root == nil)
        }
        .padding(18)
        .glassEffect(.regular, in: .rect(cornerRadius: 14, style: .continuous))
    }
}

private struct TerminalRepresentable: NSViewRepresentable {
    let session: TerminalSession
    let fontSize: Double

    func makeNSView(context: Context) -> HortusTerminalView {
        return session.makeView(fontSize: fontSize)
    }

    func updateNSView(_ nsView: HortusTerminalView, context: Context) {
        session.update(fontSize: fontSize)
        session.attach(nsView, fontSize: fontSize)
    }

    static func dismantleNSView(_ nsView: HortusTerminalView, coordinator: ()) {
        // The workspace owns the session and stops it when the vault or app
        // closes. Detaching here must not kill a shell during a SwiftUI redraw.
    }
}
