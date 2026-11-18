#if DEBUG
import AppKit
import CoreText
import Darwin

/// 设置窗口自检通过这个桥读取 SwiftUI `Settings` 场景里的同一个模型。
@MainActor
enum SettingsInspectionBridge {
    static weak var current: SettingsModel?
}

/// 真实设置窗口与双窗口终端：保注释保存、冲突保护、即时传播、指南与截图。
@MainActor
enum SettingsInspection {
    private static var stage = "start"

    private static func argument(_ name: String, _ arguments: [String]) -> String? {
        arguments.first { $0.hasPrefix("--\(name)=") }
            .map { String($0.dropFirst(name.count + 3)) }
    }

    private static func require<T>(_ value: T?) throws -> T {
        guard let value else { throw CocoaError(.coderValueNotFound) }
        return value
    }

    private static func wait(seconds: Double = 20, _ condition: () -> Bool) async throws {
        let deadline = ProcessInfo.processInfo.systemUptime + seconds
        while !condition() {
            guard ProcessInfo.processInfo.systemUptime < deadline else {
                throw NSError(domain: "Hortus.SettingsInspection", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "timeout at \(stage)"])
            }
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    private static func settingsMenuItem(in menu: NSMenu) -> NSMenuItem? {
        for item in menu.items {
            if item.keyEquivalent == ",", item.keyEquivalentModifierMask.contains(.command), !item.isSeparatorItem {
                return item
            }
            if let submenu = item.submenu, let found = settingsMenuItem(in: submenu) {
                return found
            }
        }
        return nil
    }

    private static func openSettingsWindow(excluding existing: Set<ObjectIdentifier>) async throws -> NSWindow {
        stage = "open settings"
        _ = NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
        try await Task.sleep(for: .milliseconds(400))
        if settingsWindow(excluding: existing) == nil,
           let item = NSApp.mainMenu.flatMap(settingsMenuItem(in:)), let action = item.action {
            NSApp.sendAction(action, to: item.target, from: item)
        }
        try await wait { settingsWindow(excluding: existing) != nil }
        let window = try require(settingsWindow(excluding: existing))
        window.makeKeyAndOrderFront(nil)
        return window
    }

    private static func settingsWindow(excluding existing: Set<ObjectIdentifier>) -> NSWindow? {
        NSApp.windows.first {
            $0.isVisible && $0.styleMask.contains(.titled)
                && !existing.contains(ObjectIdentifier($0)) && !($0.delegate is VaultWindowDelegate)
        }
    }

    private static func screen(_ workspace: Workspace) -> String {
        guard let tab = workspace.currentTerminalTab ?? workspace.terminals.shellTabs.first,
              let view = tab.session.inspectionView else { return "" }
        return view.visibleRowsText(0..<view.terminalDimensions.rows).joined(separator: "\n")
    }

    private static func terminalView(_ workspace: Workspace) -> HortusTerminalView? {
        (workspace.currentTerminalTab ?? workspace.terminals.shellTabs.first)?.session.inspectionView
    }

    private static func nerdGlyph(in catalog: TerminalNerdFonts) -> (scalar: UnicodeScalar, text: String)? {
        let candidates: [UInt32] = [0xE6B2, 0xE8EB, 0xF121, 0xF00C, 0xE700, 0xE70B]
        for family in catalog.families {
            let descriptor = CTFontDescriptorCreateWithAttributes(
                [kCTFontFamilyNameAttribute: family] as CFDictionary)
            let font = CTFontCreateWithFontDescriptor(descriptor, 15, nil)
            for value in candidates {
                guard let scalar = UnicodeScalar(value) else { continue }
                var characters = [UniChar(value)]
                var glyphs = [CGGlyph](repeating: 0, count: 1)
                if CTFontGetGlyphsForCharacters(font, &characters, &glyphs, 1), glyphs[0] != 0 {
                    return (scalar, String(scalar))
                }
            }
        }
        return nil
    }

    static func run(application: ApplicationWorkspace, workspace: Workspace, output: String) async {
        do {
            let arguments = ProcessInfo.processInfo.arguments
            stage = "wait for vaults"
            if let second = argument("settings-second-vault", arguments) {
                await workspace.openFolder(second)
            }
            try await wait { !application.records.isEmpty && application.records.allSatisfy { $0.workspace.root != nil } }
            try await wait(seconds: 10) { application.ready }

            let existing = Set(NSApp.windows.map { ObjectIdentifier($0) })
            let window = try await openSettingsWindow(excluding: existing)
            try await wait { SettingsInspectionBridge.current != nil && SettingsInspectionBridge.current?.loading == false }
            let model = try require(SettingsInspectionBridge.current)
            var result: [String: Any] = [:]
            result["config_path"] = model.configPath
            result["settings_window_is_vault"] = window.delegate is VaultWindowDelegate
            result["vault_windows_before"] = NSApp.windows.filter { $0.isVisible && $0.delegate is VaultWindowDelegate }.count
            result["diagnostics"] = model.diagnostics
            result["font_families"] = model.fontFamilies.count

            // A1：登录 PATH 注入后只读发现模拟 Agent；陷阱文件从未被执行。
            if argument("settings-login-path", arguments) != nil {
                stage = "discover agents"
                try await wait { model.agentScanComplete }
                result["detected_agents"] = model.detectedAgents.map { agent in
                    ["id": agent.id,
                     "executable": agent.executable,
                     "args": agent.suggestedArgs]
                }
                if let marker = argument("settings-trap-marker", arguments) {
                    result["trap_executed"] = FileManager.default.fileExists(atPath: marker)
                }
                let omp = try require(model.detectedAgents.first { $0.id == "omp" })
                let beforeFill = try String(contentsOfFile: model.configPath, encoding: .utf8)
                model.useAgent(omp)
                try await Task.sleep(for: .milliseconds(100))
                let afterFill = try String(contentsOfFile: model.configPath, encoding: .utf8)
                result["fill_command"] = model.agentCommand
                result["fill_args"] = model.agentArgs
                result["fill_selected"] = model.isAgentSelected(omp)
                result["fill_without_save"] = beforeFill == afterFill
                await model.save()
                let afterSave = try String(contentsOfFile: model.configPath, encoding: .utf8)
                result["saved_agent_executable"] = afterSave.contains("command = \"\(omp.executable)\"")
                result["saved_agent_args"] = afterSave.contains("--append-system-prompt={guide}")
                result["saved_vaults_after_fill"] = afterSave.matches(of: /\n\[\[vault\]\]/).count == 2
            }

            // A1：打开后外部手改，普通保存被拒绝；重载后成功且注释/未知键/[[vault]] 保留。
            stage = "external conflict"
            let path = model.configPath
            let original = try String(contentsOfFile: path, encoding: .utf8)
            let manual = original
                .replacingOccurrences(of: "font_size = 15.0", with: "font_size = 16.0")
                + "\n# external manual edit\n"
            try Data(manual.utf8).write(to: URL(fileURLWithPath: path))
            model.fontSize = 18.0
            model.defaultView = "editing"
            await model.save()
            result["conflict_detected"] = model.conflict
            result["conflict_status_is_error"] = model.statusIsError
            let refused = try String(contentsOfFile: path, encoding: .utf8)
            result["external_edit_preserved"] = refused.contains("# external manual edit")
                && refused.contains("font_size = 16.0")

            await model.load()
            let chosenFont = model.fontFamilies.first {
                !$0.localizedCaseInsensitiveContains("Nerd")
                    && TerminalFonts.installedFont(family: $0, size: 18) != nil
            } ?? model.fontFamilies.first
            model.fontSize = 18.0
            model.defaultView = "editing"
            model.terminalFont = chosenFont
            await model.save()
            guard !model.conflict, !model.statusIsError else {
                throw NSError(domain: "Hortus.SettingsInspection", code: 2,
                              userInfo: [NSLocalizedDescriptionKey: model.statusMessage ?? "save failed"])
            }
            result["terminal_font"] = chosenFont ?? ""
            let saved = try String(contentsOfFile: path, encoding: .utf8)
            result["comments_preserved"] = saved.contains("# 顶部说明 top comment")
                && saved.contains("# 字号 inline")
                && saved.contains("# vault 覆盖行尾注释 keep")
            result["unknown_preserved"] = saved.contains("unknown = { keep = [1, 2] }")
            result["vault_preserved"] = saved.matches(of: /\n\[\[vault\]\]/).count == 2
            result["saved_font_size"] = saved.contains("font_size = 18.0")
            result["saved_view"] = saved.contains("default_view = \"editing\"")
            result["saved_font"] = chosenFont.map { saved.contains("terminal_font = \"\($0)\"") } ?? false

            // A3：双窗口终端字号/字体即时变化，PID 与缓冲区保持，Nerd Font 提供者仍在。
            stage = "start terminals"
            for record in application.records { record.workspace.showTerminal() }
            try await wait(seconds: 30) {
                application.records.allSatisfy {
                    $0.workspace.terminalPid > 0
                        && ($0.workspace.currentTerminalTab ?? $0.workspace.terminals.shellTabs.first)?
                            .session.inspectionViewAttached == true
                }
            }
            for record in application.records {
                if let tab = record.workspace.currentTerminalTab ?? record.workspace.terminals.shellTabs.first {
                    tab.session.inspectionSend("printf 'hortus-settings-marker\\n'\r")
                }
            }
            try await wait { application.records.allSatisfy { screen($0.workspace).contains("hortus-settings-marker") } }
            let pidsBefore = application.records.map(\.workspace.terminalPid)
            let providers = application.records.map { record in
                terminalView(record.workspace).flatMap { view in
                    view.glyphFallbackProvider.map { ObjectIdentifier($0 as AnyObject) }
                }
            }
            let buffers = application.records.map { screen($0.workspace) }

            model.fontSize = 20.0
            if let chosenFont { model.terminalFont = chosenFont }
            await model.save()
            try await wait { application.records.allSatisfy { $0.workspace.fontSize == 20.0 } }
            let pidsAfter = application.records.map(\.workspace.terminalPid)
            result["pids_before"] = pidsBefore.map(Int.init)
            result["pids_after"] = pidsAfter.map(Int.init)
            result["pids_stable"] = pidsBefore == pidsAfter
            result["font_size_propagated"] = application.records.allSatisfy {
                $0.workspace.fontSize == 20 && $0.workspace.terminals.shellTabs.allSatisfy {
                    $0.session.inspectionView?.font.pointSize == 20
                }
            }
            if let chosenFont {
                let expected = TerminalFonts.installedFont(family: chosenFont, size: 20)?.fontName
                result["font_name"] = expected ?? ""
                result["font_propagated"] = application.records.allSatisfy {
                    $0.workspace.terminals.shellTabs.allSatisfy {
                        $0.session.inspectionView?.font.fontName == expected
                    }
                }
            }
            result["buffer_preserved"] = zip(buffers, application.records).allSatisfy {
                $0.contains("hortus-settings-marker") || $1.workspace.terminalPid == 0
            }
            let providersAfter = application.records.map { record in
                terminalView(record.workspace).flatMap { view in
                    view.glyphFallbackProvider.map { ObjectIdentifier($0 as AnyObject) }
                }
            }
            result["nerdfont_provider_preserved"] = providers == providersAfter

            stage = "nerd glyph"
            var glyphResult: [String: Any] = ["checked": false]
            if let first = application.records.first,
               let session = (first.workspace.currentTerminalTab ?? first.workspace.terminals.shellTabs.first)?.session,
               let catalog = session.nerdFonts, !catalog.families.isEmpty,
               let view = terminalView(first.workspace), let glyph = nerdGlyph(in: catalog) {
                view.feed(text: "\r\n\(glyph.text)\r\n")
                try await Task.sleep(for: .milliseconds(250))
                let cells = TerminalFontInspection.cells(in: view)
                let codepoint = String(glyph.scalar.value, radix: 16, uppercase: true)
                let icons = cells.filter { ($0["codepoint"] as? String) == codepoint }
                glyphResult = [
                    "checked": true,
                    "codepoint": codepoint,
                    "cells": icons.count,
                    "provider": icons.allSatisfy { $0["provider"] as? Bool == true },
                    "fonts": Array(Set(icons.compactMap { $0["font"] as? String })),
                ]
            }
            result["nerdfont_glyph"] = glyphResult

            // A3：保存后的默认视图只影响之后打开的笔记。
            stage = "default view"
            workspace.openNote("second.md")
            try await wait { workspace.current?.path == "second.md" && workspace.current?.document != nil }
            result["default_view_applied"] = workspace.current?.document?.reading == false
            result["default_view"] = workspace.defaultView

            // A3：新指南与启动模板用于下一次 Agent。
            stage = "guide"
            let guideText = "Settings check guide\nargv follows verbatim\n"
            model.guideText = guideText
            await model.saveGuide()
            result["guide_overridden"] = model.guideOverridden
            result["guide_chars"] = model.guideText.count
            if let agent = argument("settings-agent", arguments) {
                model.agentCommand = agent
                model.agentArgs = ["--guide", "{guide}", "--sleep=0"]
                await model.save()
                model.scheduleProbe()
                try await wait { model.probe?.executable?.hasSuffix("fake-agent.sh") == true }
                result["probe_available"] = model.probe?.available == true
                result["probe_executable"] = model.probe?.executable ?? ""
                workspace.addAgent()
                if let scratch = ProcessInfo.processInfo.environment["FAKE_AGENT_SCRATCH"] {
                    let marker = URL(fileURLWithPath: scratch).appendingPathComponent("argc")
                    try await wait(seconds: 30) { FileManager.default.fileExists(atPath: marker.path) }
                    let agentGuide = try String(contentsOfFile: URL(fileURLWithPath: scratch).appendingPathComponent("arg-2").path, encoding: .utf8)
                    result["agent_guide_received"] = agentGuide == guideText
                    result["agent_args_received"] = try String(contentsOfFile: URL(fileURLWithPath: scratch).appendingPathComponent("arg-3").path, encoding: .utf8)
                    let receivedPath = try String(contentsOfFile: URL(fileURLWithPath: scratch).appendingPathComponent("path").path, encoding: .utf8)
                    result["agent_path_received"] = receivedPath
                    if let login = argument("settings-login-path", arguments) {
                        result["agent_path_has_login"] = receivedPath.split(separator: ":").contains(Substring(login))
                        result["agent_path_has_homebrew"] = receivedPath.split(separator: ":").contains("/opt/homebrew/bin")
                    }
                }
            }

            // 截图：设置窗口本身，不包含桌面。
            stage = "screenshots"
            if let media = argument("settings-media", arguments),
               let label = argument("settings-label", arguments) {
                // 截图展示已填入并保存后的状态，页脚与表单保持一致。
                if let omp = model.detectedAgents.first(where: { $0.id == "omp" }) {
                    model.useAgent(omp)
                    await model.save()
                }
                let directory = URL(fileURLWithPath: media)
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                var shots: [String] = []
                for (tab, name) in [(SettingsModel.Tab.general, "general"),
                                    (.terminal, "terminal"), (.agent, "agent")] {
                    model.tab = tab
                    try await Task.sleep(for: .milliseconds(250))
                    window.makeKeyAndOrderFront(nil)
                    let file = directory.appendingPathComponent("settings-\(label)-\(name).png")
                    try await AppDelegate.snapshot(to: file.path, window: window)
                    shots.append(file.path)
                }
                result["screenshots"] = shots
            }

            // A2：设置窗口不是库窗口；关闭它不影响多窗口管理。
            stage = "close settings"
            window.performClose(nil)
            try await wait { !window.isVisible }
            try await Task.sleep(for: .milliseconds(150))
            result["settings_closed"] = !window.isVisible
            result["records_after_close"] = application.records.count
            result["vault_windows_after_close"] = NSApp.windows.filter {
                $0.isVisible && $0.delegate is VaultWindowDelegate
            }.count

            let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: URL(fileURLWithPath: output))
            NSApp.terminate(nil)
        } catch {
            fputs("Hortus settings inspection failed at \(stage): \(error)\n", stderr)
            exit(1)
        }
    }
}
#endif
