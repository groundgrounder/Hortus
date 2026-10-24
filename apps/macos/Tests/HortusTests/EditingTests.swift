import AppKit
import HortusCore
import Testing
@testable import Hortus

@Suite(.serialized) @MainActor
struct EditingTests {
    @MainActor final class Fixture {
        let directory: URL
        let vault: URL
        let core: WorkspaceCore
        let document: NoteDocument
        let view: DiagramTextView
        let window: NSWindow
        let coordinator = NoteText.Coordinator()
        let service = DiagramService()
        init(_ source: String) throws {
            directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            vault = directory.appendingPathComponent("vault")
            try FileManager.default.createDirectory(at: vault, withIntermediateDirectories: true)
            let url = vault.appendingPathComponent("note.md")
            try Data(source.utf8).write(to: url)
            core = try WorkspaceCore.inDirectories(config: directory.appendingPathComponent("config").path,
                                                   cache: directory.appendingPathComponent("cache").path)
            let session = try core.openDocument(vault: vault.path, path: "note.md")
            document = NoteDocument(io: DocumentIO(session: session, url: url), initial: try session.initial())
            let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 760, height: 540))
            view = DiagramTextView(diagramLayout: true)
            view.frame = scroll.bounds; view.allowsUndo = true; view.isRichText = false
            view.isVerticallyResizable = true; view.textContainer?.widthTracksTextView = true
            view.textContainerInset = NSSize(width: 30, height: 24)
            scroll.documentView = view
            window = NSWindow(contentRect: scroll.frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false; window.contentView = scroll
            window.makeKeyAndOrderFront(nil); window.makeFirstResponder(view)
            coordinator.view = view
            coordinator.update(NoteText(text: source, path: "note.md", vault: vault.path, fontSize: 15,
                                        service: service, core: nil, document: document) { _, _ in })
        }
        func ready() async throws {
            try await eventually { !self.view.diagramPending && self.view.isEditable }
        }
        func close() async {
            _ = await document.prepareToClose()
            coordinator.task?.cancel(); document.onReload = nil
            view.delegate = nil; view.textStorage?.delegate = nil
            window.close()
            try? FileManager.default.removeItem(at: directory)
        }
        func text() async throws -> String { try await document.io.perform { try $0.snapshot().text } }
    }
    static let source = "前文😀\n\n```mermaid\nflowchart TB\nR[Root] --> A[Branch]\nA --> B[Leaf]\n```\n\n末尾\n"

    @Test func seededNativeEditsStayEqualToCoreAndDiskFor1200Steps() async throws {
        let fixture = try Fixture(Self.source)
        try await fixture.ready()
        var seed: UInt64 = 0x104ed17
        func random(_ upper: Int) -> Int {
            seed = seed &* 6364136223846793005 &+ 1442695040888963407
            return Int((seed >> 32) % UInt64(max(1, upper)))
        }
        let inserts = ["中", "😀", "\n", "```", "~~~", "mermaid", " --> ", "", "根\n分支"]
        var reference = Self.source as NSString
        var fenceEdits = 0
        for step in 0..<1200 {
            let boundaries = Array((reference as String).indices).map { $0.utf16Offset(in: reference as String) } + [reference.length]
            let a = random(boundaries.count), b = min(boundaries.count - 1, a + random(min(8, boundaries.count - a)))
            let range = NSRange(location: boundaries[a], length: boundaries[b] - boundaries[a])
            let replacement = inserts[random(inserts.count)]
            let fence = reference.range(of: "```")
            if fence.location != NSNotFound, NSIntersectionRange(range, fence).length > 0 { fenceEdits += 1 }
            fixture.view.insertText(replacement, replacementRange: range)
            reference = reference.replacingCharacters(in: range, with: replacement) as NSString
            let mirrored = try await fixture.text()
            try #require(fixture.view.string == reference as String, "原生内容，step=\(step)")
            try #require(mirrored == fixture.view.string, "内核内容，step=\(step)")
            if step % 100 == 0 { await Task.yield() }
        }
        await fixture.document.saveNow()
        #expect(try String(contentsOf: fixture.document.io.url, encoding: .utf8) == fixture.view.string)
        #expect(!fixture.document.dirty)
        #expect(fenceEdits > 0)
        print("固定种子编辑：steps=1200 fence_edits=\(fenceEdits)")
        #expect(try FileManager.default.contentsOfDirectory(atPath: fixture.vault.path) == ["note.md"])
        await fixture.close()
    }

    @Test func chineseCompositionFreezesProjectionAndSavingUntilCommit() async throws {
        let fixture = try Fixture(Self.source)
        try await fixture.ready()
        let original = fixture.view.diagramStorage?.diagrams
        fixture.view.setSelectedRange(NSRange(location: 0, length: 0))
        fixture.view.setMarkedText("zhong", selectedRange: NSRange(location: 5, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        fixture.view.setMarkedText("中文", selectedRange: NSRange(location: 2, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        #expect(fixture.view.hasMarkedText())
        await fixture.document.saveNow()
        try await Task.sleep(for: .milliseconds(1150))
        #expect(try String(contentsOf: fixture.document.io.url, encoding: .utf8) == Self.source)
        #expect(fixture.view.diagramStorage?.diagrams.map(\.length) == original?.map(\.length))
        #expect(fixture.view.expanded.isEmpty)
        #expect(fixture.view.diagramViews.count == 1)
        fixture.view.insertText("中文", replacementRange: NSRange(location: NSNotFound, length: 0))
        #expect(!fixture.view.hasMarkedText())
        #expect(try await fixture.text() == fixture.view.string)
        #expect(fixture.view.string == "中文" + Self.source)
        try await eventually { !fixture.document.dirty }
        #expect(try String(contentsOf: fixture.document.io.url, encoding: .utf8) == fixture.view.string)
        await fixture.close()
    }

    @Test func selectionPreviewBrokenFencesAndNativeUndoRedo() async throws {
        let fixture = try Fixture(Self.source)
        try await fixture.ready()
        let block = try #require(fixture.view.blocks.first)
        fixture.view.setSelectedRange(NSRange(location: block.range.location - 1, length: 0))
        fixture.view.moveDown(nil)
        #expect(fixture.view.expanded == [block.ordinal])
        fixture.view.setSelectedRange(NSRange(location: fixture.view.sourceUTF16Length, length: 0))
        #expect(fixture.view.expanded.isEmpty)
        // "编辑源码"只在编辑视图、笔记可写时出现
        let diagram = try #require(fixture.view.diagramViews[block.ordinal])
        #expect(diagram.canEditSource)
        fixture.view.isEditable = false
        #expect(!diagram.canEditSource)
        fixture.view.isEditable = true
        fixture.view.isReading = true
        #expect(!diagram.canEditSource)
        fixture.view.isReading = false
        fixture.view.onEditSource?(block.ordinal)
        #expect(fixture.view.expanded == [block.ordinal])
        let nodeRange = (fixture.view.string as NSString).range(of: "Branch")
        fixture.view.setSelectedRange(NSRange(location: nodeRange.location, length: 0))
        #expect(fixture.view.expanded == [block.ordinal])
        #expect(fixture.view.diagramStorage?.diagrams.isEmpty == true)
        fixture.view.breakUndoCoalescing()
        fixture.view.insertText("Changed", replacementRange: nodeRange)
        fixture.view.breakUndoCoalescing()
        try await fixture.ready()
        #expect(fixture.view.diagramViews[1]?.prepared.scene.hitRegions.contains(where: { $0.label == "Changed" }) == true)
        fixture.view.undoManager?.undo()
        try await fixture.ready()
        #expect(fixture.view.string == Self.source)
        #expect(try await fixture.text() == Self.source)
        fixture.view.undoManager?.redo()
        try await fixture.ready()
        #expect(fixture.view.string.contains("Changed"))
        let closing = (fixture.view.string as NSString).range(of: "```", options: .backwards)
        fixture.view.breakUndoCoalescing()
        fixture.view.insertText("", replacementRange: closing)
        fixture.view.breakUndoCoalescing()
        try await fixture.ready()
        #expect(fixture.view.diagramViews.isEmpty)
        #expect(fixture.view.blocks.first?.reason != nil)
        #expect(try await fixture.text() == fixture.view.string)
        fixture.view.undoManager?.undo()
        try await fixture.ready()
        #expect(fixture.view.string.contains("```\n\n末尾"))
        #expect(fixture.view.diagramViews.count == 1)
        let bad = (fixture.view.string as NSString).range(of: "flowchart TB")
        fixture.view.insertText("flowchart ???", replacementRange: bad)
        try await fixture.ready()
        #expect(fixture.view.diagramViews.isEmpty)
        #expect(fixture.view.blocks.first?.reason != nil)
        fixture.view.setSelectedRange(NSRange(location: fixture.view.sourceUTF16Length, length: 0))
        await fixture.close()
    }

    @Test func automaticSaveFailureRetryExternalReloadAndConflictChoices() async throws {
        let fixture = try Fixture(Self.source)
        try await fixture.ready()
        fixture.view.insertText("本地", replacementRange: NSRange(location: 0, length: 0))
        try await eventually { !fixture.document.dirty }
        #expect(fixture.document.issue == .none)
        fixture.document.refresh()
        try await Task.sleep(for: .milliseconds(100))
        #expect(fixture.document.issue == .none)
        // 从阅读视图进入编辑后，外部载入必须保留当前选择，不能重用进入时的锚点。
        fixture.document.sourceAnchor = 8
        let selection = NSRange(location: 1, length: 1)
        fixture.view.setSelectedRange(selection)
        let external = "外部\n"
        try Data(external.utf8).write(to: fixture.document.io.url)
        fixture.document.refresh()
        try await eventually { fixture.view.string == external }
        #expect(fixture.view.selectedRange() == selection)
        #expect(fixture.view.undoManager?.canUndo != true)
        fixture.view.insertText("我的", replacementRange: NSRange(location: 0, length: 0))
        try Data("其他版本\n".utf8).write(to: fixture.document.io.url)
        await fixture.document.saveNow()
        #expect(fixture.document.issue == .conflict)
        #expect(try String(contentsOf: fixture.document.io.url, encoding: .utf8) == "其他版本\n")
        await fixture.document.saveNow(.keepMine)
        #expect(try String(contentsOf: fixture.document.io.url, encoding: .utf8) == fixture.view.string)
        fixture.view.insertText("新输入", replacementRange: NSRange(location: 0, length: 0))
        try Data("用磁盘\n".utf8).write(to: fixture.document.io.url)
        fixture.document.refresh()
        try await eventually { fixture.document.issue == .conflict }
        await fixture.document.useDisk()
        #expect(fixture.view.string == "用磁盘\n")
        fixture.view.insertText("副本", replacementRange: NSRange(location: 0, length: 0))
        let mine = fixture.view.string
        try Data("保留外部\n".utf8).write(to: fixture.document.io.url)
        await fixture.document.saveNow()
        await fixture.document.saveNow(.conflictCopy)
        let copy = fixture.vault.appendingPathComponent("note (conflict copy).md")
        #expect(try String(contentsOf: copy, encoding: .utf8) == mine)
        #expect(try String(contentsOf: fixture.document.io.url, encoding: .utf8) == "保留外部\n")
        fixture.view.insertText("失败后保留", replacementRange: NSRange(location: 0, length: 0))
        try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: fixture.document.io.url.path)
        await fixture.document.saveNow()
        #expect(fixture.document.issue == .saveFailed)
        #expect(fixture.document.dirty)
        let recovery = fixture.directory.appendingPathComponent("cache/recovery")
        #expect(try FileManager.default.contentsOfDirectory(atPath: recovery.path).count == 1)
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: fixture.document.io.url.path)
        try await eventually { !fixture.document.dirty }
        #expect(try FileManager.default.contentsOfDirectory(atPath: recovery.path).isEmpty)
        await fixture.close()
    }

    @Test func nativeSaveRetainsAllEncodingCombinationsAndCurrentPermissions() async throws {
        for bom in [false, true] {
            for crlf in [false, true] {
                for trailing in [false, true] {
                    let canonical = "正文😀\n末尾" + (trailing ? "\n" : "")
                    let prefix = bom ? "\u{feff}" : ""
                    let source = prefix + (crlf ? canonical.replacingOccurrences(of: "\n", with: "\r\n") : canonical)
                    let fixture = try Fixture(source)
                    try await fixture.ready()
                    try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: fixture.document.io.url.path)
                    fixture.view.insertText("新增", replacementRange: NSRange(location: 0, length: 0))
                    await fixture.document.saveNow()
                    let content = "新增" + canonical
                    let expected = prefix + (crlf ? content.replacingOccurrences(of: "\n", with: "\r\n") : content)
                    #expect(try Data(contentsOf: fixture.document.io.url) == Data(expected.utf8))
                    #expect(try FileManager.default.attributesOfItem(atPath: fixture.document.io.url.path)[.posixPermissions] as? Int == 0o640)
                    #expect(try FileManager.default.contentsOfDirectory(atPath: fixture.vault.path) == ["note.md"])
                    await fixture.close()
                }
            }
        }
    }
    @Test func externalRefreshInterleavedWithEditingConflictsInsteadOfLosingInput() async throws {
        let fixture = try Fixture("OLD\n")
        try await fixture.ready()
        var reloaded: [String] = []
        fixture.document.onReload = { reloaded.append($0) }
        try Data("EXTERNAL\n".utf8).write(to: fixture.document.io.url)

        fixture.document.refresh()
        waitForQueuedWork(fixture.document.io.queue)
        // 刷新只完成读取提议，主线程回调尚未执行时提交一次原生编辑。
        fixture.view.insertText("LOCAL ", replacementRange: NSRange(location: 0, length: 0))
        let edited = fixture.view.string
        try await eventually { fixture.document.issue == .conflict }
        #expect(fixture.document.text == "OLD\n")
        #expect(!reloaded.contains("EXTERNAL\n"))
        #expect(try await fixture.text() == edited)
        #expect(try String(contentsOf: fixture.document.io.url, encoding: .utf8) == "EXTERNAL\n")
        fixture.document.onReload = nil
        await fixture.close()
    }

    @Test func externalRefreshDuringCompositionWaitsForCommit() async throws {
        let fixture = try Fixture("OLD\n")
        try await fixture.ready()
        var reloaded: [String] = []
        fixture.document.onReload = { reloaded.append($0) }
        try Data("EXTERNAL\n".utf8).write(to: fixture.document.io.url)

        fixture.document.refresh()
        waitForQueuedWork(fixture.document.io.queue)
        fixture.document.compositionChanged(true)
        #expect(try await fixture.text() == "OLD\n")
        #expect(fixture.document.text == "OLD\n")
        #expect(!reloaded.contains("EXTERNAL\n"))

        fixture.document.compositionChanged(false)
        try await eventually { fixture.document.text == "EXTERNAL\n" }
        #expect(try await fixture.text() == "EXTERNAL\n")
        #expect(reloaded.contains("EXTERNAL\n"))
        fixture.document.onReload = nil
        await fixture.close()
    }

    @Test func discardRecoveryLoadsLatestDiskText() async throws {
        let fixture = try Fixture("磁盘\n")
        try await fixture.ready()
        fixture.document.edited(range: NSRange(location: 0, length: 0), replacement: "草稿")
        await fixture.document.checkpoint()
        let session = try fixture.core.openDocument(vault: fixture.vault.path, path: "note.md")
        let reopened = NoteDocument(io: DocumentIO(session: session, url: fixture.document.io.url),
                                    initial: try session.initial())
        #expect(reopened.issue == .recovery)
        try Data("外部\n".utf8).write(to: fixture.document.io.url)
        await reopened.recover(false)
        try await eventually { reopened.text == "外部\n" && reopened.issue == .none }
        reopened.stopForShutdown()
        await fixture.close()
    }

    @Test func permissionAndDeletionRefreshDriveReadOnlyAndRecreate() async throws {
        let fixture = try Fixture("权限\n")
        try await fixture.ready()
        let url = fixture.document.io.url
        try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: url.path)
        fixture.document.refresh()
        try await eventually { fixture.document.readOnlyReason != nil }
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)
        fixture.document.refresh()
        try await eventually { fixture.document.readOnlyReason == nil }
        try FileManager.default.removeItem(at: url)
        fixture.document.refresh()
        try await eventually { fixture.document.issue == .missing && fixture.document.readOnlyReason == nil }
        await fixture.document.saveNow(.recreate)
        try await eventually { fixture.document.issue == .none && !fixture.document.dirty }
        #expect(try String(contentsOf: url, encoding: .utf8) == fixture.view.string)
        await fixture.close()
    }
}

/// 在测试中阻塞主线程，直到文档队列处理完当前已排队的操作，用于制造外部刷新与输入交错。
@MainActor private func waitForQueuedWork(_ queue: DispatchQueue) {
    let gate = DispatchSemaphore(value: 0)
    queue.async { gate.signal() }
    _ = gate.wait(timeout: .now() + 5)
}

@MainActor private func eventually(_ predicate: () -> Bool) async throws {
    for _ in 0..<500 {
        if predicate() { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    Issue.record("等待条件超时")
}
