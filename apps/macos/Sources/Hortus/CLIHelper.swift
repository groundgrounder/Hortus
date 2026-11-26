import AppKit
import Foundation
import HortusApple

/// 内置 `Hortus.app/Contents/Helpers/hortus` CLI 辅助工具：软链管理与 MCP 配置复制。
@MainActor
enum CLIHelper {
    enum SymlinkStatus: Equatable {
        case notInstalled
        case linked
        case linkedElsewhere(String)
        case conflictFile
    }

    enum HelperError: LocalizedError, Equatable {
        case helperMissing
        case conflictExists(String)
        case operationFailed(String)

        var errorDescription: String? {
            switch self {
            case .helperMissing:
                return L10n.string("Bundled hortus CLI was not found")
            case let .conflictExists(path):
                return L10n.string("A file already exists at %@. Remove or move it first.", path)
            case let .operationFailed(detail):
                return detail
            }
        }
    }

    nonisolated static func bundledCLIURL(bundleURL: URL = Bundle.main.bundleURL) -> URL {
        bundleURL.appendingPathComponent("Contents/Helpers/hortus", isDirectory: false)
    }

    nonisolated static func bundledHelpersDirectory(bundleURL: URL = Bundle.main.bundleURL) -> URL {
        bundleURL.appendingPathComponent("Contents/Helpers", isDirectory: true)
    }

    nonisolated static func defaultLinkURL(
        homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> URL {
        homeDirectory.appendingPathComponent(".local/bin/hortus", isDirectory: false)
    }

    nonisolated static func symlinkStatus(
        bundleURL: URL = Bundle.main.bundleURL,
        linkURL: URL? = nil
    ) -> SymlinkStatus {
        let target = linkURL ?? defaultLinkURL()
        let helper = bundledCLIURL(bundleURL: bundleURL).standardizedFileURL.path
        let fm = FileManager.default
        guard let attrs = try? fm.attributesOfItem(atPath: target.path) else {
            return .notInstalled
        }
        guard let type = attrs[.type] as? FileAttributeType else {
            return .conflictFile
        }
        if type == .typeSymbolicLink {
            guard let rawDest = try? fm.destinationOfSymbolicLink(atPath: target.path) else {
                return .linkedElsewhere("")
            }
            let resolvedDest = URL(fileURLWithPath: rawDest, relativeTo: target.deletingLastPathComponent())
                .standardizedFileURL.path
            if resolvedDest == helper {
                return .linked
            }
            return .linkedElsewhere(resolvedDest)
        }
        return .conflictFile
    }

    /// 创建或更新 `~/.local/bin/hortus` 软链指向当前 App 内置 CLI；若存在非软链普通文件/目录则拒绝覆盖。
    @discardableResult
    nonisolated static func installSymlink(
        bundleURL: URL = Bundle.main.bundleURL,
        linkURL: URL? = nil
    ) throws -> URL {
        let helper = bundledCLIURL(bundleURL: bundleURL).standardizedFileURL
        let fm = FileManager.default
        guard fm.fileExists(atPath: helper.path) else {
            throw HelperError.helperMissing
        }
        let destination = linkURL ?? defaultLinkURL()
        let parent = destination.deletingLastPathComponent()
        do {
            try fm.createDirectory(at: parent, withIntermediateDirectories: true)
        } catch {
            throw HelperError.operationFailed(error.localizedDescription)
        }
        if let attrs = try? fm.attributesOfItem(atPath: destination.path) {
            let type = attrs[.type] as? FileAttributeType
            if type == .typeSymbolicLink {
                do {
                    try fm.removeItem(at: destination)
                } catch {
                    throw HelperError.operationFailed(error.localizedDescription)
                }
            } else {
                throw HelperError.conflictExists(destination.path)
            }
        }
        do {
            try fm.createSymbolicLink(atPath: destination.path, withDestinationPath: helper.path)
        } catch {
            throw HelperError.operationFailed(error.localizedDescription)
        }
        return destination
    }

    /// 生成标准 MCP JSON 配置；优先使用已有效连接的 `~/.local/bin/hortus`，否则回退内置绝对路径。
    nonisolated static func mcpConfigurationJSON(
        vaultPath: String?,
        bundleURL: URL = Bundle.main.bundleURL,
        linkURL: URL? = nil
    ) -> String {
        let link = linkURL ?? defaultLinkURL()
        let command: String
        if symlinkStatus(bundleURL: bundleURL, linkURL: link) == .linked {
            command = link.path
        } else {
            command = bundledCLIURL(bundleURL: bundleURL).standardizedFileURL.path
        }
        let vault = vaultPath?.trimmingCharacters(in: .whitespacesAndNewlines)
        let resolvedVault = (vault?.isEmpty == false) ? vault! : "/path/to/folder"
        let payload: [String: Any] = [
            "mcpServers": [
                "hortus": [
                    "command": command,
                    "args": ["mcp", "--vault", resolvedVault],
                ],
            ],
        ]
        guard let data = try? JSONSerialization.data(
            withJSONObject: payload,
            options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        ), let json = String(data: data, encoding: .utf8) else {
            return "{}"
        }
        return json
    }

    @discardableResult
    static func copyMCPConfiguration(
        vaultPath: String?,
        bundleURL: URL = Bundle.main.bundleURL,
        linkURL: URL? = nil
    ) -> String {
        let json = mcpConfigurationJSON(vaultPath: vaultPath, bundleURL: bundleURL, linkURL: linkURL)
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(json, forType: .string)
        return json
    }
}
