import AppKit
import CoreText
import Darwin
import HortusCore

/// 所有循环、等待、离屏计量都只在显式验收参数下执行。
@MainActor
enum DiagramInspection {
    private static var stabilityHandle: FileHandle?
    private static var stabilityStarted = 0.0
    private static var stabilityStartedAt = Date()
    private static let stabilityFormatter = ISO8601DateFormatter()

    private final class WeakView {
        weak var value: NSView?
        init(_ value: NSView) { self.value = value }
    }
    static func textViews(in view: NSView) -> [DiagramTextView] {
        (view as? DiagramTextView).map { [$0] } ?? view.subviews.flatMap { textViews(in: $0) }
    }
    static func current(document: NoteDocument? = nil) -> DiagramTextView? {
        let views = NSApp.windows.filter(\.isVisible).compactMap(\.contentView).flatMap { textViews(in: $0) }
        if let document { return views.first { $0.representedDocument == ObjectIdentifier(document) } }
        if let view = NSApp.keyWindow?.contentView.flatMap({ textViews(in: $0).first }) { return view }
        // 显式单窗口检查可能在应用尚未取得 key 状态时开始；多个窗口时不猜测目标。
        return views.count == 1 ? views.first : nil
    }
    static func sceneSignature(_ scene: DiagramScene) -> String {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        func mix(_ value: String) {
            for byte in value.utf8 {
                hash ^= UInt64(byte)
                hash &*= 0x0000_0100_0000_01b3
            }
        }
        mix(String(scene.width)); mix(String(scene.height))
        for primitive in scene.primitives { mix(String(reflecting: primitive)) }
        for hit in scene.hitRegions { mix(String(reflecting: hit)) }
        return String(format: "%016llx", hash)
    }
    static func finishEvent() {
        // 自动循环没有原生输入事件；结束 AppKit 的事件临时对象池。
        if let event = NSEvent.otherEvent(with: .applicationDefined, location: .zero, modifierFlags: [],
                                         timestamp: 0, windowNumber: 0, context: nil, subtype: 0,
                                         data1: 0, data2: 0) {
            NSApp.postEvent(event, atStart: false)
        }
    }
    static func settled(source: String? = nil, document: NoteDocument? = nil) async throws -> DiagramTextView {
        for _ in 0..<500 {
            if let view = current(document: document), !view.diagramPending, source == nil || view.string == source {
                autoreleasepool { view.layoutSubtreeIfNeeded() }
                return view
            }
            try await Task.sleep(for: .milliseconds(10))
        }
        throw CocoaError(.coderValueNotFound)
    }

    static func scrollerStyleName(_ style: NSScroller.Style) -> String {
        style == .legacy ? "legacy" : "overlay"
    }

    static func startScrollStabilityRecorder(arguments: [String]) {
        guard let argument = arguments.first(where: { $0.hasPrefix("--diagram-scroll-stability=") }) else { return }
        let output = URL(fileURLWithPath: String(argument.dropFirst("--diagram-scroll-stability=".count)))
        try? FileManager.default.createDirectory(at: output.deletingLastPathComponent(),
                                                 withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: output.path, contents: nil,
                                       attributes: [.posixPermissions: 0o600])
        guard let handle = FileHandle(forWritingAtPath: output.path) else { return }
        _ = try? handle.seekToEnd()
        stabilityHandle = handle
        stabilityStarted = ProcessInfo.processInfo.systemUptime
        stabilityStartedAt = Date()
        recordScrollStability(event: "started", view: nil)
    }

