import Foundation
import HortusApple
import HortusCore
import Testing
@testable import Hortus

@Suite("M10-2 Agent Profiles, CLI Helper & Scene SVG")
struct M10_2Tests {

    private func makeTempDir(_ name: String = #function) throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("hortus-m10-2-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func makeExecutable(at url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try "#!/bin/sh\nexit 0\n".write(to: url, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
    }

    // MARK: - 3.1 Agent Multi-Profile & Custom Command

    @Test func multiProfileSelectionAndDefaultSwitching() throws {
        let root = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: root) }
        let configDir = root.appendingPathComponent("config")
        let cacheDir = root.appendingPathComponent("cache")
        let codex = root.appendingPathComponent("bin/codex")
        let claude = root.appendingPathComponent("bin/claude")
        try makeExecutable(at: codex)
        try makeExecutable(at: claude)

        let core = try WorkspaceCore.inDirectories(config: configDir.path, cache: cacheDir.path)
        let before = try core.editableConfig()
        let input = EditableConfigInputInfo(
            fontSize: before.fontSize,
            defaultView: before.defaultView,
            terminalFont: before.terminalFont,
            agentCommand: nil,
            agentArgs: []
        )
        let profiles = [
            AgentProfileInfo(id: "codex", name: "Codex", command: codex.path, args: ["{guide}"]),
            AgentProfileInfo(id: "claude", name: "Claude Code", command: claude.path, args: ["--append-system-prompt", "{guide}"]),
        ]
        let saved = try core.saveEditableConfigWithProfiles(
            input: input,
            agentProfiles: profiles,
            agentDefaultProfile: "claude",
            expectedDigest: before.digest
        )
        #expect(saved.agentProfiles.count == 2)
        #expect(saved.agentDefaultProfile == "claude")

        let defaultSpec = try core.agentSpecForProfile(profileId: nil, loginPath: nil)
        #expect(URL(fileURLWithPath: defaultSpec.executable).resolvingSymlinksInPath().path == claude.resolvingSymlinksInPath().path)
        let codexSpec = try core.agentSpecForProfile(profileId: "codex", loginPath: nil)
        #expect(URL(fileURLWithPath: codexSpec.executable).resolvingSymlinksInPath().path == codex.resolvingSymlinksInPath().path)
    }

    @Test func customAgentLaunchUsesReadOnlyGuideFileAndSnapshotGuard() throws {
        let root = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: root) }
        let configDir = root.appendingPathComponent("config")
        let cacheDir = root.appendingPathComponent("cache")
        let vaultDir = root.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: vaultDir, withIntermediateDirectories: true)
        try "# Note\n\nBody.\n".write(
            to: vaultDir.appendingPathComponent("note.md"),
            atomically: true,
            encoding: .utf8
        )

        let core = try WorkspaceCore.inDirectories(config: configDir.path, cache: cacheDir.path)
        let launch = try core.prepareCustomAgentLaunch(command: "codex --model o3", loginPath: nil)
        #expect(launch.args == ["-lc", "codex --model o3"])
        // 严禁将多行指南拼入自由 Shell 命令行。
        #expect(!launch.args[1].contains("\n"))
        #expect(launch.guideFile.hasPrefix(cacheDir.path))
        #expect(!launch.guideFile.hasPrefix(vaultDir.path))

        let guideText = try String(contentsOfFile: launch.guideFile, encoding: .utf8)
        #expect(guideText.contains("Hortus"))
        let attrs = try FileManager.default.attributesOfItem(atPath: launch.guideFile)
        let perms = (attrs[.posixPermissions] as? NSNumber)?.intValue ?? 0
        #expect((perms & 0o222) == 0)

        // 历史去重并落缓存。
        _ = try core.recordCustomAgentCommand(command: "cmd-a")
        _ = try core.recordCustomAgentCommand(command: "cmd-b")
        let history = try core.recordCustomAgentCommand(command: "cmd-a")
        #expect(history == ["cmd-a", "cmd-b"])

        // 笔记库中不得写入任何指南或历史文件。
        let vaultContents = try FileManager.default.contentsOfDirectory(atPath: vaultDir.path)
        #expect(vaultContents == ["note.md"])
    }

    // MARK: - 3.2 CLI Helper, Symlink, PATH Append & MCP JSON

    @Test func cliSymlinkCreateUpdateAndConflictProtection() throws {
        let root = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: root) }
        let fakeApp = root.appendingPathComponent("Hortus.app", isDirectory: true)
        let helper = fakeApp.appendingPathComponent("Contents/Helpers/hortus")
        try makeExecutable(at: helper)
        let linkURL = root.appendingPathComponent("home/.local/bin/hortus")

        // 初始未安装
        #expect(CLIHelper.symlinkStatus(bundleURL: fakeApp, linkURL: linkURL) == .notInstalled)

        // 自动创建父目录并建立软链
        let installed = try CLIHelper.installSymlink(bundleURL: fakeApp, linkURL: linkURL)
        #expect(installed == linkURL)
        #expect(CLIHelper.symlinkStatus(bundleURL: fakeApp, linkURL: linkURL) == .linked)

        // 指向其他位置的软链可安全更新
        let otherTarget = root.appendingPathComponent("other/hortus")
        try makeExecutable(at: otherTarget)
        try FileManager.default.removeItem(at: linkURL)
        try FileManager.default.createSymbolicLink(atPath: linkURL.path, withDestinationPath: otherTarget.path)
        if case .linkedElsewhere = CLIHelper.symlinkStatus(bundleURL: fakeApp, linkURL: linkURL) {
            // expected
        } else {
            Issue.record("Expected linkedElsewhere status")
        }
        _ = try CLIHelper.installSymlink(bundleURL: fakeApp, linkURL: linkURL)
        #expect(CLIHelper.symlinkStatus(bundleURL: fakeApp, linkURL: linkURL) == .linked)

        // 普通文件冲突必须拒绝静默覆盖
        try FileManager.default.removeItem(at: linkURL)
        try "user binary".write(to: linkURL, atomically: true, encoding: .utf8)
        #expect(CLIHelper.symlinkStatus(bundleURL: fakeApp, linkURL: linkURL) == .conflictFile)
        #expect(throws: CLIHelper.HelperError.conflictExists(linkURL.path)) {
            try CLIHelper.installSymlink(bundleURL: fakeApp, linkURL: linkURL)
        }
        #expect(try String(contentsOf: linkURL, encoding: .utf8) == "user binary")
    }

    @Test func terminalAppendsBundledHelpersToPathTailWithoutPreempting() throws {
        let root = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: root) }
        let fakeApp = root.appendingPathComponent("Hortus.app", isDirectory: true)
        let helper = fakeApp.appendingPathComponent("Contents/Helpers/hortus")
        let helpersDir = fakeApp.appendingPathComponent("Contents/Helpers").standardizedFileURL.path

        // 未内置 helper 时保持原 PATH
        #expect(TerminalSession.appendingBundledHelpers(to: "/usr/local/bin:/usr/bin", bundleURL: fakeApp) == "/usr/local/bin:/usr/bin")

        // 内置 helper 存在时追加到尾部且去重
        try makeExecutable(at: helper)
        let appended = TerminalSession.appendingBundledHelpers(to: "/opt/homebrew/bin:/usr/bin", bundleURL: fakeApp)
        #expect(appended == "/opt/homebrew/bin:/usr/bin:\(helpersDir)")
        let deduped = TerminalSession.appendingBundledHelpers(to: appended, bundleURL: fakeApp)
        #expect(deduped == appended)
    }

    @Test func mcpConfigurationJSONPrefersLinkedCLIAndFallsBackToBundled() throws {
        let root = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: root) }
        let fakeApp = root.appendingPathComponent("Hortus.app", isDirectory: true)
        let helper = fakeApp.appendingPathComponent("Contents/Helpers/hortus")
        try makeExecutable(at: helper)
        let linkURL = root.appendingPathComponent("home/.local/bin/hortus")

        // 未软链时回退内置绝对路径
        let fallbackJSON = CLIHelper.mcpConfigurationJSON(
            vaultPath: "/Users/example/Notes",
            bundleURL: fakeApp,
            linkURL: linkURL
        )
        let fallbackObj = try #require(
            JSONSerialization.jsonObject(with: Data(fallbackJSON.utf8)) as? [String: Any]
        )
        let fallbackServers = try #require(fallbackObj["mcpServers"] as? [String: Any])
        let fallbackHortus = try #require(fallbackServers["hortus"] as? [String: Any])
        #expect(fallbackHortus["command"] as? String == helper.standardizedFileURL.path)
        #expect(fallbackHortus["args"] as? [String] == ["mcp", "--vault", "/Users/example/Notes"])

        // 软链后优先使用 ~/.local/bin/hortus
        _ = try CLIHelper.installSymlink(bundleURL: fakeApp, linkURL: linkURL)
        let linkedJSON = CLIHelper.mcpConfigurationJSON(
            vaultPath: "/Users/example/Notes",
            bundleURL: fakeApp,
            linkURL: linkURL
        )
        let linkedObj = try #require(
            JSONSerialization.jsonObject(with: Data(linkedJSON.utf8)) as? [String: Any]
        )
        let linkedServers = try #require(linkedObj["mcpServers"] as? [String: Any])
        let linkedHortus = try #require(linkedServers["hortus"] as? [String: Any])
        #expect(linkedHortus["command"] as? String == linkURL.path)
    }

    // MARK: - 3.3 scene_svg Screen-Consistent Export

    @Test func sceneSvgMatchesEngineRenderSvgByteForByte() throws {
        let engine = try PlatformGraphics.makeDiagramEngine()
        let source = "flowchart TB\nsubgraph Group\nA[Start] --> B{Check}\nB -->|yes| C([Done])\nend\n"
        let identity = DiagramIdentity(
            document: "scene-svg-test",
            block: 1,
            revision: 1,
            dark: true,
            fontGeneration: 1,
            requestGeneration: 1
        )
        let sceneData = try engine.renderRequest(
            source: source,
            mmd: true,
            width: 700,
            collapsed: [],
            identity: identity,
            previous: nil,
            viewport: []
        )
        let prepared = PreparedScene(scene: try DiagramScene(sceneData))
        let fromPrepared = try DiagramExport.renderSVG(prepared: prepared)
        let direct = try engine.renderSvg(
            source: source,
            mmd: true,
            block: 1,
            width: 700,
            dark: true,
            collapsed: []
        )
        #expect(fromPrepared == direct)
        #expect(fromPrepared.hasPrefix("<svg xmlns=\"http://www.w3.org/2000/svg\""))
    }
}
