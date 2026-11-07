import AppKit
import HortusCore
import Testing
@testable import Hortus

@MainActor private func eventually(_ predicate: () -> Bool) async throws {
    for _ in 0..<500 {
        if predicate() { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    Issue.record("等待阅读视图超时")
}

@Suite(.serialized) @MainActor
struct ReadingTests {
    @MainActor final class Fixture {
        let editing: EditingTests.Fixture
        let coordinator = ReadingText.Coordinator()
        var view: DiagramTextView { editing.view }
        var document: NoteDocument { editing.document }
        init(_ source: String) throws { editing = try EditingTests.Fixture(source) }
        func start(mathService: MathService = .shared, size: Double = 15) async throws {
            try await editing.ready()
            editing.coordinator.task?.cancel(); editing.coordinator.styling.reset()
            view.delegate = nil; view.textStorage?.delegate = nil
            view.onCommit = nil; view.onCompositionState = nil
            view.isReading = true; view.isEditable = false; view.isRichText = true; view.allowsUndo = false
            view.readingCoordinator = coordinator; coordinator.view = view
            coordinator.update(ReadingText(mathService: mathService, document: document, path: "note.md", vault: editing.vault.path,
                fontSize: size, service: editing.service, core: editing.core) { _, _ in })
            try await ready()
        }
        func ready() async throws {
            var previousFormulaState = ""
            var stableFormulaSamples = 0
            for _ in 0..<500 {
                self.editing.layoutHost()
                if self.editing.hostReady && !self.coordinator.pending
                    && self.coordinator.model?.revision == self.document.revision
                    && self.formulaHostReady {
                    editing.layoutHost()
                    let state = formulaHostState()
                    if state == previousFormulaState {
                        stableFormulaSamples += 1
                        if stableFormulaSamples == 2 { return }
                    } else {
                        previousFormulaState = state
                        stableFormulaSamples = 0
                    }
                }
                try await Task.sleep(for: .milliseconds(10))
            }
            Issue.record("阅读视图宿主未就绪: \(diagnostics(target: .zero))")
        }
        private var formulaHostReady: Bool {
            guard let manager = view.textLayoutManager, let content = manager.textContentManager else { return false }
            let items = view.inlineMath.values.map(\.item) + view.mathViews.map(\.item)
            for item in items {
                if let attachment = view.inlineMath[item.range.location],
                   let formula = view.inlineMathViews.first(where: { $0.attachment === attachment }) {
                    guard formula.superview === view, !formula.isHidden, !formula.frame.isEmpty,
                          !needsDisplay(in: formula) else { return false }
                    continue
                }
                if let formula = view.mathViews.first(where: { $0.item.identity == item.identity }) {
                    if !formula.isHidden {
                        guard formula.superview === view, !formula.frame.isEmpty,
                              !needsDisplay(in: formula) else { return false }
                        continue
                    }
                }
                // 未挂载的公式只有确认位于视口外时才允许完成；若 fragment 尚未可用，
                // 继续等待真实的布局状态，而不是把 state 当作已挂载公式的前置条件。
                guard let location = content.location(content.documentRange.location, offsetBy: item.range.location),
                      let fragment = manager.textLayoutFragment(for: location) else { return false }
                guard fragment.state.rawValue >= NSTextLayoutFragment.State.layoutAvailable.rawValue else { return false }
                let fragmentRect = fragment.layoutFragmentFrame.offsetBy(dx: view.textContainerInset.width,
                                                                         dy: view.textContainerInset.height)
                if view.visibleRect.intersects(fragmentRect) { return false }
            }
            return true
        }
        private func formulaHostState() -> String {
            let viewport = view.textLayoutManager?.textViewportLayoutController.viewportRange.map {
                let start = view.textLayoutManager?.offset(from: $0.location, to: $0.endLocation) ?? -1
                return "\(start)"
            } ?? "nil"
            let inline = view.inlineMathViews.sorted { $0.attachment.item.range.location < $1.attachment.item.range.location }
                .map { "\($0.attachment.item.range.location):\(NSStringFromRect($0.frame)):\($0.superview === view):\($0.isHidden):\(needsDisplay(in: $0))" }
                .joined(separator: ",")
            let block = view.mathViews.sorted { $0.item.range.location < $1.item.range.location }
                .map { "\($0.item.range.location):\(NSStringFromRect($0.frame)):\($0.superview === view):\($0.isHidden):\(needsDisplay(in: $0))" }
                .joined(separator: ",")
            return "viewport=\(viewport)|inline=\(inline)|block=\(block)|renders=\(view.layoutRenderCount)|pending=\(coordinator.pending)"
        }
        private func needsDisplay(in root: NSView) -> Bool {
            root.needsDisplay || root.subviews.contains(where: needsDisplay(in:))
        }
        func formulaDiagnostics() -> String {
            let manager = view.textLayoutManager
            let items = view.inlineMath.values.map(\.item) + view.mathViews.map(\.item)
            let viewport = manager?.textViewportLayoutController.viewportRange.map { range in
                let length = manager?.offset(from: range.location, to: range.endLocation) ?? -1
                return "\(length)"
            } ?? "nil"
            let fragments = items.sorted { $0.range.location < $1.range.location }.map { item in
                guard let manager, let content = manager.textContentManager,
                      let location = content.location(content.documentRange.location, offsetBy: item.range.location),
                      let fragment = manager.textLayoutFragment(for: location) else {
                    return "\(item.range.location)/\(item.range.length):fragment=nil"
                }
                let rect = fragment.layoutFragmentFrame.offsetBy(dx: view.textContainerInset.width,
                                                                 dy: view.textContainerInset.height)
                return "\(item.range.location)/\(item.range.length):state=\(fragment.state.rawValue),fragment=\(NSStringFromRect(rect)),visible=\(view.visibleRect.intersects(rect)),math=\(item.prepared.math != nil),reason=\(item.prepared.reason != nil),ax=\(NSStringFromRect(view.formulaAccessibilityFrame(item)))"
            }.joined(separator: ";")
            let mounted = view.inlineMathViews.sorted { $0.attachment.item.range.location < $1.attachment.item.range.location }.map {
                "inline:\($0.attachment.item.range.location)=\(NSStringFromRect($0.frame)):super:\($0.superview === view):hidden:\($0.isHidden):needs:\(needsDisplay(in: $0))"
            } + view.mathViews.sorted { $0.item.range.location < $1.item.range.location }.map {
                "block:\($0.item.range.location)=\(NSStringFromRect($0.frame)):super:\($0.superview === view):hidden:\($0.isHidden):needs:\(needsDisplay(in: $0))"
            }
            let visible = view.visibleRect.intersection(view.bounds)
            let bitmapState: String
            if visible.isNull || visible.isEmpty || !visible.width.isFinite || !visible.height.isFinite {
                bitmapState = "unavailable"
            } else if let bitmap = view.bitmapImageRepForCachingDisplay(in: visible) {
                view.cacheDisplay(in: visible, to: bitmap)
                var opaque = 0
                var dark = 0
                for y in 0..<bitmap.pixelsHigh {
                    for x in 0..<bitmap.pixelsWide {
                        guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB),
                              color.alphaComponent > 0.5 else { continue }
                        opaque += 1
                        if max(color.redComponent, color.greenComponent, color.blueComponent) < 0.5 { dark += 1 }
                    }
                }
                bitmapState = "\(bitmap.pixelsWide)x\(bitmap.pixelsHigh),opaque:\(opaque),dark:\(dark)"
            } else {
                bitmapState = "nil"
            }
            return "formulas=viewport:\(viewport),items:\(items.count),inline:view:\(view.inlineMathViews.count),media:\(view.mediaViews.count),block:\(view.mathViews.count),bitmap:\(bitmapState) @ \(fragments) mounted=\(mounted.joined(separator: ","))"
        }
        func revealAndWait(source: Int) async throws -> (display: Int, target: CGRect) {
            coordinator.reveal(source: source)
            return try await waitUntilVisible(source: source)
        }
        func waitUntilVisible(source: Int) async throws -> (display: Int, target: CGRect) {
            let display = coordinator.displayPosition(source: source)
            for _ in 0..<500 {
                self.editing.layoutHost()
                if self.view.visibleRect.intersects(self.coordinator.rect(at: display)) {
                    editing.layoutHost()
                    return (display, coordinator.rect(at: display))
                }
                try await Task.sleep(for: .milliseconds(10))
            }
            let target = coordinator.rect(at: display)
            Issue.record("阅读位置未完成 reveal: \(diagnostics(target: target))")
            return (display, target)
        }
        func waitForRefresh(
            text: String,
            document: NoteDocument,
            coordinator: ReadingText.Coordinator
        ) async throws {
            for _ in 0..<500 {
                self.editing.layoutHost()
                if document.text == text && !coordinator.pending
                    && coordinator.model?.revision == document.revision
                    && self.editing.hostReady {
                    try await waitForViewportToSettle()
                    return
                }
                try await Task.sleep(for: .milliseconds(10))
            }
            Issue.record("阅读刷新未完成: \(diagnostics(target: .zero))")
        }
        func waitForViewportToSettle() async throws {
            var previous = ""
            var stableSamples = 0
            for _ in 0..<500 {
                editing.layoutHost()
                let clip = editing.scroll.contentView
                let anchor = coordinator.visibleAnchor()
                let state = "\(NSStringFromRect(clip.bounds))|\(NSStringFromRect(view.visibleRect))|\(NSStringFromRect(view.frame))|\(coordinator.pending)|\(coordinator.model?.revision ?? .max)|\(anchor?.id.description ?? "nil"):\(anchor?.source.description ?? "nil"):\(anchor?.offset ?? .nan)"
                if state == previous {
                    stableSamples += 1
                    if stableSamples == 2 { return }
                } else {
                    previous = state
                    stableSamples = 0
                }
                try await Task.sleep(for: .milliseconds(10))
            }
            Issue.record("阅读视图布局与滚动状态未稳定: \(editing.layoutDiagnostics(target: .zero))")
        }
        func diagnostics(target: CGRect) -> String {
            editing.layoutDiagnostics(target: target)
                + "reading=pending:\(coordinator.pending),model-revision:\(coordinator.model?.revision.description ?? "nil"),document-revision:\(document.revision),text-revision:\(document.textRevision)"
                + "\n" + formulaDiagnostics()
        }
        func close() async {
            coordinator.detach(); view.readingCoordinator = nil
            await editing.close()
        }
    }

    @Test func displayCopySourceCopyFindAndHeadingRotor() async throws {
        let source = "# Heading😀\n\nA **bold** word and [[Target|Alias]].\n\n- [ ] Task\n"
        let f = try Fixture(source); try await f.start()
        #expect(!f.view.isEditable && f.view.isSelectable)
        for marker in ["#", "**", "[[", "- [ ]"] { #expect(!f.view.string.contains(marker)) }
        f.view.setSelectedRange(NSRange(location: 0, length: f.view.sourceUTF16Length))
        f.view.copy(nil)
        #expect(NSPasteboard.general.string(forType: .string) == f.view.string)
        f.coordinator.copyMarkdown()
        #expect(NSPasteboard.general.string(forType: .string) == source)
        let rotor = try #require(f.view.accessibilityCustomRotors().first)
        let search = NSAccessibilityCustomRotor.SearchParameters()
        search.searchDirection = .next; search.filterString = ""
        #expect(f.view.rotor(rotor, resultFor: search)?.customLabel == L10n.string("Heading level %1$@, %2$@", "1", "Heading😀"))
        f.view.setSelectedRange((f.view.string as NSString).range(of: "Alias"))
        let item = NSMenuItem(); item.tag = NSTextFinder.Action.setSearchString.rawValue
        f.view.performFindPanelAction(item)
        item.tag = NSTextFinder.Action.showFindInterface.rawValue; f.view.performFindPanelAction(item)
        #expect(f.view.enclosingScrollView?.isFindBarVisible == true)
        #expect(!f.document.dirty)
        await f.close()
    }

    @Test func twoHundredLocationsStayInTheSameBlockAndAreVisible() async throws {
        let source = (0..<220).map { "## Heading \($0)😀\n\nText **bold** and [[Target|Alias]] \($0).\n\n" }.joined()
        let f = try Fixture(source); try await f.start()
        let blocks = try #require(f.coordinator.model?.blocks)
        var seed: UInt64 = 0x106
        for _ in 0..<200 {
            seed = seed &* 6364136223846793005 &+ 1
            let index = Int(seed >> 32) % blocks.count
            let block = blocks[index]
            let source = Int(block.sourceStart + block.sourceLength / 2)
            let display = f.coordinator.displayPosition(source: source)
            let back = f.coordinator.sourcePosition(at: display)
            #expect(back >= Int(block.sourceStart) && back < Int(block.sourceStart + block.sourceLength))
            let revealed = try await f.revealAndWait(source: source)
            #expect(f.view.visibleRect.intersects(revealed.target), "\(f.diagnostics(target: revealed.target))")
        }
        await f.close()
    }

    @Test func mouseTaskSavesExactlyOneSourceCharacter() async throws {
        let source = "前言😀\n\n- [ ] First\n- [x] Second\n\nEnd\n"
        let f = try Fixture(source); try await f.start()
        let check = (f.view.string as NSString).range(of: "☐")
        let rect = f.view.firstRect(forCharacterRange: check, actualRange: nil)
        let point = f.editing.window.convertPoint(fromScreen: CGPoint(x: rect.minX + 2, y: rect.midY))
        let event = try #require(NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [], timestamp: 0,
            windowNumber: f.editing.window.windowNumber, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        #expect(f.coordinator.clickTask(event))
        try await eventually { !f.document.dirty && !f.coordinator.pending && f.view.string.contains("☑") }
        let saved = try String(contentsOf: f.document.io.url, encoding: .utf8)
        #expect(saved == source.replacingOccurrences(of: "[ ]", with: "[x]"))
        #expect(zip(source.utf16, saved.utf16).filter { $0 != $1 }.count == 1)
        await f.close()
    }

    @Test func diagramHeightFallbackAndContinuousQuoteBars() async throws {
        let source = "> Line one\n> Line two\n> Line three\n\n> [!note] Title\n> Line two\n> Line three\n\n" + EditingTests.source + "\n\n~~~mermaid\nsequenceDiagram\nA->>B: Hello\n~~~\n"
        let f = try Fixture(source); try await f.start()
        #expect(f.view.diagramStorage?.diagrams.isEmpty == true)
        #expect(f.view.diagramViews.count == 1)
        let manager = try #require(f.view.textLayoutManager), content = try #require(manager.textContentManager)
        manager.ensureLayout(for: content.documentRange)
        var quotes: [NoteLayoutFragment] = [], diagrams = 0, failures = 0
        manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: [.ensuresLayout]) { fragment in
            if let fragment = fragment as? NoteLayoutFragment {
                if fragment.sourceMask & 64 != 0 { quotes.append(fragment) }
                if let size = fragment.replacement { #expect(size.height > 20); diagrams += 1 }
                if fragment.reason != nil { failures += 1 }
            }
            return true
        }
        #expect(quotes.count == 6)
        #expect(quotes.map(\.quoteHead) == [true, false, false, true, false, false])
        #expect(quotes.map(\.quoteTail) == [false, false, true, false, false, true])
        #expect(diagrams == 1 && failures == 1)
        #expect(f.view.string.contains("sequenceDiagram"))
        await f.close()
    }

    @Test func externalMiddleChangeRebuildsOneBlockAndKeepsAnchor() async throws {
        let source = (0..<600).map { "Paragraph \($0) **bold** 中文😀.\n\n" }.joined()
        let f = try Fixture(source); try await f.start()
        _ = try await f.revealAndWait(source: source.utf16.count / 2)
        let before = try #require(f.coordinator.visibleAnchor())
        let changed = source.replacingOccurrences(of: "Paragraph 300", with: "Modified paragraph 300")
        try Data(changed.utf8).write(to: f.document.io.url)
        f.document.refresh()
        try await f.waitForRefresh(text: changed, document: f.document, coordinator: f.coordinator)
        let after = try #require(f.coordinator.visibleAnchor())
        #expect(f.coordinator.model?.rebuilt == 1)
        #expect(before.id == after.id)
        #expect(abs(before.offset - after.offset) <= 1,
                "before=\(before.id):\(before.offset),after=\(after.id):\(after.offset);\n\(f.diagnostics(target: .zero))")
        await f.close()
    }

    @Test func refreshRequestsAreCoalescedUntilTheActiveRequestFinishes() async throws {
        let f = try Fixture("# Heading\n\nBody\n")
        try await f.start()

        var count = f.coordinator.submittedRequests
        f.coordinator.refresh()
        f.coordinator.refresh()
        #expect(f.coordinator.submittedRequests == count + 1)
        try await f.ready()

        count = f.coordinator.submittedRequests
        f.coordinator.refresh()
        let scroll = try #require(f.view.enclosingScrollView as? DiagramScrollView)
        scroll.setFrameSize(NSSize(width: scroll.frame.width + 20, height: scroll.frame.height))
        scroll.layoutSubtreeIfNeeded()
        f.coordinator.refresh()
        f.coordinator.refresh()
        #expect(f.coordinator.submittedRequests == count + 1)
        try await f.ready()
        #expect(f.coordinator.submittedRequests == count + 2)
        await f.close()
    }

    @Test func scrollbarStylesUseStableContentWidthWithoutRefreshFeedback() async throws {
        let source = "# 稳定宽度\n\n" + String(repeating: "段落内容。\n\n", count: 80)
            + "```mermaid\nflowchart TB\nA[开始] --> B[结束]\n```\n"
        let f = try Fixture(source)
        let scroll = try #require(f.view.enclosingScrollView as? DiagramScrollView)
        scroll.scrollerStyle = .legacy
        try await f.start()
        let legacyWidth = scroll.stableContentWidth
        #expect(abs(f.view.frame.width - legacyWidth) <= 0.5)
        #expect(abs(f.view.layoutContentWidth - (legacyWidth - 60)) <= 0.5)
        #expect(f.view.blocks.compactMap { $0.prepared?.scene.width }
            .allSatisfy { abs($0 - f.view.layoutContentWidth) <= 0.5 })

        let legacyRequests = f.coordinator.submittedRequests
        for height in [100.0, 5_000, 120, 8_000, 200] {
            f.view.setFrameSize(NSSize(width: f.view.frame.width, height: height))
            scroll.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(20))
        }
        try await f.ready()
        #expect(f.coordinator.submittedRequests == legacyRequests)
        #expect(abs(f.view.layoutContentWidth - (legacyWidth - 60)) <= 0.5)

        scroll.scrollerStyle = .overlay
        NotificationCenter.default.post(name: NSScroller.preferredScrollerStyleDidChangeNotification, object: nil)
        try await eventually {
            !f.coordinator.pending && abs(f.view.frame.width - scroll.stableContentWidth) <= 0.5
        }
        let overlayRequests = f.coordinator.submittedRequests
        for height in [100.0, 5_000, 120, 8_000, 200] {
            f.view.setFrameSize(NSSize(width: f.view.frame.width, height: height))
            scroll.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(20))
        }
        try await f.ready()
        #expect(f.coordinator.submittedRequests == overlayRequests)
        #expect(abs(f.view.layoutContentWidth - (scroll.stableContentWidth - 60)) <= 0.5)
        await f.close()
    }
}
