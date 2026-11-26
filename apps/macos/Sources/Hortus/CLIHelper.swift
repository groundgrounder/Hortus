import AppKit
import Darwin
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

    private enum ExistingEntry {
        case absent
        case symlink
        case conflict
    }

    private nonisolated static func inspectEntry(atPath path: String) throws -> ExistingEntry {
        var st = stat()
        if lstat(path, &st) != 0 {
            let code = errno
            if code == ENOENT { return .absent }
            throw HelperError.operationFailed(String(cString: strerror(code)))
        }
        return (st.st_mode & S_IFMT) == S_IFLNK ? .symlink : .conflict
    }

    /// 创建或原子更新 `~/.local/bin/hortus` 软链指向当前 App 内置 CLI；若存在非软链普通文件/目录则拒绝覆盖。
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
        switch try inspectEntry(atPath: destination.path) {
        case .conflict:
            throw HelperError.conflictExists(destination.path)
        case .absent:
            if symlink(helper.path, destination.path) != 0 {
                let code = errno
                if code == EEXIST {
                    throw HelperError.conflictExists(destination.path)
                }
                throw HelperError.operationFailed(String(cString: strerror(code)))
            }
            return destination
        case .symlink:
            let tempLink = parent.appendingPathComponent(".hortus-link-\(UUID().uuidString).tmp", isDirectory: false)
            if symlink(helper.path, tempLink.path) != 0 {
                throw HelperError.operationFailed(String(cString: strerror(errno)))
            }
            do {
                let recheck = try inspectEntry(atPath: destination.path)
                guard recheck == .symlink else {
                    unlink(tempLink.path)
                    throw HelperError.conflictExists(destination.path)
                }
            } catch {
                unlink(tempLink.path)
                throw error
            }
            if rename(tempLink.path, destination.path) != 0 {
                let code = errno
                unlink(tempLink.path)
                if code == EISDIR || code == ENOTEMPTY || code == EEXIST {
                    throw HelperError.conflictExists(destination.path)
                }
                throw HelperError.operationFailed(String(cString: strerror(code)))
            }
            return destination
        }
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
        let resolvedVault: String
        if let vaultPath, !vaultPath.isEmpty {
            resolvedVault = vaultPath
        } else {
            resolvedVault = "/path/to/folder"
        }
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
