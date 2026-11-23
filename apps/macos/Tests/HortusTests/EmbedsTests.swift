import AppKit
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

    @MainActor
    private func request(_ target: String, heading: String? = nil, in temporary: URL,
                         handle: VaultHandle, width: Double = 460) -> EmbedRequest {
        EmbedRequest(source: "host.md", target: target, heading: heading, alias: nil,
                     ancestors: ["host.md"], depth: 1, handle: handle,
                     vault: temporary.appendingPathComponent("vault").path, width: width,
                     size: 15, dark: false, service: DiagramService(), mathService: .shared)
    }

    @MainActor
    private func wait(_ window: NSWindow, until matches: () -> Bool) async throws {
        for _ in 0..<400 {
            window.contentView?.layoutSubtreeIfNeeded()
            if matches() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        try #require(matches())
    }

    @Test @MainActor func embeddedRunsMapAndActivateAfterBadDiagramAndSendFromFirstResponder() async throws {
        let source = """
        ---
        aliases: [Sample]
        ---
        # Heading

        First paragraph.

        ```mermaid
        flowchart LR
        Broken[unclosed
        ```

        **Strong** [[Target]] #sampletag
        """
        let (temporary, handle) = try fixture(["host.md": "host text", "Child.md": source, "Target.md": "# Target"])
        defer {
            handle.close()
            try? FileManager.default.removeItem(at: temporary)
        }
        let host = try EditingTests.Fixture("host old selection")
        try await host.ready()
        let workspace = Workspace()
        workspace.tabs = [NoteTab(path: host.path, document: host.document, loading: false)]
        workspace.selected = host.path
        host.view.setSelectedRange(NSRange(location: 0, length: 4))

        let card = EmbeddedNoteView(request("Child", heading: "Heading", in: temporary, handle: handle))
        host.window.contentView?.addSubview(card)
        defer { card.cancel(); card.removeFromSuperview() }
        var link: String?, tag: String?
        card.onOpenLink = { path, _ in link = path }
        card.onShowTag = { tag = $0 }
        try await wait(host.window) { card.inspectionReadingView != nil }
        let reading = try #require(card.inspectionReadingView)
        let view = reading.inspectionTextView
        #expect(view.string.contains("Broken[unclosed"))
        #expect(!view.string.contains("\u{fffc}"))
        let styling = try #require(view.sourceStyling)
        #expect(styling.runs.map(\.start) == styling.runs.map(\.start).sorted())
        let display = view.string as NSString
        let linkRange = display.range(of: "Target")
        let tagRange = display.range(of: "sampletag")
        try #require(linkRange.location != NSNotFound && tagRange.location != NSNotFound)
        #expect(styling.mask(at: display.range(of: "Strong").location) & 2 != 0)
        #expect(styling.mask(at: linkRange.location) & (1 << 18) != 0)
        #expect(styling.mask(at: tagRange.location) & 512 != 0)
        let sliced = try #require(card.inspectionContent) as NSString
        let sourceLinkRange = sliced.range(of: "[[Target]]")
        #expect(reading.inspectionSourcePosition(at: linkRange.location) == sourceLinkRange.location + 2)
        #expect(reading.inspectionSourcePosition(at: tagRange.location) == sliced.range(of: "#sampletag").location + 1)
        reading.inspectionActivate(at: linkRange.location)
        reading.inspectionActivate(at: tagRange.location)
        #expect(link == "Target.md")
        #expect(tag == "sampletag")

        let menu = NSMenu()
        menu.autoenablesItems = false
        let sendItem = NSMenuItem(title: "Send Selection", action: nil, keyEquivalent: "l")
        sendItem.keyEquivalentModifierMask = [.control, .command, .shift]
        menu.addItem(sendItem)
        let previousMenu = NSApp.mainMenu
        NSApp.mainMenu = menu
        defer { NSApp.mainMenu = previousMenu }
        host.window.makeKey()
        #expect(host.window.makeFirstResponder(view))
        view.setSelectedRange(linkRange)
        SendToTerminalMenuState.sync(workspace: workspace)
        #expect(sendItem.isEnabled)
        let selection = try #require(SendToTerminalSelection.current(in: workspace, window: host.window))
        #expect(selection.path == "Child.md")
        #expect(selection.text == "Target")
        #expect(selection.lines == 13...13)
        let payload = TerminalPayload.selection(relativePath: selection.path, text: selection.text, lineRange: selection.lines)
        #expect(payload.text == "Child.md（第 13 行）：\nTarget")
        view.setSelectedRange(NSRange(location: linkRange.location, length: 0))
        #expect(host.view.selectedRange().length == 4)
        #expect(SendToTerminalSelection.current(in: workspace, window: host.window) == nil)
        SendToTerminalMenuState.sync(workspace: workspace)
        #expect(!sendItem.isEnabled)
        await host.close()
    }

    @Test @MainActor func embeddedDiagramsResolveLinksExportCollapseAndFitNestedCards() async throws {
        let graph = "mindmap\n  root((Root))\n    [[Target#Section]]\n      Leaf\n"
        let rich = "# Card\n\n```mermaid\n" + graph + "```\n\n"
            + "| Wide column | Other column |\n| --- | --- |\n| Long text | More text |\n"
        let (temporary, handle) = try fixture([
            "host.md": "# Host", "other/Target.md": "# Other target", "deep/Target.md": "# Section",
            "deep/graph.mmd": graph, "deep/rich.md": rich,
            "outer.md": "# Outer\n\n![[deep/rich]]\n\n" + rich,
        ])
        defer {
            handle.close()
            try? FileManager.default.removeItem(at: temporary)
        }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 980, height: 740),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let mmd = EmbeddedNoteView(request("deep/graph.mmd", in: temporary, handle: handle))
        let outer = EmbeddedNoteView(request("outer", in: temporary, handle: handle))
        outer.setFrameOrigin(NSPoint(x: 480, y: 0))
        window.contentView?.addSubview(mmd)
        window.contentView?.addSubview(outer)
        window.orderFrontRegardless()
        defer { mmd.cancel(); outer.cancel(); window.close() }
        try await wait(window) {
            mmd.inspectionDiagramView != nil && outer.inspectionEmbeddedCards.first?.inspectionDiagramCount == 1
        }
        let nested = try #require(outer.inspectionEmbeddedCards.first)
        let mmdDiagram = try #require(mmd.inspectionDiagramView)
        let noteDiagram = try #require(nested.inspectionReadingView?.inspectionTextView.diagramViews[1])
        for (view, card) in [(mmdDiagram, mmd), (noteDiagram, nested)] {
            let linked = try #require(view.prepared.scene.hitRegions.first { $0.linkTarget == "Target" })
            let center = view.convert(CGPoint(x: linked.rect.cgRect.midX, y: linked.rect.cgRect.midY), to: nil)
            let event = try #require(NSEvent.mouseEvent(with: .rightMouseDown, location: center,
                modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
                eventNumber: 0, clickCount: 1, pressure: 1))
            let menu = try #require(view.menu(for: event))
            #expect(!menu.autoenablesItems)
            #expect(menu.items.first { $0.title == L10n.string("Focus Mode") }?.isEnabled == false)
            let item = try #require(menu.items.first { $0.title == DiagramExportStrings.openLinkedNote })
            var opened: String?, heading: String?
            card.onOpenLink = { opened = $0; heading = $1 }
            NSApp.sendAction(try #require(item.action), to: item.target, from: item)
            try await wait(window) { opened != nil }
            #expect(opened == "deep/Target.md")
            #expect(heading == "Section")

            let parent = try #require(view.prepared.scene.hitRegions.first { $0.canCollapse })
            view.onToggle?(parent.nodeId)
            try await wait(window) {
                view.prepared.scene.hitRegions.contains { $0.nodeId == parent.nodeId && $0.collapsed }
            }
            let export = try #require(view.exportRequestProvider?())
            #expect(export.note == card.inspectionResolvedPath)
            let svg = try await export.svg()
            #expect(svg.hasPrefix("<svg") && !svg.contains(">Leaf<"))
            #expect(DiagramExport.renderPNG(prepared: export.prepared)?.starts(with: [0x89, 0x50, 0x4e, 0x47]) == true)
            #expect(DiagramExport.renderPDF(prepared: export.prepared)?.starts(with: "%PDF-".utf8) == true)
        }
        for card in [outer, nested] {
            let view = try #require(card.inspectionReadingView?.inspectionTextView)
            #expect(!view.diagramViews.isEmpty && !view.mediaViews.isEmpty)
            view.needsLayout = true
            view.layoutSubtreeIfNeeded()
            let items: [NSView] = Array(view.diagramViews.values) + view.mediaViews
            #expect(items.allSatisfy { $0.frame.maxX <= view.bounds.width + 0.01 })
            let renderedWidth = try #require(view.diagramViews[1]?.prepared.scene.width)
            #expect(abs(renderedWidth - max(120, card.bounds.width - 52)) < 0.01)
        }
        mmd.setFrameSize(NSSize(width: 390, height: mmd.frame.height))
        try await wait(window) { mmdDiagram.prepared.scene.width == 366 }
        #expect(mmdDiagram.frame.width == 366)
    }

    @Test @MainActor func cancelledPreparationReturnsBeforeResolvingEmbed() async throws {
        let (temporary, handle) = try fixture(["host.md": "# Host", "child.md": "# Child"])
        defer {
            handle.close()
            try? FileManager.default.removeItem(at: temporary)
        }
        let request = request("child", in: temporary, handle: handle)
        let renderer = EmbedRenderer()
        let task = Task { await renderer.prepare(request) }
        task.cancel()
        #expect(await task.value == nil)
        #expect(await renderer.prepare(request)?.info.resolvedPath == "child.md")
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
