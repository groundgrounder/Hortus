#if DEBUG
import AppKit
import HortusCore

/// P1-08 验收：双向定位与专注模式的真实窗口检查；Release 包不包含此入口。
@MainActor
enum DiagramFocusInspection {
    static let argumentPrefix = "--diagram-focus-inspection="

    private struct Context {
        let workspace: Workspace
        let window: NSWindow
        let document: NoteDocument
        let path: String
        var view: DiagramTextView
    }

    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        guard let window = NSApp.windows.first(where: { $0.isVisible }) else {
            throw CocoaError(.coderValueNotFound)
        }
        // 首屏自检可能比标签挂载更早；等正文真正成为当前文档。
        var current: NoteDocument?
        for _ in 0..<1500 {
            if let candidate = workspace.current?.document, !candidate.text.isEmpty {
                current = candidate
                break
            }
            try await Task.sleep(for: .milliseconds(10))
        }
        guard let document = current, let path = workspace.selected else {
            throw CocoaError(.coderValueNotFound)
        }
        var result: [String: Any] = ["path": path]
        func log(_ step: String) { fputs("P1-08 \(step)\n", stderr) }
        log("document ready")
        if arguments.contains("--diagram-focus-only-scene"),
           let scene = arguments.first(where: { $0.hasPrefix("--diagram-focus-scene=") })?
            .dropFirst("--diagram-focus-scene=".count).lowercased() {
            result.merge(try await prepareScene(String(scene), workspace: workspace, window: window,
                                                document: document, arguments: arguments)) { _, new in new }
            window.displayIfNeeded()
            return result
        }
        log("reading cmd click")
        result.merge(try await readingCommandClick(workspace: workspace, window: window, document: document)) { _, new in new }
        log("menu")
        result.merge(try await menuAndAccessibility(workspace: workspace, window: window, document: document)) { _, new in new }
        log("highlight")
        result.merge(try await sourceHighlight(workspace: workspace, document: document)) { _, new in new }
        log("focus")
        result.merge(try await focusLifecycle(workspace: workspace, window: window, document: document)) { _, new in new }
        log("focus done")
        if let scene = arguments.first(where: { $0.hasPrefix("--diagram-focus-scene=") })?
            .dropFirst("--diagram-focus-scene=".count).lowercased() {
            result.merge(try await prepareScene(String(scene), workspace: workspace, window: window, document: document, arguments: arguments)) { _, new in new }
        }
        window.displayIfNeeded()
        return result
    }

    // MARK: 阅读视图 ⌘-点击

    private static func readingCommandClick(workspace: Workspace, window: NSWindow,
                                            document: NoteDocument) async throws -> [String: Any] {
        if !document.reading {
            await workspace.toggleCurrentView()
        }
        for _ in 0..<500 where document.reading {
            try await Task.sleep(for: .milliseconds(10))
        }
        let view = try await ReadingInspection.settled(true, document: document)
        guard let diagram = view.diagramViews.values.first(where: {
            $0.prepared.scene.hitRegions.contains(where: \.canCollapse)
        }), let hit = diagram.prepared.scene.hitRegions.first(where: \.canCollapse),
           let expected = diagram.prepared.sourceRanges[hit.nodeId] else {
            throw CocoaError(.coderValueNotFound)
        }
        commandClick(diagram, hit: hit, window: window)
        for _ in 0..<800 where document.reading {
            try await Task.sleep(for: .milliseconds(10))
        }
        let editing = try await DiagramInspection.settled(document: document)
        return ["reading_cmd_click": ["node_id": hit.nodeId,
                                      "expected": NSStringFromRange(expected),
                                      "selected": NSStringFromRange(editing.selectedRange()),
                                      "matches": editing.selectedRange() == expected,
                                      "left_reading": !document.reading]]
    }

    private static func commandClick(_ diagram: DiagramView, hit: SceneHit, window: NSWindow) {
        let point = diagram.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
        guard let event = NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [.command],
                                             timestamp: ProcessInfo.processInfo.systemUptime,
                                             windowNumber: window.windowNumber, context: nil,
                                             eventNumber: 0, clickCount: 1, pressure: 1) else { return }
        diagram.mouseDown(with: event)
    }

    // MARK: 右键菜单与无障碍

    private static func menuAndAccessibility(workspace: Workspace, window: NSWindow,
                                             document: NoteDocument) async throws -> [String: Any] {
        var view = try await DiagramInspection.settled(document: document)
        guard let ordinal = view.blocks.first(where: { $0.prepared != nil })?.ordinal,
              let diagram = view.diagramViews[ordinal], let prepared = view.blocks.first(where: { $0.ordinal == ordinal })?.prepared,
              let hit = prepared.scene.hitRegions.first(where: { prepared.sourceRanges[$0.nodeId] != nil }) else {
            throw CocoaError(.coderValueNotFound)
        }
        let point = diagram.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
        let event = NSEvent.mouseEvent(with: .rightMouseDown, location: point, modifierFlags: [],
                                       timestamp: ProcessInfo.processInfo.systemUptime,
                                       windowNumber: window.windowNumber, context: nil,
                                       eventNumber: 0, clickCount: 1, pressure: 1)
        let menu = event.flatMap { diagram.menu(for: $0) }
        let titles = menu?.items.filter { !$0.isSeparatorItem }.map(\.title) ?? []
        var menuSelection: String?
        if let item = menu?.items.first(where: { $0.title == L10n.string("Jump to Source") }), let action = item.action {
            NSApp.sendAction(action, to: item.target, from: item)
            try await Task.sleep(for: .milliseconds(30))
            view = try await DiagramInspection.settled(document: document)
            menuSelection = NSStringFromRange(view.selectedRange())
        }
        // 无障碍自定义动作：直接调用与旁白相同的 handler。
        var accessibilitySelection: String?
        var accessibilityCount = 0
        if let children = diagram.accessibilityChildren() as? [NSAccessibilityElement] {
            for child in children.prefix(diagram.accessibilityNodeCount) {
                let actions = child.accessibilityCustomActions() ?? []
                accessibilityCount += actions.count
                guard let action = actions.first, let handler = action.handler else { continue }
                if handler() {
                    try await Task.sleep(for: .milliseconds(30))
                    view = try await DiagramInspection.settled(document: document)
                    accessibilitySelection = NSStringFromRange(view.selectedRange())
                    break
                }
            }
        }
        let expected = prepared.sourceRanges[hit.nodeId].map(NSStringFromRange)
        return ["menu": ["titles": titles,
                         "has_jump": titles.contains(L10n.string("Jump to Source")),
                         "has_focus": titles.contains(L10n.string("Focus Mode")),
                         "selection": menuSelection as Any,
                         "expected": expected as Any,
                         "matches": (menuSelection != nil && menuSelection == expected)],
                "accessibility": ["count": accessibilityCount,
                                  "selection": accessibilitySelection as Any,
                                  "expected": expected as Any,
                                  "matches": (accessibilitySelection != nil && accessibilitySelection == expected)]]
    }

    // MARK: 源码光标高亮

    private static func sourceHighlight(workspace: Workspace, document: NoteDocument) async throws -> [String: Any] {
        var view = try await DiagramInspection.settled(document: document)
        guard let index = view.blocks.firstIndex(where: { $0.prepared != nil }),
              let prepared = view.blocks[index].prepared else { throw CocoaError(.coderValueNotFound) }
        let block = view.blocks[index]
        view.setSelectedRange(NSRange(location: block.range.location + 1, length: 0))
        for _ in 0..<300 where !view.expanded.contains(block.ordinal) || view.diagramPending {
            try await Task.sleep(for: .milliseconds(10))
            view = try await DiagramInspection.settled(document: document)
        }
        let before = view.layoutRenderCount
        var samples: [[String: Any]] = []
        var allMatched = true
        for hit in prepared.scene.hitRegions {
            guard let range = view.blocks.first(where: { $0.ordinal == block.ordinal })?
                .prepared?.sourceRanges[hit.nodeId] else { continue }
            view.setSelectedRange(NSRange(location: range.location, length: 0))
            try await Task.sleep(for: .milliseconds(6))
            let highlighted = view.diagramViews[block.ordinal]?.highlightedNodeID
            samples.append(["node_id": hit.nodeId, "expected": hit.nodeId,
                            "highlighted": highlighted as Any, "matches": highlighted == hit.nodeId])
            if highlighted != hit.nodeId { allMatched = false }
        }
        let afterSamples = view.layoutRenderCount
        // 折叠节点：光标停在隐藏后代行上必须回落到最近的可见祖先。
        var ancestor: [String: Any] = [:]
        if let hit = prepared.scene.hitRegions.first(where: \.canCollapse),
           let range = prepared.sourceRanges[hit.nodeId] {
            let source = view.string as NSString
            let nodeLine = source.lineRange(for: NSRange(location: range.location, length: 0))
            let nodeIndent = indentation(source, nodeLine.location)
            var descendant: Int?
            var position = NSMaxRange(nodeLine)
            while position < NSMaxRange(block.range) {
                let line = source.lineRange(for: NSRange(location: position, length: 0))
                let text = source.substring(with: line).trimmingCharacters(in: .whitespacesAndNewlines)
                if !text.isEmpty {
                    let indent = indentation(source, line.location)
                    if indent <= nodeIndent { break }
                    descendant = line.location
                    break
                }
                if NSMaxRange(line) <= position { break }
                position = NSMaxRange(line)
            }
            if let descendant {
                view.onToggle?(block.ordinal, hit.nodeId)
                for _ in 0..<300 {
                    try await Task.sleep(for: .milliseconds(10))
                    if !view.diagramPending,
                       let current = view.blocks.first(where: { $0.ordinal == block.ordinal }),
                       current.collapsed.contains(hit.nodeId) { break }
                }
                view.setSelectedRange(NSRange(location: descendant, length: 0))
                try await Task.sleep(for: .milliseconds(10))
                let highlighted = view.diagramViews[block.ordinal]?.highlightedNodeID
                ancestor = ["node_id": hit.nodeId, "descendant": descendant,
                            "highlighted": highlighted as Any, "matches": highlighted == hit.nodeId,
                            "collapsed": view.blocks.first(where: { $0.ordinal == block.ordinal })?.collapsed.contains(hit.nodeId) == true]
                // 恢复折叠，避免影响后续专注截图。
                view.onToggle?(block.ordinal, hit.nodeId)
                for _ in 0..<300 {
                    try await Task.sleep(for: .milliseconds(10))
                    if !view.diagramPending { break }
                }
            }
        }
        // 光标移出图块后必须清除高亮。
        view.setSelectedRange(NSRange(location: max(0, view.blocks[index].range.location - 1), length: 0))
        try await Task.sleep(for: .milliseconds(10))
        let cleared = view.diagramViews[block.ordinal]?.highlightedNodeID == nil
        let after = view.layoutRenderCount
        return ["highlight": ["samples": samples, "all_visible_matched": allMatched,
                              "ancestor": ancestor, "cleared": cleared,
                              "rust_layout_calls": afterSamples - before,
                              "ancestor_layout_calls": after - afterSamples]]
    }

    private static func indentation(_ source: NSString, _ lineStart: Int) -> Int {
        var column = 0, position = lineStart
        while position < source.length {
            let character = source.character(at: position)
            if character == 0x20 { column += 1 }
            else if character == 0x09 { column += 4 }
            else { break }
            position += 1
        }
        return column
    }

    // MARK: 专注模式闭环

    private static func focusLifecycle(workspace: Workspace, window: NSWindow,
                                       document: NoteDocument) async throws -> [String: Any] {
        func log(_ step: String) { fputs("P1-08 focus: \(step)\n", stderr) }
        if !document.reading { await workspace.toggleCurrentView() }
        _ = try await ReadingInspection.settled(true, document: document)
        var entry: [String: Any] = [:]
        if let diagram = document.mountedTextView?.diagramViews.values.first(where: { $0.onFocus != nil }),
           let button = diagram.subviews.compactMap({ $0 as? NSButton })
               .first(where: { $0.title == L10n.string("Focus") }) {
            button.performClick(nil)
            for _ in 0..<600 where workspace.focusSession == nil { try await Task.sleep(for: .milliseconds(5)) }
            entry["hover_button"] = workspace.focusSession != nil
            workspace.exitFocus(restore: false)
            for _ in 0..<600 where workspace.focusSession != nil { try await Task.sleep(for: .milliseconds(5)) }
        } else {
            entry["hover_button"] = false
        }
        if let item = menuItem(L10n.string("Focus Diagram")) {
            entry["menu"] = ["present": true, "key": item.keyEquivalent,
                             "modifiers": Int(item.keyEquivalentModifierMask.rawValue),
                             "enabled": item.isEnabled]
        } else {
            entry["menu"] = ["present": false]
        }
        workspace.toggleFocusMode()
        for _ in 0..<600 where workspace.focusSession == nil { try await Task.sleep(for: .milliseconds(5)) }
        entry["menu_action"] = workspace.focusSession != nil
        workspace.exitFocus(restore: false)
        for _ in 0..<600 where workspace.focusSession != nil { try await Task.sleep(for: .milliseconds(5)) }
        _ = try await ReadingInspection.settled(true, document: document)
        log("entry")
        workspace.enterFocus(block: try await targetOrdinal(workspace, document))
        log("wait focus")
        let focus = try await waitForFocus(window)
        log("focus ready")
        let canvas = focus.canvas
        var result: [String: Any] = ["focus": ["first_frame_ms": canvas.inspectionFirstFrame ?? -1, "entry": entry]]
        let editorProbe = probe("editor-column", in: window.contentView)
        let editorFrame = editorProbe.map { $0.convert($0.bounds, to: nil) } ?? .zero
        let focusFrame = focus.convert(focus.bounds, to: nil)
        var focusInfo = result["focus"] as? [String: Any] ?? [:]
        focusInfo["overlay_frame"] = NSStringFromRect(focusFrame)
        focusInfo["editor_frame"] = NSStringFromRect(editorFrame)
        focusInfo["inside_editor"] = editorFrame.insetBy(dx: -1, dy: -1).contains(focusFrame)
        focusInfo["covers_editor"] = focusFrame.width >= editorFrame.width * 0.8 && focusFrame.height >= editorFrame.height * 0.8
        focusInfo["toolbar_present"] = window.toolbar != nil
        if let left = probe("left-panel", in: window.contentView) {
            let leftFrame = left.convert(left.bounds, to: nil)
            focusInfo["left_panel_present"] = true
            focusInfo["sidebar_untouched"] = !focusFrame.intersects(leftFrame)
        } else {
            focusInfo["left_panel_present"] = false
            focusInfo["sidebar_untouched"] = true
        }
        // 缩放边界与快捷动作都不重排。
        let layoutBeforeZoom = canvas.inspectionRustLayoutCalls
        canvas.inspectionZoom(0.1)
        let low = canvas.inspectionScale
        canvas.inspectionZoom(10)
        let high = canvas.inspectionScale
        canvas.inspectionActual()
        let actual = canvas.inspectionScale
        canvas.inspectionFit()
        let fit = canvas.inspectionScale
        let layoutAfterZoom = canvas.inspectionRustLayoutCalls
        focusInfo["zoom"] = ["low": low, "high": high, "actual": actual, "fit": fit,
                             "rust_layout_calls": layoutAfterZoom - layoutBeforeZoom]
        log("zoom")
        // 小地图：放大后可见，点击中心把场景中心移到视口中心；适应后隐藏。
        canvas.inspectionZoom(4)
        let minimapVisible = !canvas.inspectionMinimapHidden
        let minimapBefore = canvas.inspectionOffset
        let scene = canvas.inspectionSceneSize
        let jumpExpected = CGPoint(x: canvas.inspectionViewport.midX - scene.width * 2,
                                   y: canvas.inspectionViewport.midY - scene.height * 2)
        canvas.inspectionMinimapJumpCenter()
        let afterJump = canvas.inspectionOffset
        canvas.inspectionFit()
        let minimapHiddenAtFit = canvas.inspectionMinimapHidden
        log("minimap")
        focusInfo["minimap"] = ["visible_when_zoomed": minimapVisible,
                                "hidden_at_fit": minimapHiddenAtFit,
                                "jump_delta": [afterJump.x - minimapBefore.x, afterJump.y - minimapBefore.y],
                                "jump_expected": [jumpExpected.x, jumpExpected.y],
                                "jump_matches": abs(afterJump.x - jumpExpected.x) <= 2 && abs(afterJump.y - jumpExpected.y) <= 2]
        // 节点搜索。
        var search: [String: Any] = [:]
        let hits = canvas.inspectionNodeHits()
        if let hit = hits.first(where: { !($0["label"] as? String ?? "").isEmpty }), let label = hit["label"] as? String {
            canvas.inspectionSearch(label)
            try await Task.sleep(for: .milliseconds(30))
            let matches = canvas.inspectionMatches
            canvas.inspectionNextMatch(1)
            let frame = canvas.inspectionCurrentMatchFrame()
            let viewport = canvas.inspectionViewport
            search = ["query": label, "matches": matches,
                      "current_in_viewport": frame.map { viewport.intersects($0) } ?? false]
        }
        // 折叠节点下的隐藏匹配与一键展开。
        log("hidden")
        var hidden: [String: Any] = [:]
        if let collapse = try await hiddenMatchSetup(workspace: workspace, view: document.mountedTextView,
                                                      document: document, canvas: canvas) {
            hidden = collapse
        }
        // ⌘F 只打开专注内搜索；⌘G / ⇧⌘G 在匹配间循环。
        var shortcuts: [String: Any] = [:]
        if let event = keyEvent("f", [.command], keyCode: 3, window: window) {
            _ = focus.performKeyEquivalent(with: event)
        }
        shortcuts["search_field_visible"] = focus.inspectionSearchVisible
        shortcuts["editor_find_bar"] = document.mountedTextView?.enclosingScrollView?.isFindBarVisible ?? false
        log("search")
        canvas.inspectionSearch("e")
        let cycleCount = canvas.inspectionMatches["visible"] as? Int ?? 0
        let beforeCycle = canvas.inspectionMatches["current"] as? Int ?? 0
        if let event = keyEvent("g", [.command], keyCode: 5, window: window) {
            _ = focus.performKeyEquivalent(with: event)
        }
        let afterCycle = canvas.inspectionMatches["current"] as? Int ?? 0
        if let event = keyEvent("G", [.command, .shift], keyCode: 5, window: window) {
            _ = focus.performKeyEquivalent(with: event)
        }
        let backCycle = canvas.inspectionMatches["current"] as? Int ?? 0
        shortcuts["cycle"] = ["count": cycleCount, "before": beforeCycle,
                              "after": afterCycle, "back": backCycle]
        focusInfo["shortcuts"] = shortcuts
        focusInfo["search"] = search
        focusInfo["hidden_search"] = hidden
        // 缩放/平移 300 帧：0 次重排，记录逐帧耗时。
        canvas.inspectionFit()
        log("frames")
        let layoutBeforeFrames = canvas.inspectionRustLayoutCalls
        let durations = canvas.inspectionMeasure(300)
        let layoutAfterFrames = canvas.inspectionRustLayoutCalls
        let sorted = durations.sorted()
        let p95 = sorted.isEmpty ? 0 : sorted[Int(Double(sorted.count - 1) * 0.95)]
        focusInfo["frames"] = ["count": durations.count, "p95_ms": p95,
                               "rust_layout_calls": layoutAfterFrames - layoutBeforeFrames]
        // 缓存：设置缩放与平移后写入，重进应恢复。
        canvas.inspectionZoom(2)
        canvas.inspectionSetOffset(CGPoint(x: -40, y: -30))
        let savedScale = canvas.inspectionScale, savedOffset = canvas.inspectionOffset
        canvas.inspectionPersist()
        try await Task.sleep(for: .milliseconds(400))
        log("command")
        // 专注模式内 ⌘-点击跳源码。
        var command: [String: Any] = [:]
        if let hit = canvas.inspectionNodeHits().first(where: { $0["can_collapse"] as? Bool == true }),
           let id = hit["id"] as? String,
           let expected = document.mountedTextView.flatMap({ text -> NSRange? in
               guard let block = text.blocks.first(where: { $0.prepared != nil }) else { return nil }
               return block.prepared?.sourceRanges[id]
           }) {
            canvas.inspectionCommandClick(id)
            for _ in 0..<800 where workspace.focusSession != nil {
                try await Task.sleep(for: .milliseconds(10))
            }
            for _ in 0..<800 where document.reading {
                try await Task.sleep(for: .milliseconds(10))
            }
            var editing: DiagramTextView?
            for _ in 0..<1000 {
                if let view = document.mountedTextView, !view.isReading, !view.diagramPending,
                   view.selectedRange() == expected {
                    editing = view
                    break
                }
                try await Task.sleep(for: .milliseconds(10))
            }
            if editing == nil { editing = try await DiagramInspection.settled(document: document) }
            let selected = editing?.selectedRange() ?? NSRange(location: NSNotFound, length: 0)
            command = ["node_id": id, "expected": NSStringFromRange(expected),
                       "selected": NSStringFromRange(selected),
                       "matches": selected == expected,
                       "exited": workspace.focusSession == nil, "reading": document.reading]
        }
        focusInfo["command_click"] = command
        result["focus"] = focusInfo
        log("restore editing")
        // 退出恢复：编辑视图的滚动与选区。
        let editing = try await DiagramInspection.settled(document: document)
        let editingSelection = NSRange(location: 120, length: 0)
        editing.setSelectedRange(editingSelection)
        try await Task.sleep(for: .milliseconds(60))
        let stableEditing = try await DiagramInspection.settled(document: document)
        stableEditing.enclosingScrollView?.contentView.scroll(to: CGPoint(x: 0, y: 80))
        stableEditing.enclosingScrollView?.reflectScrolledClipView(stableEditing.enclosingScrollView!.contentView)
        let editingScroll = stableEditing.enclosingScrollView?.contentView.bounds.origin ?? .zero
        workspace.enterFocus(block: try await targetOrdinal(workspace, document))
        _ = try await waitForFocus(window)
        workspace.exitFocus()
        for _ in 0..<800 where workspace.focusSession != nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        try await Task.sleep(for: .milliseconds(320))
        let restored = try await DiagramInspection.settled(document: document)
        result["restore_editing"] = ["reading": document.reading,
                                     "selection": NSStringFromRange(restored.selectedRange()),
                                     "expected_selection": NSStringFromRange(editingSelection),
                                     "selection_restored": restored.selectedRange() == editingSelection,
                                     "scroll": [restored.enclosingScrollView?.contentView.bounds.origin.x ?? 0,
                                                restored.enclosingScrollView?.contentView.bounds.origin.y ?? 0],
                                     "expected_scroll": [editingScroll.x, editingScroll.y]]
        log("restore reading")
        // 阅读视图进入/退出恢复。
        await workspace.toggleCurrentView()
        let readingView = try await ReadingInspection.settled(true, document: document)
        let scroll = readingView.enclosingScrollView?.contentView.bounds.origin ?? .zero
        workspace.enterFocus(block: try await targetOrdinal(workspace, document))
        _ = try await waitForFocus(window)
        workspace.exitFocus()
        for _ in 0..<800 where workspace.focusSession != nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        try await Task.sleep(for: .milliseconds(320))
        let back = try await ReadingInspection.settled(true, document: document)
        result["restore_reading"] = ["reading": document.reading,
                                     "scroll": [back.enclosingScrollView?.contentView.bounds.origin.x ?? 0,
                                                back.enclosingScrollView?.contentView.bounds.origin.y ?? 0],
                                     "expected_scroll": [scroll.x, scroll.y],
                                     "scroll_restored": abs((back.enclosingScrollView?.contentView.bounds.origin.y ?? -1) - scroll.y) <= 1]
        log("cache")
        // 缓存恢复（同一进程内）。
        workspace.enterFocus(block: try await targetOrdinal(workspace, document))
        let again = try await waitForFocus(window)
        try await Task.sleep(for: .milliseconds(80))
        result["cache_restore"] = ["scale": again.canvas.inspectionScale, "saved_scale": savedScale,
                                   "offset": [again.canvas.inspectionOffset.x, again.canvas.inspectionOffset.y],
                                   "saved_offset": [savedOffset.x, savedOffset.y],
                                   "scale_matches": abs(again.canvas.inspectionScale - savedScale) <= 0.001,
                                   "offset_matches": abs(again.canvas.inspectionOffset.x - savedOffset.x) <= 2
                                       && abs(again.canvas.inspectionOffset.y - savedOffset.y) <= 2]
        log("resize")
        // 窗口宽度变化：树形图栏数随宽度变化（先收窄再放宽）。
        var frame = window.frame
        let originalFrame = frame
        frame.size.width = 560
        window.setFrame(frame, display: true)
        try await Task.sleep(for: .milliseconds(800))
        let canvasBefore = again.canvas.inspectionViewport.width
        let beforeResize = again.canvas.inspectionRowCounts()
        let sceneWidth = again.canvas.inspectionSceneSize.width
        frame.size.width = 1100
        window.setFrame(frame, display: true)
        for _ in 0..<600 {
            try await Task.sleep(for: .milliseconds(10))
            if again.canvas.inspectionViewport.width > canvasBefore + 1 { break }
        }
        try await Task.sleep(for: .milliseconds(900))
        let afterResize = again.canvas.inspectionRowCounts()
        result["resize"] = ["window_frame": NSStringFromRect(window.frame),
                            "canvas_width_before": canvasBefore,
                            "canvas_width_after": again.canvas.inspectionViewport.width,
                            "scene_width_before": sceneWidth,
                            "scene_width_after": again.canvas.inspectionSceneSize.width,
                            "rows_before": beforeResize, "rows_after": afterResize,
                            "rows_changed": beforeResize != afterResize]
        window.setFrame(originalFrame, display: true)
        // 退出路径：Esc 与 ⇧⌘D 都必须关闭覆盖层。
        var exits: [String: Any] = [:]
        if let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
                                        timestamp: 0, windowNumber: window.windowNumber, context: nil,
                                        characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
                                        isARepeat: false, keyCode: 53) {
            again.canvas.keyDown(with: event)
        }
        for _ in 0..<600 where workspace.focusSession != nil { try await Task.sleep(for: .milliseconds(5)) }
        exits["escape"] = workspace.focusSession == nil
        workspace.enterFocus(block: try await targetOrdinal(workspace, document))
        let closing = try await waitForFocus(window)
        if let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.command, .shift],
                                        timestamp: 0, windowNumber: window.windowNumber, context: nil,
                                        characters: "D", charactersIgnoringModifiers: "D",
                                        isARepeat: false, keyCode: 2) {
            _ = closing.performKeyEquivalent(with: event)
        }
        for _ in 0..<600 where workspace.focusSession != nil { try await Task.sleep(for: .milliseconds(5)) }
        exits["shift_command_d"] = workspace.focusSession == nil
        result["exits"] = exits
        workspace.exitFocus()
        for _ in 0..<800 where workspace.focusSession != nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        result["reading_at_end"] = document.reading
        return result
    }

    private static func targetOrdinal(_ workspace: Workspace, _ document: NoteDocument) async throws -> UInt32 {
        _ = try await DiagramInspection.settled(document: document)
        guard let target = workspace.focusTarget() else { throw CocoaError(.coderValueNotFound) }
        return target.ordinal
    }

    private static func waitForFocus(_ window: NSWindow) async throws -> FocusModeView {
        for _ in 0..<1000 {
            if let focus = find(FocusModeView.self, in: window.contentView), focus.canvas.inspectionReady,
               focus.canvas.inspectionFirstFrame != nil {
                return focus
            }
            try await Task.sleep(for: .milliseconds(5))
        }
        throw CocoaError(.coderValueNotFound)
    }

    private static func hiddenMatchSetup(workspace: Workspace, view: DiagramTextView?, document: NoteDocument,
                                         canvas: FocusCanvasView) async throws -> [String: Any]? {
        let hits = canvas.inspectionNodeHits()
        func intPair(_ value: Any?) -> (Int, Int)? {
            guard let array = value as? [Any], array.count == 2 else { return nil }
            let first = (array[0] as? NSNumber)?.intValue ?? (array[0] as? Int)
            let second = (array[1] as? NSNumber)?.intValue ?? (array[1] as? Int)
            guard let first, let second else { return nil }
            return (first, second)
        }
        func isCollapsible(_ value: Any?) -> Bool {
            (value as? NSNumber)?.boolValue ?? (value as? Bool) ?? false
        }
        struct Entry { let id: String, label: String, start: Int, collapse: Bool }
        var entries: [Entry] = []
        for hit in hits {
            guard let range = intPair(hit["source_range"]), range.0 != NSNotFound,
                  let id = hit["id"] as? String, let label = hit["label"] as? String else { continue }
            entries.append(Entry(id: id, label: label, start: range.0, collapse: isCollapsible(hit["can_collapse"])))
        }
        entries.sort { $0.start < $1.start }
        var diagnostics: [String: Any] = ["hits": hits.count,
                                          "collapsible": entries.filter { $0.collapse }.count,
                                          "order": entries.map { ["id": $0.id, "label": $0.label, "start": $0.start] },
                                          "tried": []]
        for index in entries.indices where entries[index].collapse {
            guard index + 1 < entries.count, !entries[index + 1].label.isEmpty else { continue }
            let id = entries[index].id
            let label = entries[index + 1].label
            let layoutBefore = canvas.inspectionRustLayoutCalls
            canvas.inspectionToggle(id)
            for _ in 0..<500 {
                try await Task.sleep(for: .milliseconds(10))
                if canvas.inspectionRustLayoutCalls > layoutBefore,
                   canvas.inspectionCollapsed.contains(id),
                   canvas.inspectionNodeHits().contains(where: { $0["id"] as? String == id && isCollapsible($0["collapsed"]) }) {
                    break
                }
            }
            canvas.inspectionSearch(label)
            var hiddenCount = 0
            for _ in 0..<400 {
                try await Task.sleep(for: .milliseconds(10))
                hiddenCount = canvas.inspectionMatches["hidden"] as? Int ?? 0
                if hiddenCount > 0 { break }
            }
            guard hiddenCount > 0 else {
                canvas.inspectionToggle(id)
                for _ in 0..<300 {
                    try await Task.sleep(for: .milliseconds(10))
                    if !canvas.inspectionCollapsed.contains(id) { break }
                }
                var tried = diagnostics["tried"] as? [Any] ?? []
                tried.append(["id": id, "label": label, "hidden": hiddenCount])
                diagnostics["tried"] = tried
                continue
            }
            canvas.inspectionExpandHidden()
            for _ in 0..<500 {
                try await Task.sleep(for: .milliseconds(10))
                if (canvas.inspectionMatches["hidden"] as? Int ?? 0) == 0,
                   (canvas.inspectionMatches["visible"] as? Int ?? 0) > 0 { break }
            }
            try await Task.sleep(for: .milliseconds(60))
            let expanded = canvas.inspectionMatches
            return ["label": label, "collapsed_node": id,
                    "hidden_before": hiddenCount,
                    "visible_after": expanded["visible"] as? Int ?? 0,
                    "hidden_after": expanded["hidden"] as? Int ?? 0,
                    "collapsed_after": canvas.inspectionCollapsed,
                    "diagnostics": diagnostics]
        }
        return ["hidden_before": 0, "diagnostics": diagnostics]
    }

    // MARK: 截图场景

    private static func prepareScene(_ scene: String, workspace: Workspace, window: NSWindow,
                                     document: NoteDocument, arguments: [String]) async throws -> [String: Any] {
        workspace.exitFocus(restore: false)
        for _ in 0..<400 where workspace.focusSession != nil {
            try await Task.sleep(for: .milliseconds(5))
        }
        var view = document.reading ? try await ReadingInspection.settled(true, document: document)
            : try await DiagramInspection.settled(document: document)
        // 首屏可能还没挂上正文表面；进入专注前先等视图就绪。
        if document.reading {
            view = try await ReadingInspection.settled(true, document: document)
        } else {
            view = try await DiagramInspection.settled(document: document)
        }
        if scene == "source-highlight" {
            if document.reading { await workspace.toggleCurrentView(); view = try await DiagramInspection.settled(document: document) }
            guard let block = view.blocks.first(where: { $0.prepared != nil }), let prepared = block.prepared,
                  let hit = prepared.scene.hitRegions.first(where: { $0.canCollapse }) ?? prepared.scene.hitRegions.first,
                  let range = prepared.sourceRanges[hit.nodeId] else { return [:] }
            view.setSelectedRange(NSRange(location: block.range.location + 1, length: 0))
            try await Task.sleep(for: .milliseconds(200))
            view = try await DiagramInspection.settled(document: document)
            view.setSelectedRange(NSRange(location: range.location, length: 0))
            try await Task.sleep(for: .milliseconds(150))
            window.displayIfNeeded()
            return ["scene": scene]
        }
        workspace.enterFocus(block: try await targetOrdinal(workspace, document))
        let focus = try await waitForFocus(window)
        // 启动早期侧栏尺寸尚未落定；覆盖层必须与编辑列同宽后再摆场景。
        for _ in 0..<800 {
            if let editor = probe("editor-column", in: window.contentView) {
                let column = editor.convert(editor.bounds, to: nil)
                let frame = focus.convert(focus.bounds, to: nil)
                if abs(frame.width - column.width) <= 2, abs(frame.minX - column.minX) <= 2,
                   abs(frame.minY - column.minY) <= 2 { break }
            }
            try await Task.sleep(for: .milliseconds(10))
        }
        let canvas = focus.canvas
        switch scene {
        case "focus-overview":
            canvas.inspectionFit()
            // 全景仍保留小地图：略高于适应比例，让整图超出视口。
            canvas.inspectionZoom(canvas.inspectionScale * 1.18)
        case "focus-zoom-300":
            let hits = canvas.inspectionNodeHits()
            if let id = (hits.first(where: { $0["can_collapse"] as? Bool == true })?["id"] as? String)
                ?? (hits.first?["id"] as? String) {
                canvas.inspectionZoomTo(3, on: id)
            }
        case "focus-search":
            let query = arguments.first(where: { $0.hasPrefix("--diagram-focus-search=") })
                .map { String($0.dropFirst("--diagram-focus-search=".count)) } ?? ""
            let collapsible = canvas.inspectionNodeHits()
                .filter { $0["can_collapse"] as? Bool == true }
                .compactMap { $0["id"] as? String }
            if let id = collapsible.count > 1 ? collapsible[1] : collapsible.first {
                canvas.inspectionZoomTo(1.6, on: id)
                let layoutBefore = canvas.inspectionRustLayoutCalls
                canvas.inspectionToggle(id)
                for _ in 0..<600 {
                    try await Task.sleep(for: .milliseconds(10))
                    if canvas.inspectionRustLayoutCalls > layoutBefore,
                       canvas.inspectionCollapsed.contains(id),
                       canvas.inspectionNodeHits().contains(where: {
                           $0["id"] as? String == id && ($0["collapsed"] as? NSNumber)?.boolValue == true
                       }) { break }
                }
            }
            if !query.isEmpty { focus.inspectionShowSearch(query) }
        case "cache-check":
            break
        default:
            canvas.inspectionFit()
        }
        if scene != "cache-check" { canvas.inspectionPersist() }
        try await Task.sleep(for: .milliseconds(220))
        window.displayIfNeeded()
        return ["scene": scene, "state": canvas.screenshotState(), "layout": focus.inspectionLayout]
    }

    // MARK: 工具

    private static func keyEvent(_ characters: String, _ modifiers: NSEvent.ModifierFlags,
                                 keyCode: UInt16, window: NSWindow) -> NSEvent? {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers,
                         timestamp: 0, windowNumber: window.windowNumber, context: nil,
                         characters: characters, charactersIgnoringModifiers: characters,
                         isARepeat: false, keyCode: keyCode)
    }

    private static func menuItem(_ title: String) -> NSMenuItem? {
        func search(_ menu: NSMenu) -> NSMenuItem? {
            for item in menu.items {
                if item.title == title { return item }
                if let submenu = item.submenu, let found = search(submenu) { return found }
            }
            return nil
        }
        return NSApp.mainMenu.flatMap(search)
    }

    private static func probe(_ name: String, in root: NSView?) -> NSView? {
        guard let root else { return nil }
        if root.identifier?.rawValue == name { return root }
        for view in root.subviews { if let found = probe(name, in: view) { return found } }
        return nil
    }

    private static func find<T: NSView>(_ type: T.Type, in root: NSView?) -> T? {
        guard let root else { return nil }
        if let match = root as? T { return match }
        for view in root.subviews { if let found = find(type, in: view) { return found } }
        return nil
    }
}
#endif