    /// 从主线程同步追加原始事件；即使布局循环占满主队列，也能保留失败现场。
    static func recordScrollStability(event: String, view: DiagramTextView?,
                                      details: [String: Any] = [:]) {
        guard let handle = stabilityHandle else { return }
        let scroll = view?.enclosingScrollView
        let style = scroll?.scrollerStyle ?? NSScroller.preferredScrollerStyle
        let elapsed = ProcessInfo.processInfo.systemUptime - stabilityStarted
        var row: [String: Any] = [
            "timestamp": stabilityFormatter.string(from: stabilityStartedAt.addingTimeInterval(elapsed)),
            "elapsed_seconds": elapsed,
            "event": event,
            "scrollbar_setting": UserDefaults.standard.string(forKey: "AppleShowScrollBars") ?? "system",
            "preferred_scroller_style": scrollerStyleName(NSScroller.preferredScrollerStyle),
            "scroller_style": scrollerStyleName(style),
        ]
        if let view, let scroll {
            let verticalScroller = scroll.verticalScroller
            let stableWidth = (scroll as? DiagramScrollView)?.stableContentWidth ?? view.bounds.width
            row.merge([
                "view_kind": view.isReading ? "reading" : "editing",
                "scroll_bounds": NSStringFromRect(scroll.bounds),
                "clip_bounds": NSStringFromRect(scroll.contentView.bounds),
                "document_bounds": NSStringFromRect(view.bounds),
                "document_frame": NSStringFromRect(view.frame),
                "content_size": NSStringFromSize(scroll.contentSize),
                "scroll_origin": [scroll.contentView.bounds.origin.x, scroll.contentView.bounds.origin.y],
                "vertical_scroller_visible": verticalScroller?.isHidden == false,
                "content_width": view.layoutContentWidth,
                "stable_content_width": stableWidth,
                "scene_widths": view.blocks.compactMap { $0.prepared?.scene.width },
                "scene_heights": view.blocks.compactMap { $0.prepared?.scene.height },
                "pending": view.diagramPending,
                "refresh_count": view.layoutRefreshCount,
                "render_count": view.layoutRenderCount,
                "cancellation_count": view.layoutCancellationCount,
                "window_width": view.window?.frame.width ?? 0,
            ]) { _, replacement in replacement }
        }
        row.merge(details) { _, replacement in replacement }
        guard let data = try? JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]),
              var line = String(data: data, encoding: .utf8) else { return }
        line.append("\n")
        try? handle.write(contentsOf: Data(line.utf8))
    }

    /// 在真实窗口中按固定间隔记录滚动布局状态，供 legacy / overlay 两态重算。
    static func scrollStability(workspace: Workspace, arguments: [String], output: URL) async throws {
        if stabilityHandle == nil { startScrollStabilityRecorder(arguments: arguments) }
        _ = output
        let reading = arguments.contains("--diagram-reading")
        if workspace.current?.document?.reading != reading { await workspace.toggleCurrentView() }
        var inspected: DiagramTextView?
        for _ in 0..<500 {
            if let view = current(document: workspace.current?.document), view.isReading == reading {
                inspected = view
                break
            }
            try await Task.sleep(for: .milliseconds(10))
        }
        guard let view = inspected, let scroll = view.enclosingScrollView else {
            throw CocoaError(.coderValueNotFound)
        }
        // 命令行默认域是按进程设置的测试输入；只在窗口内落实同一值，不写回系统设置。
        let preferredStyle = NSScroller.preferredScrollerStyle
        scroll.scrollerStyle = preferredStyle
        let settleDeadline = ProcessInfo.processInfo.systemUptime + 5
        while view.diagramPending, ProcessInfo.processInfo.systemUptime < settleDeadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        recordScrollStability(event: "observation_started", view: view)
        let started = ProcessInfo.processInfo.systemUptime
        let resizeViewport = arguments.contains("--diagram-stability-resize")
        let originalWindowFrame = view.window?.frame
        var index = 0
        repeat {
            if resizeViewport, index.isMultiple(of: 40), let window = view.window, let originalWindowFrame {
                var frame = originalWindowFrame
                frame.size.height = index.isMultiple(of: 80) ? 560 : 760
                window.setFrame(frame, display: true)
                scroll.contentView.scroll(to: CGPoint(x: 0, y: index.isMultiple(of: 80) ? 0 : 64))
                scroll.reflectScrolledClipView(scroll.contentView)
                scroll.flashScrollers()
            }
            let observationSeconds = ProcessInfo.processInfo.systemUptime - started
            recordScrollStability(event: "sample", view: view,
                                  details: ["sample": index, "observation_seconds": observationSeconds])
            if observationSeconds >= 8 { break }
            index += 1
            try await Task.sleep(for: .milliseconds(20))
        } while true
        recordScrollStability(event: "finished", view: view, details: ["samples": index + 1])
        if let originalWindowFrame { view.window?.setFrame(originalWindowFrame, display: true) }
    }
    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        let reading = arguments.contains("--diagram-reading")
        let originalSource = workspace.current?.text
        if workspace.current?.document?.reading != reading { await workspace.toggleCurrentView() }
        let memory = arguments.contains("--diagram-memory") ? try await memorySamples(workspace: workspace) : []
        var view: DiagramTextView
        if reading, let document = workspace.current?.document {
            view = try await ReadingInspection.settled(true, document: document)
        } else {
            view = try await settled(source: workspace.current?.text, document: workspace.current?.document)
        }
        let firstMain = view.maximumMainWorkMilliseconds
        if let argument = arguments.first(where: { $0.hasPrefix("--diagram-width=") }),
           let width = Double(argument.dropFirst("--diagram-width=".count)), width >= 120,
           let window = view.window {
            let sceneWidth = view.blocks.first?.prepared?.scene.width ?? view.layoutContentWidth
            var frame = window.frame
            frame.size.width += width - sceneWidth
            window.setFrame(frame, display: true)
            try await Task.sleep(for: .milliseconds(50))
            view = try await settled(document: workspace.current?.document)
            // 启动窗口尺寸可能来自上一次会话；按目标宽度重做初排，
            // 使同一文档、同一宽度的验收场景不受上次窗口宽度影响。
            await view.onFreshRender?()
            view = try await settled(document: workspace.current?.document)
        }
        if arguments.contains("--diagram-reveal"), let diagram = view.diagramViews.values.min(by: { $0.frame.minY < $1.frame.minY }) {
            view.scrollToVisible(CGRect(x: diagram.frame.minX, y: diagram.frame.minY, width: diagram.frame.width, height: min(500, diagram.frame.height)))
            view.displayIfNeeded()
        }
        let dark = view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let requestedWidth = arguments.first(where: { $0.hasPrefix("--diagram-width=") })
            .flatMap { Double($0.dropFirst("--diagram-width=".count)) }
            ?? view.layoutContentWidth
        var initialRows: [[Int]] = []
        var initialWidths: [Double] = []
        var initialSignatures: [String] = []
        var initialScenes: [String] = []
        if arguments.contains("--diagram-reference"), let source = originalSource, !view.blocks.isEmpty {
            let blocks = view.blocks
            let document = blocks.first?.documentIdentity ?? "diagram-inspection"
            let mmd = workspace.selected?.lowercased().hasSuffix(".mmd") == true
            let references = try await Task.detached {
                let engine = try DiagramEngine()
                return try blocks.map { block in
                    try engine.renderRequestInspection(source: source, mmd: mmd, width: requestedWidth,
                                                       collapsed: Array(block.collapsed),
                                                       identity: DiagramIdentity(document: document, block: block.ordinal,
                                                                                 revision: block.revision, dark: dark,
                                                                                 fontGeneration: 0, requestGeneration: 0),
                                                       previous: nil, viewport: [])
                }
            }.value
            initialRows = references.map { rowCounts($0.hitRegions) }
            initialWidths = references.map(\.width)
            initialSignatures = references.map { reference in
                (try? DiagramScene(reference)).map(DiagramInspection.sceneSignature) ?? ""
            }
            initialScenes = references.compactMap(\.semanticJson)
        }
        var anchorResize: [[String: Any]] = []
        if arguments.contains("--diagram-anchor"), let window = view.window {
            let original = window.frame
            for delta in [-60.0, 120.0] {
                let before = anchorSnapshot(view)
                var frame = original
                frame.size.width += delta
                window.setFrame(frame, display: true)
                try await Task.sleep(for: .milliseconds(30))
                view = try await settled(document: workspace.current?.document)
                let after = anchorSnapshot(view)
                let oldAnchors = Dictionary(uniqueKeysWithValues: (before["anchors"] as? [[String: Any]] ?? []).compactMap { item -> (String, CGPoint)? in
                    guard let id = item["node_id"] as? String, let x = item["x"] as? Double, let y = item["y"] as? Double else { return nil }
                    return (id, CGPoint(x: x, y: y))
                })
                let newAnchors = Dictionary(uniqueKeysWithValues: (after["anchors"] as? [[String: Any]] ?? []).compactMap { item -> (String, CGPoint)? in
                    guard let id = item["node_id"] as? String, let x = item["x"] as? Double, let y = item["y"] as? Double else { return nil }
                    return (id, CGPoint(x: x, y: y))
                })
                let distances = oldAnchors.keys.filter { newAnchors[$0] != nil }.map { id in
                    hypot(newAnchors[id]!.x - oldAnchors[id]!.x, newAnchors[id]!.y - oldAnchors[id]!.y)
                }.sorted()
                let medianValue: Double?
                if distances.isEmpty {
                    medianValue = nil
                } else if distances.count.isMultiple(of: 2) {
                    medianValue = (distances[distances.count / 2 - 1] + distances[distances.count / 2]) / 2
                } else {
                    medianValue = distances[distances.count / 2]
                }
                let median: Any = medianValue ?? NSNull()
                let oldGroups = before["groups"] as? [String: Int] ?? [:]
                let newGroups = after["groups"] as? [String: Int] ?? [:]
                let groupChanges = oldGroups.keys.filter { newGroups[$0] != nil && oldGroups[$0] != newGroups[$0] }.count
                anchorResize.append(["window_delta": delta, "before": before, "after": after,
                                     "common_anchors": distances.count,
                                     "median_displacement_px": median,
                                     "max_displacement_px": distances.last ?? NSNull(), "group_changes": groupChanges])
            }
            window.setFrame(original, display: true)
            try await Task.sleep(for: .milliseconds(30))
            view = try await settled(document: workspace.current?.document)
        }
        var beforeClick = 0
        if arguments.contains("--diagram-click") {
            guard let diagram = view.diagramViews.values.first, let window = diagram.window,
                  let hit = diagram.prepared.scene.hitRegions.first(where: \.canCollapse) else { throw CocoaError(.coderValueNotFound) }
            beforeClick = diagram.prepared.scene.hitRegions.count
            let point = diagram.convert(CGPoint(x: hit.rect.cgRect.midX, y: hit.rect.cgRect.midY), to: nil)
            guard let event = NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: [],
                                                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                                                context: nil, eventNumber: 0, clickCount: 1, pressure: 1) else { throw CocoaError(.coderValueNotFound) }
            diagram.mouseDown(with: event)
            try await Task.sleep(for: .milliseconds(100))
            for _ in 0..<100 {
                if let current = view.blocks.first(where: { $0.prepared?.scene.hitRegions.contains(where: { $0.nodeId == hit.nodeId && $0.collapsed != hit.collapsed }) == true }),
                   current.collapsed.contains(hit.nodeId) != hit.collapsed, !view.diagramPending { break }
                try await Task.sleep(for: .milliseconds(10))
            }
            view = try await settled(document: workspace.current?.document)
        }
        var resize: [[String: Any]] = []
        if arguments.contains("--diagram-resize"), let window = view.window {
            let original = window.frame
            for index in 0..<60 {
                var frame = original
                frame.size.width = original.width - 200 + CGFloat(index % 30) * 10
                window.setFrame(frame, display: true)
                try await Task.sleep(for: .milliseconds(10))
                view = try await settled(document: workspace.current?.document)
                if let block = view.blocks.first, let prepared = block.prepared, let diagram = view.diagramViews[block.ordinal] {
                    let visible = diagram.visibleRect.intersection(diagram.bounds)
                    let scale = window.backingScaleFactor
                    let draw = try drawMilliseconds(prepared, visible: visible, scale: scale)
                    resize.append(["width": prepared.scene.width, "height": prepared.scene.height, "layout_phases": view.layoutPhases, "render_ms": block.renderMilliseconds,
                                   "ffi_ms": block.transferMilliseconds, "install_ms": view.installMilliseconds,
                                   "rust_layout_ms": block.layoutMilliseconds,
                                   "install_phases": view.installPhases, "draw_ms": draw, "total_ms": block.renderMilliseconds + view.installMilliseconds + draw,
                                   "draw_width": visible.width, "draw_height": visible.height, "draw_scale": scale,
                                   "payload_bytes": prepared.payloadBytes])
                }
            }
            window.setFrame(original, display: true)
            try await Task.sleep(for: .milliseconds(20))
            view = try await settled(document: workspace.current?.document)
        }
        var errors: [Double] = []
        let diagrams = view.diagramViews.values.sorted { $0.frame.minY < $1.frame.minY }
        var accessibilityItems: [[String: Any]] = []
        var accessible = 0
        var accessibleEdges = 0
        var expandedValid = true
        for diagram in diagrams {
            errors += diagram.prepared.lines.map { abs(CTLineGetTypographicBounds($0.1, nil, nil, nil) - $0.0.width) }
            let nodes = Array((diagram.accessibilityChildren() as? [NSAccessibilityElement] ?? []).prefix(diagram.accessibilityNodeCount))
            accessible += diagram.accessibilityNodeCount
            accessibleEdges += diagram.accessibilityEdgeCount
            for (index, pair) in zip(nodes, diagram.prepared.scene.hitRegions).enumerated() {
                let (node, hit) = pair
                let expected = diagram.window?.convertToScreen(diagram.convert(hit.rect.cgRect, to: nil)) ?? .zero
                let actual = node.accessibilityFrame()
                let frameError = max(abs(expected.minX - actual.minX), abs(expected.minY - actual.minY), abs(expected.width - actual.width), abs(expected.height - actual.height))
                let labelMatches = node.accessibilityLabel() == hit.accessibilityLabel
                accessibilityItems.append(["kind": "node", "index": index, "label_matches": labelMatches, "frame_error_px": frameError])
                if hit.canCollapse { expandedValid = expandedValid && node.accessibilityRole() == .disclosureTriangle && node.isAccessibilityExpanded() == !hit.collapsed }
            }
            let children = diagram.accessibilityChildren() as? [NSAccessibilityElement] ?? []
            let paths = diagram.prepared.scene.primitives.compactMap { primitive -> (UInt64, [ScenePoint])? in
                guard case let .graphPath(id, _, points, _, _, _, _, _, _, _, _, _) = primitive else { return nil }
                return (id, points)
            }
            for (index, pair) in zip(children.dropFirst(nodes.count), paths).enumerated() {
                let (element, path) = pair
                guard let first = path.1.first else { throw CocoaError(.coderValueNotFound) }
                let pathBounds = path.1.dropFirst().reduce(CGRect(origin: first.cgPoint, size: .zero)) { $0.union(CGRect(origin: $1.cgPoint, size: .zero)) }.insetBy(dx: -5, dy: -5)
                let labelBounds = diagram.prepared.scene.primitives.compactMap { primitive -> CGRect? in
                    guard case let .labelBackground(id, rect, _) = primitive, id == path.0 else { return nil }
                    return rect.cgRect
                }.first
                let bounds = labelBounds ?? pathBounds
                let expected = diagram.window?.convertToScreen(diagram.convert(bounds, to: nil)) ?? .zero
                let actual = element.accessibilityFrame()
                let error = max(abs(expected.minX - actual.minX), abs(expected.minY - actual.minY), abs(expected.width - actual.width), abs(expected.height - actual.height))
                accessibilityItems.append(["kind": "edge", "index": index, "label_matches": element.accessibilityLabel() == diagram.prepared.edgeLabels[path.0], "frame_error_px": error])
            }
        }
        var customFragments = 0
        var errorFragments = 0
        if let manager = view.textLayoutManager, let content = manager.textContentManager {
            manager.enumerateTextLayoutFragments(from: content.documentRange.location, options: []) { fragment in
                if let fragment = fragment as? NoteLayoutFragment, fragment.replacement != nil { customFragments += 1 }
                if let fragment = fragment as? NoteLayoutFragment, fragment.reason != nil { errorFragments += 1 }
                return true
            }
        }
        errors.sort()
        let hits = diagrams.reduce(0) { $0 + $1.prepared.scene.hitRegions.count }
        let sourcePreserved: Bool
        if reading, let document = workspace.current?.document {
            sourcePreserved = try await document.io.perform { try $0.snapshot().text } == originalSource
                && view.blocks.allSatisfy { $0.revision == document.revision }
        } else { sourcePreserved = view.string == originalSource }
        let graphMetrics = diagrams.compactMap { $0.prepared.scene.graphMetrics }.map {
            ["roots": Int($0.roots), "edges": Int($0.edges), "node_overlaps": Int($0.nodeOverlaps),
             "invisible_edges": Int($0.invisibleEdges), "containers": Int($0.containers),
             "edge_crossings": Int($0.edgeCrossings), "horizontal_overflow": $0.horizontalOverflow ? 1 : 0]
        }
        let graphGeometry = diagrams.compactMap { diagram in
            diagram.prepared.scene.graphMetrics == nil ? nil : graphGeometry(diagram)
        }
        let fillCount = Set(diagrams.flatMap { diagram in
            diagram.prepared.scene.primitives.compactMap { primitive -> SceneColor? in
                if case let .graphShape(_, _, _, _, fill, _, _) = primitive { return fill }
                return nil
            }
        }).count
        var presses: [[String: Any]] = []
        if arguments.contains("--diagram-ax-press"), !reading {
            for diagram in diagrams {
                let children = diagram.accessibilityChildren() as? [NSAccessibilityElement] ?? []
                let ranges = diagram.prepared.scene.hitRegions.map { $0.canCollapse ? nil : diagram.prepared.sourceRanges[$0.nodeId] }
                    + diagram.prepared.scene.primitives.compactMap { primitive -> NSRange? in
                        guard case let .graphPath(id, _, _, _, _, _, _, _, _, _, _, _) = primitive else { return nil }
                        return diagram.prepared.edgeRanges[id]
                    }.map { Optional($0) }
                for (element, expected) in zip(children, ranges) {
                    guard let expected else { continue }
                    let performed = element.accessibilityPerformPress()
                    try await Task.sleep(for: .milliseconds(10))
                    presses.append(["performed": performed, "selection_matches": view.selectedRange() == expected])
                }
            }
        }
        var accessibilityContinuity: [String: Any] = [:]
        if arguments.contains("--diagram-continuity"),
           let diagram = diagrams.first,
           let index = diagram.prepared.scene.hitRegions.firstIndex(where: \.canCollapse),
           let child = (diagram.accessibilityChildren() as? [NSAccessibilityElement])?[index] {
            let nodeId = diagram.prepared.scene.hitRegions[index].nodeId
            let identity = ObjectIdentifier(child)
            NSAccessibility.post(element: child, notification: .focusedUIElementChanged)
            let collapsedPress = child.accessibilityPerformPress()
            try await Task.sleep(for: .milliseconds(30))
            view = try await settled(document: workspace.current?.document)
            let collapsedChildren = diagram.accessibilityChildren() as? [NSAccessibilityElement] ?? []
            let collapsedElement = zip(collapsedChildren, diagram.prepared.scene.hitRegions)
                .first { $0.1.nodeId == nodeId }?.0
            let collapsedState = collapsedElement?.isAccessibilityExpanded() == false
            let collapsedIdentity = collapsedElement.map(ObjectIdentifier.init) == identity
            if let collapsedElement { NSAccessibility.post(element: collapsedElement, notification: .focusedUIElementChanged) }
            let expandedPress = collapsedElement?.accessibilityPerformPress() ?? false
            try await Task.sleep(for: .milliseconds(30))
            view = try await settled(document: workspace.current?.document)
            let expandedChildren = diagram.accessibilityChildren() as? [NSAccessibilityElement] ?? []
            let expandedElement = zip(expandedChildren, diagram.prepared.scene.hitRegions)
                .first { $0.1.nodeId == nodeId }?.0
            let expandedState = expandedElement?.isAccessibilityExpanded() == true
            let expandedIdentity = expandedElement.map(ObjectIdentifier.init) == identity
            if let expandedElement { NSAccessibility.post(element: expandedElement, notification: .focusedUIElementChanged) }
            accessibilityContinuity = ["node_id": nodeId, "collapsed_press": collapsedPress,
                                       "expanded_press": expandedPress, "collapsed_state": collapsedState,
                                       "expanded_state": expandedState, "collapsed_identity_preserved": collapsedIdentity,
                                       "expanded_identity_preserved": expandedIdentity]
        }
        return ["diagrams": diagrams.count, "blocks": view.blocks.count, "reading": view.isReading,
                "graph_metrics": graphMetrics, "graph_geometry": graphGeometry, "graph_fill_count": fillCount,
                "scene_heights": diagrams.map { $0.prepared.scene.height },
                "row_counts": diagrams.map { diagram in
                    rowCounts(diagram.prepared.scene.hitRegions)
                },
                "initial_row_counts": initialRows, "initial_scene_widths": initialWidths,
                "initial_scene_signatures": initialSignatures,
                "initial_semantic_scenes": initialScenes,
                "dark": dark,
                "custom_fragments": customFragments, "error_fragments": errorFragments,
                "source_preserved": sourcePreserved, "textkit2": view.textLayoutManager != nil,
                "visible_nodes": hits, "accessibility_nodes": accessible, "accessibility_expanded_valid": expandedValid,
                "accessibility_edges": accessibleEdges,
                "accessibility_items": accessibilityItems, "accessibility_presses": presses,
                "accessibility_continuity": accessibilityContinuity,
                "heavy_work_thread_checks": view.blocks.map(\.heavyWorkThreadChecks),
                "heavy_work_thread_evidence": view.blocks.map(\.heavyWorkThreadEvidence),
                "source_hits": sourceHits(diagrams, source: workspace.current?.text ?? ""),
                "before_click_nodes": beforeClick,
                "collapsed_nodes": view.blocks.flatMap { Array($0.collapsed) }.count,
                "badge_count": diagrams.flatMap { $0.prepared.lines }.filter { $0.0.text.hasPrefix("+") }.count,
                "reasons": view.blocks.compactMap(\.reason),
                "diagnostics": view.blocks.compactMap(\.diagnostic),
                "width_max_px": errors.last ?? 0, "width_p95_px": errors.isEmpty ? 0 : errors[Int(Double(errors.count - 1) * 0.95)],
                "spans": errors.count, "first_main_work_max_ms": firstMain,
                "payload_bytes": diagrams.map { $0.prepared.payloadBytes },
                "scene_widths": diagrams.map { $0.prepared.scene.width },
                "scene_signatures": diagrams.map { sceneSignature($0.prepared.scene) },
                "requested_content_width": view.layoutContentWidth,
                "scrollbar_setting": UserDefaults.standard.string(forKey: "AppleShowScrollBars") ?? "system",
                "preferred_scroller_style": scrollerStyleName(NSScroller.preferredScrollerStyle),
                "scroller_style": scrollerStyleName(view.enclosingScrollView?.scrollerStyle ?? NSScroller.preferredScrollerStyle),
                "window_width": view.window?.frame.width ?? 0,
                "anchor_resize": anchorResize, "resize": resize, "memory": memory]
    }
    private static func anchorSnapshot(_ view: DiagramTextView) -> [String: Any] {
        guard let diagram = view.diagramViews.values.first else { return [:] }
        let anchors = diagram.viewportAnchors().map {
            ["node_id": $0.nodeId, "x": $0.screenCenter.x, "y": $0.screenCenter.y]
        }
        var rows: [Double: Int] = [:]
        for hit in diagram.prepared.scene.hitRegions { rows[hit.rect.y, default: 0] += 1 }
        let block = view.blocks.first
        let groups = Dictionary(uniqueKeysWithValues: (block?.previous?.nodes ?? []).map { ($0.nodeId, Int($0.group)) })
        return ["anchors": anchors, "scene_width": diagram.prepared.scene.width,
                "row_counts": rows.sorted { $0.key < $1.key }.map(\.value), "groups": groups]
    }
    private static func rowCounts(_ hits: [SceneHit]) -> [Int] {
        var rows: [Double: Int] = [:]
        for hit in hits { rows[hit.rect.y, default: 0] += 1 }
        return rows.sorted { $0.key < $1.key }.map(\.value)
    }

    /// 从最终 Scene 图元独立重算几何，避免只信任核心返回的汇总指标。
    private static func graphGeometry(_ diagram: DiagramView) -> [String: Any] {
        let scene = diagram.prepared.scene
        let hits = scene.hitRegions.map(\.rect.cgRect)
        let containerRects = scene.primitives.compactMap { primitive -> CGRect? in
            guard case let .graphShape(rect, shape, _, _, _, _, _) = primitive, shape == "Container" else { return nil }
            return rect.cgRect
        }
        let paths = scene.primitives.compactMap { primitive -> (UInt64, [ScenePoint])? in
            guard case let .graphPath(id, _, points, _, _, _, _, _, _, _, _, _) = primitive else { return nil }
            return (id, points)
        }
        let labels = scene.primitives.compactMap { primitive -> (UInt64, CGRect)? in
            guard case let .labelBackground(id, rect, _) = primitive else { return nil }
            return (id, rect.cgRect)
        }
        var nodeOverlaps = 0
        for index in hits.indices {
            for other in hits.index(after: index)..<hits.endIndex {
                let a = hits[index], b = hits[other]
                if a.minX < b.maxX - 0.01 && a.maxX > b.minX + 0.01
                    && a.minY < b.maxY - 0.01 && a.maxY > b.minY + 0.01
                    && !a.contains(b) && !b.contains(a) {
                    nodeOverlaps += 1
                }
            }
        }
        var edgeNode = 0, nonorthogonal = 0, horizontalOverflow = false
        for (_, points) in paths {
            for (index, pair) in zip(points, points.dropFirst()).enumerated() {
                let a = pair.0.cgPoint, b = pair.1.cgPoint
                if abs(a.x - b.x) > 0.001 && abs(a.y - b.y) > 0.001 { nonorthogonal += 1 }
                for rect in hits {
                    if containerRects.contains(rect) { continue }
                    let endpoint = (index == 0 && rect.contains(a)) || (index == points.count - 2 && rect.contains(b))
                    if !endpoint && segmentIntersects(a, b, rect) { edgeNode += 1 }
                }
            }
            for point in points where point.x < -0.01 || point.x > scene.width + 0.01 { horizontalOverflow = true }
        }
        for primitive in scene.primitives {
            switch primitive {
            case let .graphShape(rect, _, _, _, _, _, _), let .labelBackground(_, rect, _):
                if rect.cgRect.minX < -0.01 || rect.cgRect.maxX > scene.width + 0.01 { horizontalOverflow = true }
            case let .text(span):
                if span.baseline.x < -0.01 || span.baseline.x + span.width > scene.width + 0.01 { horizontalOverflow = true }
            default: break
            }
        }
        var crossings = 0, labelNode = 0, labelEdge = 0, labelLabel = 0, shared = 0, foreignShared = 0
        var crossingDetails: [[String: Any]] = []
        for first in paths.indices {
            for second in paths.indices where second > first {
                for a in zip(paths[first].1, paths[first].1.dropFirst()) {
                    for b in zip(paths[second].1, paths[second].1.dropFirst()) {
                        let ah = abs(a.0.y - a.1.y) < 0.001, bh = abs(b.0.y - b.1.y) < 0.001
                        if ah != bh {
                            let (horizontal, vertical) = ah ? (a, b) : (b, a)
                            let x = vertical.0.x, y = horizontal.0.y
                            if x > min(horizontal.0.x, horizontal.1.x) + 0.001
                                && x < max(horizontal.0.x, horizontal.1.x) - 0.001
                                && y > min(vertical.0.y, vertical.1.y) + 0.001
                                && y < max(vertical.0.y, vertical.1.y) - 0.001 {
                                crossings += 1
                                crossingDetails.append(["edge_a": paths[first].0, "edge_b": paths[second].0,
                                                        "point": [x, y], "path_a": pointsArray(paths[first].1),
                                                        "path_b": pointsArray(paths[second].1)])
                            }
                        } else if ah && abs(a.0.y - b.0.y) < 0.001
                            && min(a.0.x, a.1.x) < max(b.0.x, b.1.x) - 0.001
                            && min(b.0.x, b.1.x) < max(a.0.x, a.1.x) - 0.001 {
                            shared += 1
                            let endpoint = { (points: [ScenePoint], first: Bool) -> Int? in
                                let point = first ? points.first : points.last
                                guard let point else { return nil }
                                return hits.firstIndex { $0.contains(point.cgPoint) }
                            }
                            let sameSource = endpoint(paths[first].1, true) == endpoint(paths[second].1, true)
                            let sameTarget = endpoint(paths[first].1, false) == endpoint(paths[second].1, false)
                            if !sameSource && !sameTarget { foreignShared += 1 }
                        }
                    }
                }
            }
        }
        for (labelId, label) in labels {
            labelNode += hits.filter { intersects(label, $0) }.count
            for path in paths where path.0 != labelId {
                if zip(path.1, path.1.dropFirst()).contains(where: { segmentIntersects($0.0.cgPoint, $0.1.cgPoint, label) }) {
                    labelEdge += 1
                }
            }
        }
        for index in labels.indices {
            for other in labels.index(after: index)..<labels.endIndex where intersects(labels[index].1, labels[other].1) {
                labelLabel += 1
            }
        }
        return ["node_overlaps": nodeOverlaps, "edge_node": edgeNode, "nonorthogonal": nonorthogonal,
                "crossings": crossings, "crossing_details": crossingDetails,
                "shared_segments": shared, "foreign_shared_segments": foreignShared,
                "label_node": labelNode, "label_edge": labelEdge,
                "label_label": labelLabel, "horizontal_overflow": horizontalOverflow,
                "paths": paths.count, "expected_edges": Int(scene.graphMetrics?.edges ?? 0)]
    }

    private static func intersects(_ a: CGRect, _ b: CGRect) -> Bool {
        a.minX < b.maxX - 0.01 && a.maxX > b.minX + 0.01
            && a.minY < b.maxY - 0.01 && a.maxY > b.minY + 0.01
    }

    private static func segmentIntersects(_ a: CGPoint, _ b: CGPoint, _ rect: CGRect) -> Bool {
        if abs(a.x - b.x) < 0.001 {
            a.x > rect.minX + 0.001 && a.x < rect.maxX - 0.001
                && min(a.y, b.y) < rect.maxY - 0.001 && max(a.y, b.y) > rect.minY + 0.001
        } else {
            a.y > rect.minY + 0.001 && a.y < rect.maxY - 0.001
                && min(a.x, b.x) < rect.maxX - 0.001 && max(a.x, b.x) > rect.minX + 0.001
        }
    }

    private static func pointsArray(_ points: [ScenePoint]) -> [[Double]] {
        points.map { [$0.x, $0.y] }
    }
    /// 检查真实窗口中每个节点和边都有可回查的源码命中；公开结果只输出计数。
    private static func sourceHits(_ diagrams: [DiagramView], source: String) -> [String: Any] {
        var nodes = 0, nodeMatches = 0, edges = 0, edgeMatches = 0
        for diagram in diagrams where diagram.prepared.acceptsInteraction {
            let prepared = diagram.prepared
            for hit in prepared.scene.hitRegions {
                nodes += 1
                let rect = hit.rect.cgRect
                let candidates = [CGPoint(x: rect.midX, y: rect.midY),
                                  CGPoint(x: rect.minX + 4, y: rect.minY + 4),
                                  CGPoint(x: rect.midX, y: rect.minY + 8)]
                if let range = prepared.sourceRanges[hit.nodeId],
                   candidates.contains(where: { diagram.sourceRange(at: $0) == range }),
                   DiagramService.byteRange(range, in: source) == (Int(hit.sourceStart)..<Int(hit.sourceEnd)) {
                    nodeMatches += 1
                }
            }
            for primitive in prepared.scene.primitives {
                guard case let .graphPath(id, _, points, start, end, _, _, _, _, _, _, _) = primitive else { continue }
                edges += 1
                guard let range = prepared.edgeRanges[id],
                      DiagramService.byteRange(range, in: source) == (Int(start)..<Int(end)) else { continue }
                let matched = zip(points, points.dropFirst()).contains { a, b in
                    [0.2, 0.5, 0.8].contains { t in
                        diagram.sourceRange(at: CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t)) == range
                    }
                }
                if matched { edgeMatches += 1 }
            }
        }
        return ["nodes": nodes, "node_matches": nodeMatches, "edges": edges, "edge_matches": edgeMatches]
    }

    private static func memorySamples(workspace: Workspace) async throws -> [[String: Any]] {
        guard let path = workspace.selected else { throw CocoaError(.coderValueNotFound) }
        var memory: [[String: Any]] = [], oldText: [WeakView] = [], oldDiagrams: [WeakView] = []
        var textIdentities: Set<ObjectIdentifier> = []
        for index in 0..<20 {
            autoreleasepool {
                if let view = current(document: workspace.current?.document) {
                    textIdentities.insert(ObjectIdentifier(view))
                    oldText.append(WeakView(view))
                    oldDiagrams += view.diagramViews.values.map { WeakView($0) }
                }
            }
            // 每轮任务结束后再采样，避免自检异步栈持有上轮的原生视图。
            let cycleValues = try await Task { @MainActor in
                workspace.closeTab(path)
                for _ in 0..<500 {
                    if !workspace.tabs.contains(where: { $0.path == path }) { break }
                    try await Task.sleep(for: .milliseconds(10))
                }
                guard !workspace.tabs.contains(where: { $0.path == path }) else { throw CocoaError(.coderValueNotFound) }
                finishEvent()
                try await Task.sleep(for: .milliseconds(200))
                let closedRaw = try residentBytes()
                relieveFreePages()
                let closed = try residentBytes()
                let closedFootprint = try footprintBytes()
                let closedMalloc = mallocState()
                workspace.openNote(path)
                for _ in 0..<500 {
                    if workspace.current?.loading == false, current(document: workspace.current?.document) != nil { break }
                    try await Task.sleep(for: .milliseconds(10))
                }
                if workspace.current?.document?.reading == true { await workspace.toggleCurrentView() }
                let view = try await settled(source: workspace.current?.text, document: workspace.current?.document)
                autoreleasepool {
                    view.displayIfNeeded()
                    for diagram in view.diagramViews.values { diagram.displayIfNeeded() }
                }
                let window = view.window
                let diagram = view.diagramViews.values.first
                let openedMalloc = mallocState()
                let openedRaw = try residentBytes()
                relieveFreePages()
                let openedFootprint = try footprintBytes()
                let openedBytes = try residentBytes()
                return (closed, closedRaw, closedFootprint, closedMalloc.0, closedMalloc.1,
                        openedFootprint, openedBytes, openedRaw, openedMalloc.0, openedMalloc.1,
                        window?.frame ?? .zero, window?.isVisible == true,
                        window?.occlusionState.contains(.visible) == true, NSApp.isActive,
                        window?.backingScaleFactor ?? 0,
                        diagram.map { [$0.frame.width, $0.frame.height] } ?? [])
            }.value
            let (closed, closedRaw, closedFootprint, closedMallocInUse, closedMallocAllocated,
                 openedFootprint, openedBytes, openedRaw, openedMallocInUse, openedMallocAllocated,
                 windowFrame, windowVisible, windowOccluded, appActive, backingScale, diagramFrame) = cycleValues
            finishEvent()
            try await Task.sleep(for: .milliseconds(200))
            var usage = rusage()
            getrusage(RUSAGE_SELF, &usage)
            memory.append(["cycle": index + 1, "opened_bytes": openedBytes, "closed_bytes": closed,
                           "opened_raw_bytes": openedRaw, "closed_raw_bytes": closedRaw,
                           "opened_footprint_bytes": openedFootprint, "closed_footprint_bytes": closedFootprint,
                           "opened_malloc_in_use": openedMallocInUse, "opened_malloc_allocated": openedMallocAllocated,
                           "closed_malloc_in_use": closedMallocInUse, "closed_malloc_allocated": closedMallocAllocated,
                           "peak_bytes": usage.ru_maxrss, "text_view_instances": textIdentities.count,
                           "window_frame": NSStringFromRect(windowFrame), "window_visible": windowVisible,
                           "window_occluded": !windowOccluded, "app_active": appActive,
                           "backing_scale": backingScale, "diagram_frame": diagramFrame,
                           "retained_old_text_views": oldText.filter { $0.value != nil && $0.value !== current(document: workspace.current?.document) }.count,
                           "retained_old_diagram_views": oldDiagrams.filter { $0.value != nil }.count])
        }
        return memory
    }

    /// RSS 含分配器保留的空闲页；采样前归还它们，保留仍在使用的对象。
    private static func relieveFreePages() {
        malloc_zone_pressure_relief(nil, 0)
    }

    /// 与 RSS 并列的真实占用与分配器统计，用来区分 retained 对象、空闲页和共享页。
    private static func footprintBytes() throws -> UInt64 {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.stride / MemoryLayout<integer_t>.stride)
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard result == KERN_SUCCESS else { throw CocoaError(.fileReadUnknown) }
        return info.phys_footprint
    }

    private static func mallocState() -> (inUse: UInt64, allocated: UInt64) {
        var stats = malloc_statistics_t()
        malloc_zone_statistics(malloc_default_zone(), &stats)
        return (UInt64(stats.size_in_use), UInt64(stats.size_allocated))
    }
    private static func drawMilliseconds(_ prepared: PreparedScene, visible: CGRect, scale: CGFloat) throws -> Double {
        guard !visible.isEmpty else { return 0 }
        // 与原生视图相同的可见区域、坐标和 backing scale，不为屏幕外的整张长图分配位图。
        guard let context = CGContext(data: nil, width: Int(ceil(visible.width * scale)), height: Int(ceil(visible.height * scale)),
                                      bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { throw CocoaError(.coderValueNotFound) }
        context.scaleBy(x: scale, y: scale)
        context.translateBy(x: -visible.minX, y: visible.height + visible.minY)
        context.scaleBy(x: 1, y: -1)
        context.clip(to: visible)
        let start = ProcessInfo.processInfo.systemUptime
        SceneRenderer.draw(prepared, in: context)
        return (ProcessInfo.processInfo.systemUptime - start) * 1000
    }
    private static func residentBytes() throws -> UInt64 {
        var info = proc_taskinfo()
        let size = Int32(MemoryLayout.size(ofValue: info))
        guard proc_pidinfo(getpid(), PROC_PIDTASKINFO, 0, &info, size) == size else { throw CocoaError(.fileReadUnknown) }
        return info.pti_resident_size
    }
}
