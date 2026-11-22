import Foundation
import HortusCore
import Testing
@testable import Hortus

@Suite struct EmbedsTests {
    private func fixture(_ notes: [String: String]) throws -> (URL, VaultHandle) {
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let root = temporary.appendingPathComponent("vault")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        for (path, text) in notes {
            let file = root.appendingPathComponent(path)
            try FileManager.default.createDirectory(at: file.deletingLastPathComponent(),
                                                    withIntermediateDirectories: true)
            try text.write(to: file, atomically: true, encoding: .utf8)
        }
        let core = try WorkspaceCore.inDirectories(config: temporary.appendingPathComponent("config").path,
                                                   cache: temporary.appendingPathComponent("cache").path)
        let handle = try core.openVault(path: root.path)
        handle.updateSearchIndex(upserted: Array(notes.keys), removed: [])
        return (temporary, handle)
    }

    @Test func embedResolutionSlicesSectionsAndReportsBoundaries() throws {
        let target = """
        ---
        aliases: [Blueprint]
        ---
        # Title

        Intro body.

        ## Section Two

        Section body.

        ## Section Three

        Later body.
        """
        let (temporary, handle) = try fixture([
            "design.md": target,
            "arch.mmd": "flowchart LR\n  A --> B\n",
            "a.md": "![[b]]",
            "b.md": "![[a]]",
        ])
        defer {
            handle.close()
            try? FileManager.default.removeItem(at: temporary)
        }
        let full = try handle.resolveEmbed(sourcePath: "a.md", rawTarget: "design", heading: nil,
                                           alias: nil, ancestors: ["a.md"], maxDepth: 3, width: 640, size: 15)
        #expect(full.kind == "note")
        #expect(full.resolvedPath == "design.md")
        #expect(full.content.hasPrefix("# Title"))
        #expect(!full.content.contains("aliases:"))
        #expect(!full.blocks.isEmpty)

        let section = try handle.resolveEmbed(sourcePath: "a.md", rawTarget: "design",
                                              heading: "  section two ", alias: "Alias",
                                              ancestors: ["a.md"], maxDepth: 3, width: 640, size: 15)
        #expect(section.kind == "note" && section.headingFound)
        #expect(section.content.hasPrefix("## Section Two"))
        #expect(!section.content.contains("Later body"))
        #expect(section.displayTitle == "Alias")

        let missing = try handle.resolveEmbed(sourcePath: "a.md", rawTarget: "design",
                                              heading: "No Such Heading", alias: nil,
                                              ancestors: ["a.md"], maxDepth: 3, width: 640, size: 15)
        #expect(missing.kind == "heading_missing" && !missing.headingFound)

        let mmd = try handle.resolveEmbed(sourcePath: "a.md", rawTarget: "arch.mmd", heading: nil,
                                          alias: nil, ancestors: ["a.md"], maxDepth: 3, width: 640, size: 15)
        #expect(mmd.kind == "diagram" && mmd.isMmd && mmd.diagrams.count == 1)

        let cycle = try handle.resolveEmbed(sourcePath: "a.md", rawTarget: "b", heading: nil,
                                            alias: nil, ancestors: ["a.md", "b.md"], maxDepth: 3,
                                            width: 640, size: 15)
        #expect(cycle.kind == "cycle")

        let deep = try handle.resolveEmbed(sourcePath: "a.md", rawTarget: "design", heading: nil,
                                           alias: nil, ancestors: ["a.md", "b.md", "c.md", "d.md"],
                                           maxDepth: 3, width: 640, size: 15)
        #expect(deep.kind == "depth_exceeded")

        let broken = try handle.resolveEmbed(sourcePath: "a.md", rawTarget: "missing", heading: nil,
                                             alias: nil, ancestors: ["a.md"], maxDepth: 3,
                                             width: 640, size: 15)
        #expect(broken.kind == "broken")
    }

    @Test func folderRenamePlansAndAppliesLinksOnDisk() throws {
        let outside = "\u{feff}[[docs/intro]] [[docs/sub/api#Auth|API]]\r\n![[docs/arch.mmd]] [[intro]] [[docs-extra/keep]]\r\n"
        let (temporary, handle) = try fixture([
            "docs/intro.md": "[[docs/sub/api]]\n[[intro]]\n",
            "docs/sub/api.md": "[[docs/intro]]\n",
            "docs/arch.mmd": "flowchart LR\n  A --> B\n",
            "docs-extra/keep.md": "# Keep\n",
            "outside.md": outside,
            "dirty.md": "[[docs/intro]]\n",
            "unrelated.md": "[[docs-extra/keep]]\n",
        ])
        defer {
            handle.close()
            try? FileManager.default.removeItem(at: temporary)
        }
        let root = temporary.appendingPathComponent("vault")
        let plan = handle.planFolderRenameLinks(oldDir: "docs", newDir: "papers")
        #expect(plan.totalRewrites == 6 && plan.affectedNotes == 4)
        #expect(plan.files.first(where: { $0.sourcePath == "outside.md" })?.edits.count == 3)
        #expect(plan.files.first(where: { $0.sourcePath == "docs-extra/keep.md" }) == nil)
        #expect(plan.files.first(where: { $0.sourcePath == "unrelated.md" }) == nil)

        _ = try handle.renameEntry(path: "docs", newName: "papers")
        let temporaryReplacement = try FileManager.default.url(for: .itemReplacementDirectory,
                                                              in: .userDomainMask, appropriateFor: root,
                                                              create: true)
        defer { try? FileManager.default.removeItem(at: temporaryReplacement) }
        let result = try handle.applyFolderRenameLinksIn(oldDir: "docs", newDir: "papers",
                                                        skipPaths: ["dirty.md"],
                                                        temporary: temporaryReplacement.path)
        #expect(result.totalRewrites == 5)
        #expect(result.skipped.map(\.path) == ["dirty.md"])
        #expect(result.failed.isEmpty)
        #expect(result.updated.contains("outside.md"))
        #expect(result.updated.contains("papers/intro.md"))
        #expect(result.updated.contains("papers/sub/api.md"))

        #expect(try String(contentsOf: root.appendingPathComponent("outside.md"), encoding: .utf8) ==
            outside.trimmingCharacters(in: CharacterSet(charactersIn: "\u{feff}"))
                .replacingOccurrences(of: "docs/intro", with: "papers/intro")
                .replacingOccurrences(of: "docs/sub/api", with: "papers/sub/api")
                .replacingOccurrences(of: "docs/arch.mmd", with: "papers/arch.mmd"))
        #expect(try String(contentsOf: root.appendingPathComponent("papers/intro.md"), encoding: .utf8) ==
            "[[papers/sub/api]]\n[[intro]]\n")
        #expect(try String(contentsOf: root.appendingPathComponent("papers/sub/api.md"), encoding: .utf8) ==
            "[[papers/intro]]\n")
        #expect(try String(contentsOf: root.appendingPathComponent("dirty.md"), encoding: .utf8) ==
            "[[docs/intro]]\n")
        #expect(try String(contentsOf: root.appendingPathComponent("docs-extra/keep.md"), encoding: .utf8) ==
            "# Keep\n")
        #expect(handle.backlinks(path: "papers/intro.md").isEmpty == false)
        #expect(handle.resolveLink(sourcePath: "outside.md", rawTarget: "papers/intro").resolvedPath ==
            "papers/intro.md")
    }
}
