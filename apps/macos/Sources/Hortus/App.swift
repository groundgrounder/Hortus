import SwiftUI
import Darwin
import ScreenCaptureKit
import CoreGraphics

@main
struct HortusApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var layout = Layout()
    @State private var workspace = Workspace()

    var body: some Scene {
        Window("Hortus", id: "main") {
            ShellView()
                .environment(layout).environment(workspace)
                .frame(minWidth: 900, minHeight: 560)
                .task {
                    delegate.workspace = workspace
                    delegate.layout = layout
                    await workspace.start(layout: layout)
                    let args = ProcessInfo.processInfo.arguments
                    if args.contains("--reading-wait-for-scan") || args.contains("--search-wait-for-scan") {
                        while workspace.scanning { try? await Task.sleep(for: .milliseconds(10)) }
                    }
                    for arg in args where arg.hasPrefix("--note=") {
                        workspace.openNote(String(arg.dropFirst("--note=".count)))
                    }
                    if let arg = args.first(where: { $0.hasPrefix("--quick-open-query=") }) {
                        workspace.presentQuickOpen()
                        workspace.updateQuickOpen(query: String(arg.dropFirst("--quick-open-query=".count)), marked: false)
                    }
                    if let arg = args.first(where: { $0.hasPrefix("--search-query=") }) {
                        workspace.showSearchPanel()
                        workspace.updateSearch(query: String(arg.dropFirst("--search-query=".count)), marked: false)
                    }
                    if args.contains("--search-wait-for-scan") { await workspace.waitForSearchTasks() }
                    if args.contains("--inspect-hide-left") { layout.visible[.left] = false }
                    if let arg = args.first(where: { $0.hasPrefix("--inspect-left-width=") }),
                       let width = Double(arg.dropFirst("--inspect-left-width=".count)) { layout.size[.left] = width }
                    delegate.ready = true
                    if let arg = args.first(where: { $0.hasPrefix("--check-vault-lifecycle=") }) {
                        await WorkspaceInspection.checkLifecycle(workspace, output: String(arg.dropFirst("--check-vault-lifecycle=".count)))
                    }
                    delegate.inspectWhenReady()
                }
                .alert("Error", isPresented: Binding(get: { workspace.error != nil }, set: { if !$0 { workspace.error = nil } })) {
                    Button("OK") { workspace.error = nil }
                } message: { Text(workspace.error ?? "") }
        }
        .defaultSize(width: 1320, height: 840)
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands {
            CommandGroup(replacing: .appInfo) {
                Button("About Hortus") { delegate.showAboutPanel() }
            }
            CommandGroup(replacing: .newItem) {
                Button("Open Folder…") { workspace.chooseFolder() }.keyboardShortcut("o", modifiers: .command)
                Menu("Recent") {
                    if workspace.recent.isEmpty { Text("No recently opened folders") }
                    ForEach(workspace.recent) { recent in
                        Button(recent.path) { Task { await workspace.openFolder(recent.path) } }
                    }
                }
                Divider()
                Button("Close Tab") { workspace.closeTab() }.keyboardShortcut("w", modifiers: .command)
                    .disabled(workspace.selected == nil)
            }
            CommandGroup(replacing: .saveItem) {
                Button("Save") { workspace.saveCurrent() }.keyboardShortcut("s", modifiers: .command)
                    .disabled(workspace.current?.document == nil)
            }
            CommandGroup(after: .textEditing) {
                Button("Align Table Source") { NSApp.sendAction(#selector(DiagramTextView.alignTableSource(_:)), to: nil, from: nil) }
                    .keyboardShortcut("t", modifiers: [.option, .command])
                    .disabled(workspace.current?.document?.reading != false)
                Button("Find…") { find(.showFindInterface) }.keyboardShortcut("f", modifiers: .command)
                Button("Find Next") { find(.nextMatch) }.keyboardShortcut("g", modifiers: .command)
                Button("Find Previous") { find(.previousMatch) }.keyboardShortcut("g", modifiers: [.shift, .command])
                Button("Find and Replace…") { find(.showReplaceInterface) }.keyboardShortcut("f", modifiers: [.option, .command])
                Divider()
                Button(L10n.string("Quick Open")) { workspace.presentQuickOpen() }
                    .keyboardShortcut("p", modifiers: .command)
                Button(L10n.string("Search Notes")) { workspace.showSearchPanel() }
                    .keyboardShortcut("f", modifiers: [.shift, .command])
            }
            CommandGroup(after: .toolbar) {
                Button(workspace.current?.document?.reading == true ? "Switch to Editing" : "Switch to Reading") { workspace.toggleView() }
                    .keyboardShortcut("e", modifiers: .command).disabled(workspace.current?.document == nil)
            }
            CommandMenu("View") {
                Toggle(L10n.string("Show Terminal"), isOn: Binding(
                    get: { layout.terminalVisible },
                    set: { shown in
                        layout.setTerminalVisible(shown)
                        if shown { DispatchQueue.main.async { workspace.terminal.focus() } }
                    }))
                    .keyboardShortcut("`", modifiers: .control)
                Button(L10n.string("Clear Terminal")) { workspace.terminal.clearScreen() }
                    .keyboardShortcut("k", modifiers: .command)
                    .disabled(workspace.terminal.state != .running)
            }
            CommandMenu("Panels") {
                Toggle("Left Sidebar", isOn: shown(.left)).keyboardShortcut("s", modifiers: [.control, .command])
                Toggle("Right Sidebar", isOn: shown(.right)).keyboardShortcut("i", modifiers: [.option, .command])
                Toggle("Bottom Panel", isOn: shown(.bottom)).keyboardShortcut("j", modifiers: .command)
                Divider()
                Button("Reset Layout") { layout.reset() }
            }
        }
    }

    private func find(_ action: NSTextFinder.Action) {
        let item = NSMenuItem(); item.tag = action.rawValue
        NSApp.sendAction(#selector(NSTextView.performFindPanelAction(_:)), to: nil, from: item)
    }
    private func shown(_ region: Region) -> Binding<Bool> {
        Binding(get: { layout.visible[region] ?? true }, set: { layout.visible[region] = $0 })
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    var workspace: Workspace?
    var layout: Layout?
    var ready = false
    private let termination = TerminationGate()
    /// 用户已明确放弃未保存的正文；之后的 terminate 不再拦截。
    private var allowTermination = false
    private var inspectionDeadline: DispatchWorkItem?
    private var inspectionStarted = false
    private var launchTime = ContinuousClock.now
    private var focusObserver: NSObjectProtocol?

    func showAboutPanel() {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "0.1.0"
        let build = info["CFBundleVersion"] as? String ?? ""
        // 居中和标题对齐；显式用 labelColor，否则深色模式下默认黑字看不清
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = .center
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize),
            .foregroundColor: NSColor.labelColor,
            .paragraphStyle: paragraph,
        ]
        let repository = "https://github.com/groundgrounder/Hortus"
        let credits = NSMutableAttributedString(
            string: "\(L10n.string("A native Markdown knowledge base for people and agents."))\n\n\(L10n.string("GPL-3.0-only application; MIT OR Apache-2.0 core."))\n",
            attributes: attributes)
        var link = attributes
        link[.link] = URL(string: repository)
        credits.append(NSAttributedString(string: repository, attributes: link))
        // 包里的许可证原文和第三方许可证声明，点开用系统默认的文本应用查看
        let files = [("License", "LICENSE"), ("Third-Party Licenses", "ThirdPartyNotices")].compactMap { title, name in
            Bundle.main.url(forResource: name, withExtension: "txt").map { (L10n.string(title), $0) }
        }
        for (index, (title, url)) in files.enumerated() {
            credits.append(NSAttributedString(string: index == 0 ? "\n" : "  ·  ", attributes: attributes))
            var file = attributes
            file[.link] = url
            credits.append(NSAttributedString(string: title, attributes: file))
        }
        NSApp.orderFrontStandardAboutPanel(options: [
            .applicationName: "Hortus",
            .applicationVersion: "v\(version)",
            .version: build,
            .credits: credits,
        ])
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        let args = ProcessInfo.processInfo.arguments
        if args.contains("--dark") { NSApp.appearance = NSAppearance(named: .darkAqua) }
        if args.contains("--light") { NSApp.appearance = NSAppearance(named: .aqua) }
        if args.contains("--simulate-high-contrast") { NSApp.appearance = NSAppearance(named: .aqua) }
        NSApp.activate()
        focusObserver = NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in await self?.workspace?.saveAllDocuments() }
        }
        guard args.contains(where: { $0.hasPrefix("--reading-inspection=") || $0.hasPrefix("--snapshot=") || $0.hasPrefix("--about-snapshot=") || $0.hasPrefix("--metrics=") || $0.hasPrefix("--search-performance=") || $0.hasPrefix("--edit-inspection=") || $0.hasPrefix("--diagram-inspection=") || $0.hasPrefix("--toolbar-inspection=") || $0.hasPrefix("--terminal-inspection=") || $0.hasPrefix("--terminal-idle=") || ($0 == "--inspect-fullscreen" || $0 == "--inspect-fullscreen-top") }) else { return }
        // 自检只设置两次一次性唤醒，其余等待由界面状态变更触发。
        let deadline = DispatchWorkItem { fputs("Hortus 自检超时\n", stderr); exit(2) }
        inspectionDeadline = deadline
        DispatchQueue.main.asyncAfter(deadline: .now() + 15, execute: deadline)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.inspectWhenReady() }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let workspace else { return .terminateNow }
        if allowTermination { return .terminateNow }
        termination.begin(
            documentTitles: workspace.tabs.map(\.path),
            documents: { completion in workspace.prepareShutdown(completion: completion) },
            cleanup: { completion in workspace.beginShutdownCleanup(completion: completion) }
        ) { [weak self] outcome in
            switch outcome {
            case .allowed:
                sender.reply(toApplicationShouldTerminate: true)
            case .blocked(let unsaved):
                sender.reply(toApplicationShouldTerminate: false)
                // 必须先答复 false；terminateLater 的嵌套事件循环不调度 MainActor 任务。
                DispatchQueue.main.async { [weak self] in
                    self?.handleBlockedTermination(unsaved, workspace: workspace)
                }
            }
        }
        return .terminateLater
    }

    private func handleBlockedTermination(_ unsaved: [String], workspace: Workspace) {
        switch workspace.decideUnsaved(unsaved, quitting: true) {
        case .retry:
            NSApp.terminate(nil)
        case .discard:
            // 用户明确放弃：清理这些文档的恢复稿，然后不再拦截后续 terminate。
            workspace.abandonDocuments(unsaved) { NSApp.terminate(nil) }
            allowTermination = true
        case .cancel:
            break
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func inspectWhenReady() {
        guard inspectionDeadline != nil, !inspectionStarted, let workspace else { return }
        let visualInspection = ProcessInfo.processInfo.arguments.contains {
            $0.hasPrefix("--reading-inspection=") || $0.hasPrefix("--snapshot=") || $0.hasPrefix("--edit-inspection=") || $0.hasPrefix("--diagram-inspection=") || $0.hasPrefix("--toolbar-inspection=") || $0.hasPrefix("--quick-open-query=") || $0.hasPrefix("--search-query=") || $0.hasPrefix("--search-performance=") || $0.hasPrefix("--terminal-inspection=") || $0.hasPrefix("--terminal-idle=") || $0 == "--inspect-fullscreen" || $0 == "--inspect-fullscreen-top"
        }
        let settled = withObservationTracking {
            ready && !workspace.scanning && !workspace.opening && !workspace.tabs.contains(where: \.loading)
                && (visualInspection || workspace.root == nil || workspace.entries.isEmpty || workspace.scanDisplayMilliseconds != nil)
        } onChange: { [weak self] in
            Task { @MainActor in self?.inspectWhenReady() }
        }
        let diagramViews = NSApp.windows.compactMap(\.contentView).flatMap { DiagramInspection.textViews(in: $0) }
        guard !diagramViews.contains(where: \.diagramPending) else { return }
        guard settled, launchTime.duration(to: .now) >= .seconds(1) else { return }
        inspectionStarted = true
        inspectionDeadline?.cancel()
        Task { await finishInspection() }
    }

    private func finishInspection() async {
        guard let workspace else { return }
        let args = ProcessInfo.processInfo.arguments
        do {
            if let arg = args.first(where: { $0.hasPrefix("--reading-inspection=") }) {
                let result = try await ReadingInspection.run(workspace: workspace, arguments: args)
                let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                let path = String(arg.dropFirst("--reading-inspection=".count))
                try await Task.detached { try data.write(to: URL(fileURLWithPath: path)) }.value
                if args.contains("--inspect-hold-open") { return }
            }
            if let arg = args.first(where: { $0.hasPrefix("--edit-inspection=") }) {
                let result = try await EditingInspection.run(workspace: workspace, arguments: args)
                let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                let path = String(arg.dropFirst("--edit-inspection=".count))
                try await Task.detached { try data.write(to: URL(fileURLWithPath: path)) }.value
                if args.contains("--inspect-hold-open") { return }
            }
            if let arg = args.first(where: { $0.hasPrefix("--terminal-inspection=") }), let layout {
                let result = try await TerminalInspection.run(workspace: workspace, layout: layout, arguments: args)
                let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                try data.write(to: URL(fileURLWithPath: String(arg.dropFirst("--terminal-inspection=".count))))
            }
            if let arg = args.first(where: { $0.hasPrefix("--terminal-idle=") }), let layout {
                let result = try await TerminalInspection.idle(workspace: workspace, layout: layout, arguments: args)
                if let shot = args.first(where: { $0.hasPrefix("--snapshot=") }) {
                    try await Task.sleep(for: .milliseconds(300))
                    try await Self.snapshot(to: String(shot.dropFirst("--snapshot=".count)))
                }
                let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                try data.write(to: URL(fileURLWithPath: String(arg.dropFirst("--terminal-idle=".count))))
                return
            }
            if let arg = args.first(where: { $0.hasPrefix("--diagram-inspection=") }) {
                let result = try await DiagramInspection.run(workspace: workspace, arguments: args)
                let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                let path = String(arg.dropFirst("--diagram-inspection=".count))
                try await Task.detached { try data.write(to: URL(fileURLWithPath: path)) }.value
            }
            if (args.contains("--inspect-fullscreen") || args.contains("--inspect-fullscreen-top")),
               let window = NSApp.windows.first(where: \.isVisible) {
                window.toggleFullScreen(nil)
                try await Task.sleep(for: .seconds(3))
                let bounds = CGDisplayBounds(CGMainDisplayID())
                let location = args.contains("--inspect-fullscreen-top")
                    ? CGPoint(x: bounds.midX, y: bounds.minY + 2)
                    : CGPoint(x: bounds.midX, y: bounds.midY)
                CGWarpMouseCursorPosition(location)
                CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: location,
                        mouseButton: .left)?.post(tap: .cghidEventTap)
                try await Task.sleep(for: .seconds(2))
            }
            if let arg = args.first(where: { $0.hasPrefix("--toolbar-inspection=") }), let layout,
               let window = NSApp.windows.first(where: \.isVisible) {
                var result = ToolbarInspection.measure(window: window, layout: layout)
                if args.contains("--inspect-resize") {
                    result["resize_samples"] = try await ToolbarInspection.exerciseLayout(window: window, layout: layout)
                }
                let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                let path = String(arg.dropFirst("--toolbar-inspection=".count))
                try await Task.detached { try data.write(to: URL(fileURLWithPath: path)) }.value
                if args.contains("--inspect-hold") {
                    // 留出外部屏幕截图的时间；只在显式验收参数下运行。
                    try await Task.sleep(for: .seconds(3))
                }
            }
            if let arg = args.first(where: { $0.hasPrefix("--snapshot=") }) {
                if args.contains("--snapshot-end"), let view = DiagramInspection.current() {
                    view.scrollRangeToVisible(NSRange(location: view.sourceUTF16Length, length: 0))
                    view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
                }
                try await Self.snapshot(to: String(arg.dropFirst("--snapshot=".count)))
            }
            if let arg = args.first(where: { $0.hasPrefix("--about-snapshot=") }) {
                let existingWindows = Set(NSApp.windows.map(\.windowNumber))
                showAboutPanel()
                try await Task.sleep(for: .milliseconds(500))
                guard let about = NSApp.windows.first(where: { $0.isVisible && !existingWindows.contains($0.windowNumber) }) else {
                    throw CocoaError(.fileWriteUnknown)
                }
                try await Self.snapshot(to: String(arg.dropFirst("--about-snapshot=".count)),
                                        window: about)
                if args.contains("--inspect-hold-open") { return }
            }
            if let arg = args.first(where: { $0.hasPrefix("--metrics=") }) {
                var usage = rusage()
                getrusage(RUSAGE_SELF, &usage)
                let metrics: [String: Any] = [
                    "first_screen_ms": workspace.firstDrawMilliseconds ?? 0,
                    "scan_complete_ms": Double(workspace.scanMicros) / 1000,
                    "scan_display_ms": workspace.scanDisplayMilliseconds ?? 0,
                    "peak_rss_bytes": usage.ru_maxrss,
                    "entries": workspace.entries.count,
                    "notes": workspace.entries.values.filter { !$0.directory }.count,
                    "folders": workspace.entries.values.filter(\.directory).count,
                    "tree_version": workspace.treeVersion,
                    "current_characters": workspace.current?.text.count ?? 0,
                    "tabs": workspace.tabs.map(\.path),
                    "selected": workspace.selected ?? "",
                    "title": workspace.title,
                ]
                let data = try JSONSerialization.data(withJSONObject: metrics, options: [.prettyPrinted, .sortedKeys])
                let path = String(arg.dropFirst("--metrics=".count))
                try await Task.detached { try data.write(to: URL(fileURLWithPath: path)) }.value
            }
            if let arg = args.first(where: { $0.hasPrefix("--search-performance=") }) {
                let query = args.first(where: { $0.hasPrefix("--search-performance-query=") })
                    .map { String($0.dropFirst("--search-performance-query=".count)) } ?? "note"
                guard let samples = await workspace.searchPerformanceSamples(fullTextQuery: query),
                      let ready = workspace.searchIndexReadyMilliseconds,
                      let memory = workspace.searchIndexPeakRSSDeltaBytes else {
                    throw CocoaError(.fileReadUnknown)
                }
                let metrics: [String: Any] = [
                    "quick_open_ui_ms": samples.quickOpen,
                    "full_text_ui_ms": samples.fullText,
                    "index_ready_ms": ready,
                    "index_peak_rss_delta_bytes": memory,
                    "notes": workspace.entries.values.filter { !$0.directory }.count,
                    "folders": workspace.entries.values.filter(\.directory).count,
                ]
                let data = try JSONSerialization.data(withJSONObject: metrics, options: [.prettyPrinted, .sortedKeys])
                let path = String(arg.dropFirst("--search-performance=".count))
                try await Task.detached { try data.write(to: URL(fileURLWithPath: path)) }.value
                if args.contains("--inspect-hold-open") { return }
            }
            if args.contains("--inspect-slow-shutdown") { workspace.shutdownDelayForInspection = 10 }
            NSApp.terminate(nil)
        } catch { fputs("Hortus 自检失败：\(error)\n", stderr); exit(1) }
    }

    static func snapshot(to path: String, window: NSWindow? = nil) async throws {
        guard let window = window ?? NSApp.keyWindow ?? NSApp.windows.first(where: \.isVisible) else { throw CocoaError(.fileWriteUnknown) }
        // 只枚举本进程的窗口，无需申请屏幕录制权限；保留系统合成的玻璃效果。
        let content = try await SCShareableContent.currentProcess
        guard let source = content.windows.first(where: { $0.windowID == CGWindowID(window.windowNumber) }) else {
            throw CocoaError(.fileWriteUnknown)
        }
        let filter = SCContentFilter(desktopIndependentWindow: source)
        let configuration = SCStreamConfiguration()
        configuration.width = Int(filter.contentRect.width * CGFloat(filter.pointPixelScale))
        configuration.height = Int(filter.contentRect.height * CGFloat(filter.pointPixelScale))
        configuration.showsCursor = false
        configuration.ignoreGlobalClipSingleWindow = true
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
        let rep = NSBitmapImageRep(cgImage: image)
        guard let data = rep.representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
        try await Task.detached { try data.write(to: URL(fileURLWithPath: path)) }.value
    }
}
