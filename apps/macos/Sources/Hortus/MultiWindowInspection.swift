#if DEBUG
import AppKit
import Darwin
import ScreenCaptureKit

/// 显式检查参数下操作真实 NSWindow；正常运行不创建采样或计时任务。
@MainActor
enum MultiWindowInspection {
    private static var stage = "start"
    static func baseline(workspace: Workspace, output: String) async {
        do {
            guard let application = workspace.application else { throw CocoaError(.coderValueNotFound) }
            let args = ProcessInfo.processInfo.arguments
            let restoring = args.contains("--multi-window-restore")
            var result: [String: Any] = [:]
            if restoring {
                try await settled(application)
                result["restored"] = application.records.map(describe)
            } else {
                guard let argument = args.first(where: { $0.hasPrefix("--multi-window-second=") }),
                      let first = application.records.first else { throw CocoaError(.fileReadInvalidFileName) }
                let root = workspace.root
                let tabs = workspace.tabs.map(\.path)
                let secondPath = String(argument.dropFirst("--multi-window-second=".count))
                stage = "open second"
                await workspace.openFolder(secondPath)
                try await settled(application)
                guard let second = application.records.last, second !== first else { throw CocoaError(.coderValueNotFound) }
                stage = "load second document"
                second.workspace.openNote("same.md")
                try await settled(application)
                result["original_preserved"] = root == workspace.root && tabs == workspace.tabs.map(\.path)
                let firstDocument = try require(first.workspace.current?.document)
                let secondDocument = try require(second.workspace.current?.document)
                result["documents_isolated"] = firstDocument !== secondDocument && firstDocument.text != secondDocument.text
                let count = application.records.count
                stage = "deduplicate existing"
                await second.workspace.openFolder(root! + "/../a/.")
                result["existing_activated"] = application.records.count == count && application.active === first
                let alias = URL(fileURLWithPath: secondPath).deletingLastPathComponent().appendingPathComponent("alias-a")
                try FileManager.default.createSymbolicLink(atPath: alias.path, withDestinationPath: root!)
                await second.workspace.openFolder(alias.path)
                result["canonical_deduplicated"] = application.records.count == count && application.contexts.count == count && application.active === first
                result["before_layout"] = application.records.map(describe)
                let oldBSize = second.layout.size[.left]
                first.layout.moveToRegion(.outline, .left)
                first.layout.visible[.left] = false
                first.layout.size[.left] = 340
                result["groups_shared"] = first.layout.state.regions.mapValues(\.groups) == second.layout.state.regions.mapValues(\.groups)
                result["size_existing_unchanged"] = second.layout.size[.left] == oldBSize
                result["visible_isolated"] = second.layout.visible[.left] == true
                let thirdPath = URL(fileURLWithPath: secondPath).deletingLastPathComponent().appendingPathComponent("c").path
                stage = "open third"
                await second.workspace.openFolder(thirdPath)
                try await settled(application)
                let third = try require(application.records.last)
                result["new_size"] = third.layout.size[.left] ?? 0
                stage = "close third"
                third.window?.performClose(nil)
                try await wait { application.records.count == 2 && application.contexts.count == 2 }
                stage = "start terminals"
                for record in [first, second] { record.layout.setTerminalVisible(true) }
                try await wait { first.workspace.terminal.inspectionShellPid > 0 && second.workspace.terminal.inspectionShellPid > 0 }
                let aPID = first.workspace.terminal.inspectionShellPid, bPID = second.workspace.terminal.inspectionShellPid
                result["terminal_pids"] = [aPID, bPID]
                result["terminal_roots"] = [first.workspace.terminal.root ?? "", second.workspace.terminal.root ?? ""]
                first.workspace.presentQuickOpen()
                first.workspace.updateQuickOpen(query: "same", marked: false)
                result["quick_open_isolated"] = first.workspace.quickOpenVisible && !second.workspace.quickOpenVisible
                first.workspace.dismissQuickOpen()
                application.activate(second)
                try await Task.sleep(for: .milliseconds(150))
                updateMenus(NSApp.mainMenu)
                await Task.yield()
                let oldView = secondDocument.reading
                stage = "key window command"
                let performed = key("e", modifiers: [.command])
                try await wait { secondDocument.reading != oldView }
                result["key_window_command"] = performed && firstDocument.reading && !secondDocument.reading
                application.activate(first)
                result["background_terminal_preserved"] = aPID == first.workspace.terminal.inspectionShellPid && bPID == second.workspace.terminal.inspectionShellPid
                // 外部批次覆盖同名路径、创建、改名与删除；每库独立刷新。
                let aURL = URL(fileURLWithPath: root!)
                stage = "external refresh"
                let start = ProcessInfo.processInfo.systemUptime
                try Data("# A externally refreshed\n".utf8).write(to: aURL.appendingPathComponent("same.md"))
                try Data("# Added\n".utf8).write(to: aURL.appendingPathComponent("added.md"))
                try await wait { firstDocument.text.contains("externally refreshed") && first.workspace.entries["added.md"] != nil }
                result["external_update_ms"] = (ProcessInfo.processInfo.systemUptime - start) * 1000
                result["external_isolated"] = !secondDocument.text.contains("externally refreshed")
                stage = "external rename delete"
                let renameStart = ProcessInfo.processInfo.systemUptime
                try FileManager.default.moveItem(at: aURL.appendingPathComponent("added.md"), to: aURL.appendingPathComponent("renamed.md"))
                try await wait { first.workspace.entries["added.md"] == nil && first.workspace.entries["renamed.md"] != nil }
                try FileManager.default.removeItem(at: aURL.appendingPathComponent("renamed.md"))
                try await wait { first.workspace.entries["renamed.md"] == nil }
                result["external_rename_delete_ms"] = (ProcessInfo.processInfo.systemUptime - renameStart) * 1000
                await application.persistSession()
            }
            try await settled(application)
            result["visible_windows"] = application.records.filter { $0.window?.isVisible == true }.count
            result["windows"] = application.records.map(describe)
            result["contexts"] = application.contexts.values.map { ["root": $0.root, "handles": 1, "listeners": $0.listenerCount, "trees": 1, "indexes": 1, "windows": $0.windowCount] as [String: Any] }
            let session = application.snapshot()
            result["session"] = try JSONSerialization.jsonObject(with: JSONEncoder().encode(session))
            if let arg = args.first(where: { $0.hasPrefix("--multi-window-media=") }) {
                let directory = URL(fileURLWithPath: String(arg.dropFirst("--multi-window-media=".count)))
                for (index, record) in application.records.enumerated() {
                    if let window = record.window { try await AppDelegate.snapshot(to: directory.appendingPathComponent("window-\(index).png").path, window: window) }
                }
                try await snapshotWindows(application.records, to: directory.appendingPathComponent("two-windows.png"))
            }
            // 恢复证据取完后检查非最后窗口关闭：背景 shell 与文档继续可用。
            if restoring, let second = application.records.last, let first = application.records.first, second !== first {
                let aPID = first.workspace.terminal.inspectionShellPid
                let bPID = second.workspace.terminal.inspectionShellPid
                let weakWindow = second.window
                second.window?.performClose(nil)
                try await wait { application.records.count == 1 && application.contexts.count == 1 }
                result["closed_shell_pid"] = bPID
                result["other_shell_survived"] = aPID == first.workspace.terminal.inspectionShellPid && aPID > 0
                result["closed_window_hidden"] = weakWindow?.isVisible != true
                result["remaining_contexts"] = application.contexts.count
            }
            result["threads"] = try WorkspaceInspection.threadCount()
            let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: URL(fileURLWithPath: output))
            NSApp.terminate(nil)
        } catch { fputs("Hortus multi-window inspection failed at \(stage): \(error)\n", stderr); exit(1) }
    }

    private static func describe(_ record: WindowRecord) -> [String: Any] {
        ["id": record.id, "root": record.workspace.root ?? "", "window": record.window?.windowNumber ?? -1,
         "visible": record.window?.isVisible == true, "tabs": record.workspace.tabs.map(\.path),
         "selected": record.workspace.selected ?? "", "views": record.workspace.sessionViews(),
         "documents": record.workspace.tabs.compactMap { $0.document.map { String(describing: ObjectIdentifier($0)) } },
         "terminal_pid": record.workspace.terminal.inspectionShellPid,
         "terminal_root": record.workspace.terminal.root ?? "",
         "left_size": record.layout.size[.left] ?? 0,
         "left_visible": record.layout.visible[.left] ?? true,
         "frame": record.window.map { NSStringFromRect($0.frame) } ?? ""]
    }

    private static func snapshotWindows(_ records: [WindowRecord], to url: URL) async throws {
        let content = try await SCShareableContent.currentProcess
        let ids = Set(records.compactMap { $0.window.map { CGWindowID($0.windowNumber) } })
        guard let display = content.displays.first else { throw CocoaError(.fileWriteUnknown) }
        let filter = SCContentFilter(display: display, including: content.windows.filter { ids.contains($0.windowID) })
        let configuration = SCStreamConfiguration()
        configuration.width = display.width
        configuration.height = display.height
        configuration.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
        guard let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
        try data.write(to: url)
    }

    private static func require<T>(_ value: T?) throws -> T {
        guard let value else { throw CocoaError(.coderValueNotFound) }
        return value
    }
    private static func key(_ text: String, modifiers: NSEvent.ModifierFlags) -> Bool {
        guard let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers,
                                           timestamp: ProcessInfo.processInfo.systemUptime,
                                           windowNumber: NSApp.keyWindow?.windowNumber ?? 0, context: nil,
                                           characters: text, charactersIgnoringModifiers: text,
                                           isARepeat: false, keyCode: 14) else { return false }
        return NSApp.mainMenu?.performKeyEquivalent(with: event) ?? false
    }
    private static func updateMenus(_ menu: NSMenu?) {
        menu?.delegate?.menuNeedsUpdate?(menu!)
        menu?.update()
        for item in menu?.items ?? [] { updateMenus(item.submenu) }
    }
    private static func settled(_ application: ApplicationWorkspace) async throws {
        try await wait { application.records.allSatisfy { $0.window != nil && !$0.workspace.scanning && !$0.workspace.tabs.contains(where: \.loading) } }
        try await Task.sleep(for: .milliseconds(150))
    }
    private static func wait(_ condition: () -> Bool) async throws {
        for _ in 0..<500 {
            if condition() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        throw CocoaError(.coderValueNotFound)
    }
}
#endif
