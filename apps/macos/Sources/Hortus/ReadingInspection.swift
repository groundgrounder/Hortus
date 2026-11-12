import AppKit
import ApplicationServices
import Darwin
import HortusCore
import Quartz

/// 显式验收参数下记录切换与 reveal 的原始状态；默认不分配文件，也不参与布局决策。
@MainActor
enum AnchorInspection {
    private static var handle: FileHandle?
    private static var startedAt = 0.0
    private(set) static var completedToggles = 0
    private(set) static var records: [[String: Any]] = []
    static var phase = ""
    static var isActive: Bool { handle != nil }

    /// 兼容仍返回非可选位置的旧检查入口。
    static func sourceValue(_ value: Int) -> Int { value }
    static func sourceValue(_ value: Int?) -> Int { value ?? -1 }

    static func beginPhase(_ name: String) {
        guard isActive else { return }
        phase = name
        record("phase", [:])
    }

    static func start(arguments: [String]) {
        guard let argument = arguments.first(where: { $0.hasPrefix("--anchor-inspection=") }) else { return }
        let output = URL(fileURLWithPath: String(argument.dropFirst("--anchor-inspection=".count)))
        try? FileManager.default.createDirectory(at: output.deletingLastPathComponent(),
                                                 withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: output.path, contents: nil, attributes: [.posixPermissions: 0o600])
        handle = FileHandle(forWritingAtPath: output.path)
        _ = try? handle?.seekToEnd()
        startedAt = ProcessInfo.processInfo.systemUptime
        completedToggles = 0
        records = []
        record("started", [:])
    }

    static func record(_ event: String, _ details: [String: Any]) {
        guard handle != nil else { return }
        var row = details
        row["event"] = event
        row["phase"] = phase
        row["t_ms"] = (ProcessInfo.processInfo.systemUptime - startedAt) * 1000
        if event == "toggle_exit" { completedToggles += 1 }
        records.append(row)
        guard JSONSerialization.isValidJSONObject(row),
              let data = try? JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]) else { return }
        try? handle?.write(contentsOf: data + Data([0x0a]))
        handle?.synchronizeFile()
    }

    /// 原生视图与阅读协调器的当前状态；只在检查启用时计算。
    static func snapshot(_ view: DiagramTextView?, document: NoteDocument?) -> [String: Any] {
        var row: [String: Any] = [:]
        if let document {
            row["document_reading"] = document.reading
            row["anchor"] = document.sourceAnchor
            row["anchor_length"] = document.sourceSelectionLength
            row["doc_utf16"] = document.text.utf16.count
            row["revision"] = document.revision
        }
        guard let view else { row["view"] = "none"; return row }
        let visible = view.visibleRect
        let sample = view.characterIndexForInsertion(at: CGPoint(x: 31, y: visible.minY + 1))
        let selected = view.selectedRange()
        row["view"] = String(describing: ObjectIdentifier(view))
        row["view_reading"] = view.isReading
        row["view_in_window"] = view.window != nil
        row["view_utf16"] = view.sourceUTF16Length
        row["content_width"] = Double(view.layoutContentWidth)
        row["window_width"] = Double(view.window?.frame.width ?? 0)
        row["visible"] = [Double(visible.minX), Double(visible.minY),
                          Double(visible.width), Double(visible.height)]
        row["document_height"] = Double(view.frame.height)
        row["bounds_height"] = Double(view.bounds.height)
        row["usage_height"] = Double(view.textLayoutManager?.usageBoundsForTextContainer.maxY ?? 0)
        row["sample"] = sample
        row["selected"] = [selected.location, selected.length]
        row["at_bottom"] = visible.maxY >= view.frame.height - 1
        if let coordinator = view.readingCoordinator {
            row["ranges"] = coordinator.ranges.count
            row["model_utf16"] = coordinator.model?.text.utf16.count ?? -1
            row["model_revision"] = coordinator.model?.revision ?? 0
            row["model_pending"] = coordinator.pending
            row["reveal_pending"] = coordinator.revealPending
            row["sample_source"] = sourceValue(coordinator.sourcePosition(at: sample))
            row["anchor_display"] = coordinator.displayPosition(source: document?.sourceAnchor ?? 0)
            if let anchor = coordinator.visibleAnchor() {
                row["visible_block"] = String(describing: anchor.id)
                row["visible_source"] = anchor.source
                row["visible_offset"] = anchor.offset
            }
        }
        return row
    }
}

/// 真实窗口计量只由显式验收参数启动，日常运行不增加轮询。
@MainActor
enum ReadingInspection {
    private struct ReferenceScene: Sendable {
        let ordinal: Int
        let width: Double
        let height: Double
        let signature: String
    }

    @MainActor
    private final class WeakView {
        weak var value: DiagramTextView?
        weak var readingCoordinator: ReadingText.Coordinator?
        weak var sourceStyling: SourceStyling?
        weak var editingCoordinator: NoteText.EditorCoordinator?
        init(_ value: DiagramTextView) {
            self.value = value; readingCoordinator = value.readingCoordinator
            sourceStyling = value.sourceStyling; editingCoordinator = value.editingCoordinator
        }
    }
    static func settled(_ reading: Bool, document: NoteDocument) async throws -> DiagramTextView {
        for _ in 0..<10000 {
            if let view = DiagramInspection.current(document: document), view.isReading == reading,
               view.representedDocument == ObjectIdentifier(document), !view.diagramPending,
               reading ? view.readingCoordinator?.pending == false && view.readingCoordinator?.revealPending == false
                    && view.readingCoordinator?.model?.revision == document.revision
                    : view.sourceStyling?.pending == false {
                // 自检从异步任务直接驱动 AppKit，像原生事件一样结束临时对象池。
                autoreleasepool { view.layoutSubtreeIfNeeded(); view.displayIfNeeded() }
                if view.mediaViews.contains(where: { !$0.isHidden && $0.imagePending }) {
                    try await Task.sleep(for: .milliseconds(1)); continue
                }
                return view
            }
            try await Task.sleep(for: .milliseconds(1))
        }
        throw CocoaError(.coderValueNotFound)
    }
    private static func retainedResources(_ references: [WeakView], active: DiagramTextView) -> Int {
        references.compactMap(\.value).filter { view in
            guard view !== active else { return false }
            let reference = references.first { $0.value === view }
            return (view.textStorage?.length ?? 0) > 0 || !view.diagramViews.isEmpty
                || view.diagramStorage != nil || view.textLayoutManager?.textContentManager != nil
                || reference?.readingCoordinator?.model != nil
                || reference?.sourceStyling?.runs.isEmpty == false
                || (reference?.editingCoordinator?.retainedBlockCount ?? 0) > 0
                || (reference?.editingCoordinator?.document != nil)
        }.count
    }
    static func run(workspace: Workspace, layout: Layout?, arguments: [String]) async throws -> [String: Any] {
        guard let document = workspace.current?.document, let path = workspace.selected else { throw CocoaError(.coderValueNotFound) }
        let mode = arguments.first { $0.hasPrefix("--reading-check=") }.map { String($0.dropFirst("--reading-check=".count)) } ?? "visual"
        if mode == "math-memory" {
            return try await MathInspection.runMemory(workspace: workspace, path: path, arguments: arguments)
        }
        var view = try await settled(document.reading, document: document)
        let openedAt = view.isReading ? view.readingCoordinator?.displayedAt ?? 0 : view.sourceStyling?.openDisplayedAt ?? 0
        let source = try await document.io.perform { try $0.snapshot().text }
        if mode == "diagram-toggle-spacing" {
            return try await diagramToggleSpacing(document: document, view: view, source: source, path: path)
        }
        if mode == "rapid-toggle-anchor" {
            AnchorInspection.start(arguments: arguments)
            return try await rapidToggleAnchor(document: document, view: view, source: source, path: path)
        }
        if mode == "diagram-narrow" {
            return try await diagramNarrow(workspace: workspace, layout: layout, document: document, view: view,
                                           source: source, path: path, arguments: arguments)
        }
        var result: [String: Any] = ["reading": view.isReading, "textkit2": view.textLayoutManager != nil,
            "utf8_bytes": source.utf8.count, "open_ms": (openedAt - workspace.noteOpenStartedAt) * 1000,
            "resident_bytes": try residentBytes(), "display_utf16": view.sourceUTF16Length,
            "diagrams": view.diagramViews.count, "rebuilt": view.readingCoordinator?.model?.rebuilt ?? 0]
        result["reading_phases"] = view.readingCoordinator?.phaseMilliseconds ?? [:]
        result["media"] = view.mediaBlocks.count
        if arguments.contains("--inspect-reading-text") {
            result["display_text"] = view.string
            result["table_cells"] = view.mediaBlocks.flatMap { item in
                item.prepared.block.cells.map { cell in
                    ["source": cell.source, "text": cell.text,
                     "masks": cell.runs.map(\.mask),
                     "formulas": cell.math.map { ["latex": $0.latex, "block": $0.block] as [String: Any] }] as [String: Any]
                }
            }
            result["table_drawn_cells"] = view.mediaViews.flatMap(\.tableInspectionCells)
        }
        if mode.hasPrefix("math") {
            return try await MathInspection.run(workspace: workspace, document: document, view: view,
                source: source, mode: mode, arguments: arguments, initial: result)
        }
        for _ in 0..<1000 {
            if document.outlineRevision == document.revision { break }
            try await Task.sleep(for: .milliseconds(1))
        }
        result["outline_count"] = document.headings.count
        result["outline_analysis_ms"] = document.outlineMilliseconds
        result["outline_ready_ms"] = (document.outlineReadyAt - workspace.noteOpenStartedAt) * 1000
        result["outline_revision_matches"] = document.outlineRevision == document.revision
        if !document.headings.isEmpty {
            for _ in 0..<1000 {
                if document.outlineDisplayedAt > 0 { break }
                try await Task.sleep(for: .milliseconds(1))
            }
            result["outline_displayed_ms"] = (document.outlineDisplayedAt - workspace.noteOpenStartedAt) * 1000
        }
        if mode == "outline" {
            var visible = 0
            for heading in document.headings {
                workspace.revealHeading(heading)
                view.layoutSubtreeIfNeeded()
                if let coordinator = view.readingCoordinator {
                    let position = coordinator.displayPosition(source: Int(heading.start))
                    if view.visibleRect.intersects(coordinator.rect(at: position)) { visible += 1 }
                } else if view.selectedRange() == NSRange(location: Int(heading.start), length: 0) {
                    visible += 1
                }
            }
            result["outline_correct_targets"] = visible
            result["outline_titles"] = document.headings.map(\.title)
            result["outline_levels"] = document.headings.map(\.level)
            if let first = document.headings.first { workspace.revealHeading(first) }
        }
        result["visible_images"] = view.mediaViews.filter { !$0.isHidden && $0.hasDecodedImage }.count
        if mode == "media" {
            let attachments = try await document.io.perform { $0.attachmentMetrics() }
            result["attachment_index_files"] = attachments.files
            result["attachment_index_directories"] = attachments.directories
            result["attachment_index_build_ms"] = Double(attachments.buildMicros) / 1000
            result["attachment_index_estimated_bytes"] = attachments.estimatedBytes
            result["attachment_index_builds"] = attachments.builds
            result["media_reasons"] = view.mediaBlocks.map { $0.prepared.reason }
            result["visible_painted_ms"] = (max(openedAt, view.mediaPaintedAt) - workspace.noteOpenStartedAt) * 1000
            result["image_decodes"] = await LocalImageCache.shared.decodes
            result["main_thread_decodes"] = await LocalImageCache.shared.mainThreadDecodes
            result["tables"] = view.mediaBlocks.filter { !$0.prepared.block.columns.isEmpty }.count
            result["media_frames"] = view.mediaViews.filter { !$0.isHidden }.map { NSStringFromRect($0.frame) }
            if arguments.contains("--media-preview"),
               let image = view.mediaViews.first(where: { !$0.isHidden && $0.hasDecodedImage }),
               let event = NSEvent.mouseEvent(with: .leftMouseDown, location: .zero, modifierFlags: [],
                    timestamp: 0, windowNumber: view.window?.windowNumber ?? 0, context: nil,
                    eventNumber: 0, clickCount: 1, pressure: 1) {
                image.mouseDown(with: event)
                try await Task.sleep(for: .milliseconds(200))
                guard let panel = QLPreviewPanel.shared(), panel.isVisible, panel.dataSource === image,
                      image.numberOfPreviewItems(in: panel) == 1 else { throw CocoaError(.coderValueNotFound) }
                result["quick_look_visible"] = true
                panel.orderOut(nil)
            }
            return result
        }
        result["open_phases"] = workspace.noteOpenPhases
        if let firstRefresh = view.readingCoordinator?.firstRefreshStartedAt, firstRefresh > 0 {
            result["pre_render_ms"] = (firstRefresh - workspace.noteOpenStartedAt) * 1000
        }
        result["refresh_to_display_ms"] = view.readingCoordinator?.latencyMilliseconds
        if mode == "visual" {
            if arguments.contains("--reading-editing"), document.reading {
                await workspace.toggleCurrentView(); view = try await settled(false, document: document)
            }
            if arguments.contains("--media-source"), !view.isReading,
               let table = view.mediaBlocks.first(where: { !$0.prepared.block.columns.isEmpty }) {
                view.setSelectedRange(NSRange(location: table.range.location + 2, length: 0))
                view.editingCoordinator?.mediaSelectionChanged()
                view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
            }
            result["reading"] = view.isReading
            return result
        }
        if mode == "performance" {
            var frames: [Double] = []
            for i in 0..<240 {
                let start = ProcessInfo.processInfo.systemUptime
                let y = CGFloat(i < 120 ? i : 239 - i) * 65
                view.enclosingScrollView?.contentView.scroll(to: CGPoint(x: 0, y: y))
                view.enclosingScrollView?.reflectScrolledClipView(view.enclosingScrollView!.contentView)
                view.needsLayout = true; view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
                frames.append((ProcessInfo.processInfo.systemUptime - start) * 1000)
                try await Task.sleep(for: .milliseconds(1))
            }
            frames.sort(); result["scroll_p95_ms"] = frames[Int(Double(frames.count - 1) * 0.95)]
            result["scroll_max_ms"] = frames.last; result["scroll_samples"] = frames.count
            workspace.openNote("other.md")
            for _ in 0..<10000 {
                if let other = workspace.current?.document { _ = try await settled(other.reading, document: other); break }
                try await Task.sleep(for: .milliseconds(1))
            }
            workspace.openNote(path)
            view = try await settled(document.reading, document: document)
            let reopenedAt = view.isReading ? view.readingCoordinator?.displayedAt ?? 0 : view.sourceStyling?.openDisplayedAt ?? 0
            result["cached_open_ms"] = (reopenedAt - workspace.noteOpenStartedAt) * 1000
            result["cached_rebuilt"] = view.readingCoordinator?.model?.rebuilt ?? 0
            var switches: [Double] = []
            var old: [WeakView] = []
            for _ in 0..<4 {
                old.append(WeakView(view))
                let began = ProcessInfo.processInfo.systemUptime
                await workspace.toggleCurrentView(); view = try await settled(document.reading, document: document)
                let end = view.isReading ? view.readingCoordinator?.displayedAt ?? 0 : view.sourceStyling?.openDisplayedAt ?? 0
                switches.append((end - began) * 1000)
            }
            result["switch_ms"] = switches
            result["retained_old_views"] = old.filter { $0.value != nil && $0.value !== view }.count
            result["retained_old_resources"] = retainedResources(old, active: view)
            result["resident_after_switch_bytes"] = try residentBytes()
        } else if mode == "switching" {
            guard document.reading, let blocks = view.readingCoordinator?.model?.blocks else { throw CocoaError(.coderValueNotFound) }
            var seed: UInt64 = 0x106, correct = 0, visible = 0
            var failures: [[String: Any]] = []
            var old: [WeakView] = []
            for _ in 0..<200 {
                seed = seed &* 6364136223846793005 &+ 1
                let block = blocks[Int(seed >> 32) % blocks.count]
                let source = Int(block.sourceStart + block.sourceLength / 2)
                guard let coordinator = view.readingCoordinator else { throw CocoaError(.coderValueNotFound) }
                coordinator.reveal(source: source)
                let display = coordinator.displayPosition(source: source)
                view.setSelectedRange(NSRange(location: display, length: min(1, max(0, view.sourceUTF16Length - display))))
                old.append(WeakView(view)); await workspace.toggleCurrentView()
                view = try await settled(false, document: document)
                let caret = view.selectedRange().location
                if caret >= Int(block.sourceStart), caret < Int(block.sourceStart + block.sourceLength) { correct += 1 }
                view.setSelectedRange(NSRange(location: source, length: 0))
                old.append(WeakView(view)); await workspace.toggleCurrentView()
                view = try await settled(true, document: document)
                if let coordinator = view.readingCoordinator {
                    let rect = coordinator.rect(at: coordinator.displayPosition(source: source))
                    if view.visibleRect.intersects(rect) { visible += 1 }
                    else { failures.append(["source": source, "anchor": document.sourceAnchor,
                        "selected": view.selectedRange().location,
                        "selected_source": AnchorInspection.sourceValue(coordinator.sourcePosition(at: view.selectedRange().location)),
                        "rect": NSStringFromRect(rect), "visible": NSStringFromRect(view.visibleRect),
                        "frame": NSStringFromRect(view.frame), "bounds": NSStringFromRect(view.bounds),
                        "scroll_content": NSStringFromRect(view.enclosingScrollView?.contentView.bounds ?? .zero)]) }
                }
            }
            result["round_trips"] = 200; result["correct_blocks"] = correct; result["visible_blocks"] = visible
            // 与既有图视图验收一致，等原生滚动和 SwiftUI 拆卸的延迟引用释放。
            try await Task.sleep(for: .milliseconds(200))
            result["retained_old_views"] = old.filter { $0.value != nil && $0.value !== view }.count
            result["retained_old_resources"] = retainedResources(old, active: view)
            result["failures"] = failures
            result["retained_details"] = old.compactMap { reference -> [String: Any]? in
                guard let old = reference.value, old !== view else { return nil }
                return ["reading": old.isReading, "length": old.string.utf16.count, "storage_length": old.textStorage?.length ?? -1,
                        "window": old.window != nil, "diagram_views": old.diagramViews.count,
                        "reading_model_length": reference.readingCoordinator?.model?.text.utf16.count ?? 0,
                        "reading_model_blocks": reference.readingCoordinator?.model?.blocks.count ?? 0,
                        "source_runs": reference.sourceStyling?.runs.count ?? 0,
                        "editing_blocks": reference.editingCoordinator?.retainedBlockCount ?? 0,
                        "editing_document_retained": reference.editingCoordinator?.document != nil] as [String: Any]
            }
        } else if mode == "external" {
            guard let coordinator = view.readingCoordinator else { throw CocoaError(.coderValueNotFound) }
            coordinator.reveal(source: source.utf16.count / 2); view.layoutSubtreeIfNeeded()
            guard let before = coordinator.visibleAnchor() else { throw CocoaError(.coderValueNotFound) }
            let middle = (source as NSString).range(of: "Paragraph 8000:")
            guard middle.location != NSNotFound else { throw CocoaError(.coderValueNotFound) }
            let changed = (source as NSString).replacingCharacters(in: middle, with: "Modified paragraph 8000:")
            let began = ProcessInfo.processInfo.systemUptime
            try Data(changed.utf8).write(to: document.io.url)
            for _ in 0..<10000 {
                if document.text == changed, coordinator.model?.revision == document.revision, !coordinator.pending { break }
                try await Task.sleep(for: .milliseconds(1))
            }
            guard document.text == changed, let after = coordinator.visibleAnchor() else { throw CocoaError(.coderValueNotFound) }
            result["watch_to_display_ms"] = (coordinator.displayedAt - began) * 1000
            result["after_debounce_ms"] = (coordinator.displayedAt - document.refreshStartedAt) * 1000
            result["rebuilds"] = coordinator.model?.rebuilt ?? 0
            result["changed_characters"] = coordinator.changedCharacters
            result["same_anchor"] = before.id == after.id
            result["anchor_shift_pt"] = abs(before.offset - after.offset)
            result["anchor_before"] = before.offset
            result["anchor_after"] = after.offset
            result["refresh_io_ms"] = document.refreshIOMilliseconds
            result["external_accept_ms"] = document.externalAcceptMilliseconds
            result["refresh_to_display_ms"] = coordinator.latencyMilliseconds
            result["reading_phases"] = coordinator.phaseMilliseconds
        }
        return result
    }

    /// 快速连续 ⌘E：真实 HID 事件之间不加等待，立即读取阅读视口并核对锚点没有落到 EOF。
    private static func rapidToggleAnchor(document: NoteDocument, view: DiagramTextView, source: String,
                                          path: String) async throws -> [String: Any] {
        guard document.reading, view.isReading, view.blocks.count == 2 else { throw CocoaError(.coderValueNotFound) }
        let width = view.layoutContentWidth
        guard width > 180 else { throw CocoaError(.coderValueNotFound) }
        let access = CGPreflightPostEventAccess()
        guard access else {
            throw NSError(domain: "ReadingInspection", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "真实 ⌘E 检查需要 Accessibility 键盘事件权限"])
        }
        let activated = await activateInspectionWindow(view.window)
        guard activated else {
            throw NSError(domain: "ReadingInspection", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: "窗口未取得键盘焦点，无法发送真实 ⌘E"])
        }
        let initialRevision = document.revision
        var rounds: [[String: Any]] = []
        var reading = view

        AnchorInspection.beginPhase("top")
        try await scrollToTop(reading)
        reading = try await settled(true, document: document)
        rounds.append(try await rapidRound(document: document, view: reading, phase: "top"))
        reading = try await settled(true, document: document)

        AnchorInspection.beginPhase("middle")
        try await revealMiddle(reading)
        reading = try await settled(true, document: document)
        rounds.append(try await rapidRound(document: document, view: reading, phase: "middle"))
        reading = try await settled(true, document: document)

        AnchorInspection.beginPhase("bottom")
        try await scrollToBottom(reading)
        reading = try await settled(true, document: document)
        rounds.append(try await rapidRound(document: document, view: reading, phase: "bottom"))
        reading = try await settled(true, document: document)

        AnchorInspection.beginPhase("tail")
        try await scrollToTop(reading)
        reading = try await settled(true, document: document)
        try await revealDiagram(reading, ordinal: 2)
        reading = try await settled(true, document: document)
        rounds.append(try await rapidRound(document: document, view: reading, phase: "tail"))
        reading = try await settled(true, document: document)

        if let window = reading.window {
            let original = window.frame
            let originalWidth = reading.layoutContentWidth
            var wider = original
            wider.size.width += 180
            window.setFrame(wider, display: true)
            reading = try await waitForContentWidth(document: document, above: originalWidth + 80)
            try await Task.sleep(for: .milliseconds(200))
            reading = try await settled(true, document: document)
            AnchorInspection.beginPhase("widened")
            try await revealMiddle(reading)
            reading = try await settled(true, document: document)
            rounds.append(try await rapidRound(document: document, view: reading, phase: "widened"))
            reading = try await settled(true, document: document)
            let widenedWidth = reading.layoutContentWidth
            window.setFrame(original, display: true)
            reading = try await waitForContentWidth(document: document, below: widenedWidth - 80)
            try await Task.sleep(for: .milliseconds(200))
            reading = try await settled(true, document: document)
        }

        // 上一轮 reveal 还在排队时再次快速往返；最终仍应回到阅读且位置不变。
        AnchorInspection.beginPhase("repeat")
        try await scrollToTop(reading)
        reading = try await settled(true, document: document)
        let repeatBefore = rapidSnapshot(reading, document: document, phase: "repeat_before")
        let repeatStart = AnchorInspection.completedToggles
        let repeatKeys = try await pressCommandERapidly(sequences: 4)
        let repeatDelivered = await waitForToggles(repeatStart + 4)
        let repeatImmediate = await immediateReadingSnapshot(document: document, phase: "repeat_immediate")
        reading = try await settled(true, document: document)
        let repeatFinal = rapidSnapshot(reading, document: document, phase: "repeat_final")

        let disk = try await document.io.perform { try $0.snapshot().text }
        let sourcePreserved = disk == source && document.text == source && document.revision == initialRevision
        let modelNotReady = AnchorInspection.records.filter { event in
            if event["event"] as? String == "toggle_reading_unavailable" { return true }
            guard event["event"] as? String == "toggle_reading_anchor" else { return false }
            return (event["ranges"] as? Int ?? 0) == 0 || (event["model_utf16"] as? Int ?? -1) < 0
        }.count
        return [
            "mode": "rapid-toggle-anchor",
            "model_not_ready_toggles": modelNotReady,
            "keyboard_access": access,
            "activated": activated,
            "event_tap": "cghidEventTap",
            "real_keyboard": true,
            "rapid_sleep_ms": 0,
            "source_preserved": sourcePreserved,
            "disk_matches_source": disk == source,
            "revision": initialRevision,
            "rounds": rounds,
            "repeat": [
                "before": repeatBefore,
                "keys": repeatKeys,
                "delivered_toggles": repeatDelivered,
                "immediate": repeatImmediate,
                "final": repeatFinal,
            ],
            "events": AnchorInspection.records,
        ]
    }

    private static func rapidRound(document: NoteDocument, view: DiagramTextView, phase: String) async throws -> [String: Any] {
        let before = rapidSnapshot(view, document: document, phase: "\(phase)_before")
        let start = AnchorInspection.completedToggles
        let keys = try await pressCommandERapidly(sequences: 2)
        let delivered = await waitForToggles(start + 2)
        let immediate = await immediateReadingSnapshot(document: document, phase: "\(phase)_immediate")
        let final = try await settled(true, document: document)
        return [
            "phase": phase,
            "before": before,
            "keys": keys,
            "delivered_toggles": delivered,
            "immediate": immediate,
            "final": rapidSnapshot(final, document: document, phase: "\(phase)_final"),
        ]
    }

    /// 两组完整 ⌘E 连续投递，中间和组内都不 sleep。
    private static func pressCommandERapidly(sequences: Int) async throws -> [[String: Any]] {
        let source = CGEventSource(stateID: .hidSystemState)
        var events: [[String: Any]] = []
        for sequence in 1...sequences {
            for down in [true, false] {
                guard let event = CGEvent(keyboardEventSource: source, virtualKey: 14, keyDown: down) else {
                    throw CocoaError(.coderInvalidValue)
                }
                event.flags = .maskCommand
                let postedAt = ProcessInfo.processInfo.systemUptime
                event.post(tap: .cghidEventTap)
                events.append(["sequence": sequence, "down": down, "tap": "cghidEventTap",
                               "posted_at_ms": postedAt * 1000])
            }
        }
        return events
    }

    /// 只等待输入事件被处理，不等页面渲染稳定。
    private static func waitForToggles(_ target: Int) async -> Int {
        for _ in 0..<2000 {
            if AnchorInspection.completedToggles >= target { break }
            try? await Task.sleep(for: .milliseconds(1))
        }
        return AnchorInspection.completedToggles
    }

    private static func immediateReadingSnapshot(document: NoteDocument, phase: String) async -> [String: Any] {
        var waited = 0.0
        let started = ProcessInfo.processInfo.systemUptime
        var fallback: DiagramTextView?
        for _ in 0..<200 {
            if let view = DiagramInspection.current(document: document), view.isReading {
                // 刚插入的空白阅读面还不能代表用户看到的位置；等模型装好，但不等 reveal/pending 结算。
                if view.readingCoordinator?.model != nil {
                    waited = (ProcessInfo.processInfo.systemUptime - started) * 1000
                    return rapidSnapshot(view, document: document, phase: phase, extra: ["immediate_wait_ms": waited])
                }
                if fallback == nil { fallback = view }
            }
            if document.reading == false, waited > 0 { break }
            try? await Task.sleep(for: .milliseconds(1))
            waited = (ProcessInfo.processInfo.systemUptime - started) * 1000
        }
        return rapidSnapshot(fallback, document: document, phase: phase, extra: ["immediate_wait_ms": waited])
    }

    private static func rapidSnapshot(_ view: DiagramTextView?, document: NoteDocument, phase: String,
                                      extra: [String: Any] = [:]) -> [String: Any] {
        var row = AnchorInspection.snapshot(view, document: document)
        row["phase"] = phase
        row["content_width"] = Double(view?.layoutContentWidth ?? 0)
        row["window_width"] = Double(view?.window?.frame.width ?? 0)
        row.merge(extra) { _, new in new }
        return row
    }

    /// 窗口改宽后，SwiftUI 和滚动容器分多轮更新宽度；等实际内容宽度生效再定位。
    private static func waitForContentWidth(document: NoteDocument, above: Double) async throws -> DiagramTextView {
        for _ in 0..<300 {
            let view = try await settled(true, document: document)
            if view.layoutContentWidth > above { return view }
            try await Task.sleep(for: .milliseconds(1))
        }
        throw CocoaError(.coderValueNotFound)
    }

    private static func waitForContentWidth(document: NoteDocument, below: Double) async throws -> DiagramTextView {
        for _ in 0..<300 {
            let view = try await settled(true, document: document)
            if view.layoutContentWidth < below { return view }
            try await Task.sleep(for: .milliseconds(1))
        }
        throw CocoaError(.coderValueNotFound)
    }

    /// 用既有 reveal 把视口定位到正文中部；只作切换前准备，往返输入仍是 HID 键盘事件。
    private static func revealMiddle(_ view: DiagramTextView) async throws {
        guard let coordinator = view.readingCoordinator, let model = coordinator.model, !model.blocks.isEmpty else {
            throw CocoaError(.coderValueNotFound)
        }
        let sourceLength = model.blocks.map { Int($0.sourceStart + $0.sourceLength) }.max() ?? 0
        let middle = Double(sourceLength) / 2
        let block = model.blocks.filter { !$0.diagram && $0.sourceLength > 0 }.min {
            abs(Double($0.sourceStart + $0.sourceLength / 2) - middle)
                < abs(Double($1.sourceStart + $1.sourceLength / 2) - middle)
        } ?? model.blocks[model.blocks.count / 2]
        coordinator.reveal(source: Int(block.sourceStart + block.sourceLength / 2))
        view.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(20))
        view.layoutSubtreeIfNeeded()
    }

    private static func scrollDocument(_ view: DiagramTextView, fraction: Double) async throws {
        guard let scroll = view.enclosingScrollView, let manager = view.textLayoutManager else {
            throw CocoaError(.coderValueNotFound)
        }
        view.layoutSubtreeIfNeeded()
        let content = max(view.frame.height,
                          manager.usageBoundsForTextContainer.maxY + view.textContainerInset.height * 2)
        let maxY = max(0, content - scroll.contentView.bounds.height)
        scroll.contentView.scroll(to: CGPoint(x: 0, y: maxY * fraction))
        scroll.reflectScrolledClipView(scroll.contentView)
        view.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(20))
        view.layoutSubtreeIfNeeded()
    }

    private static func scrollToBottom(_ view: DiagramTextView) async throws {
        try await scrollDocument(view, fraction: 1)
    }

    /// 真实 ⌘E 往返后的双图间距回归：只通过 CGEvent 键盘事件切换，直接比较图 frame。
    private static func diagramToggleSpacing(document: NoteDocument, view: DiagramTextView, source: String,
                                             path: String) async throws -> [String: Any] {
        guard document.reading, view.isReading else { throw CocoaError(.coderValueNotFound) }
        guard view.blocks.count == 2 else { throw CocoaError(.coderValueNotFound) }
        let width = view.layoutContentWidth
        guard width > 180 else { throw CocoaError(.coderValueNotFound) }
        let reference = try await referenceScenes(view, source: source, path: path, width: width)
        let access = CGPreflightPostEventAccess()
        guard access else {
            throw NSError(domain: "ReadingInspection", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "真实 ⌘E 检查需要 Accessibility 键盘事件权限"])
        }
        let activated = await activateInspectionWindow(view.window)
        guard activated else {
            throw NSError(domain: "ReadingInspection", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: "窗口未取得键盘焦点，无法发送真实 ⌘E"])
        }
        let firstView = String(describing: ObjectIdentifier(view))
        let initialTop = spacingSnapshot(view, document: document, phase: "initial_top")
        try await revealDiagram(view, ordinal: 2)
        let initialSecond = spacingSnapshot(view, document: document, phase: "initial_second")
        try await scrollToTop(view)
        let initialReturn = spacingSnapshot(view, document: document, phase: "initial_return")
        let initialRevision = document.revision

        let enterKey = try await pressCommandE()
        let editing = try await settled(false, document: document)
        guard !editing.isReading, !document.reading else { throw CocoaError(.coderValueNotFound) }
        let editingState = diagramState(editing, phase: "editing")
        let leaveKey = try await pressCommandE()
        var reading = try await settled(true, document: document)
        guard reading.isReading, document.reading else { throw CocoaError(.coderValueNotFound) }
        let finalTop = spacingSnapshot(reading, document: document, phase: "final_top")
        try await scrollToTop(reading)
        let finalTopVisible = spacingSnapshot(reading, document: document, phase: "final_top_visible")
        try await revealDiagram(reading, ordinal: 2)
        let finalSecond = spacingSnapshot(reading, document: document, phase: "final_second")
        try await scrollToTop(reading)
        try await revealDiagram(reading, ordinal: 2)
        let scrollReturn = spacingSnapshot(reading, document: document, phase: "scroll_return")
        try await scrollToTop(reading)
        try await revealDiagram(reading, ordinal: 2)
        let scrollReturnAgain = spacingSnapshot(reading, document: document, phase: "scroll_return_again")

        var resized: [String: Any] = [:]
        var restored: [String: Any] = [:]
        if let window = reading.window {
            let original = window.frame
            var wider = original
            wider.size.width += 180
            window.setFrame(wider, display: true)
            try await Task.sleep(for: .milliseconds(120))
            reading = try await settled(true, document: document)
            try await revealDiagram(reading, ordinal: 2)
            resized = spacingSnapshot(reading, document: document, phase: "resized")
            window.setFrame(original, display: true)
            try await Task.sleep(for: .milliseconds(120))
            reading = try await settled(true, document: document)
            try await revealDiagram(reading, ordinal: 2)
            restored = spacingSnapshot(reading, document: document, phase: "restored")
        }

        let disk = try await document.io.perform { try $0.snapshot().text }
        let sourcePreserved = disk == source && document.text == source && document.revision == initialRevision
        return [
            "mode": "diagram-toggle-spacing",
            "keyboard_access": access,
            "activated": activated,
            "event_tap": "cghidEventTap",
            "key_events": [enterKey, leaveKey],
            "real_keyboard": true,
            "first_view": firstView,
            "final_view": String(describing: ObjectIdentifier(reading)),
            "revision": initialRevision,
            "source_preserved": sourcePreserved,
            "disk_matches_source": disk == source,
            "reference": reference,
            "editing": editingState,
            "initial_top": initialTop,
            "initial_second": initialSecond,
            "initial_return": initialReturn,
            "final_top": finalTop,
            "final_top_visible": finalTopVisible,
            "final_second": finalSecond,
            "scroll_return": scrollReturn,
            "scroll_return_again": scrollReturnAgain,
            "resized": resized,
            "restored": restored,
        ]
    }

    /// 真实窄宽度回归：用真实面板宽度把正文压到指定的 layoutContentWidth，
    /// 重新打开笔记后检查首次渲染、⌘E 往返、滚动和窗口改宽。失败时返回测量值，不提前抛错。
    private static func diagramNarrow(workspace: Workspace, layout: Layout?, document: NoteDocument,
                                      view: DiagramTextView, source: String, path: String,
                                      arguments: [String]) async throws -> [String: Any] {
        guard document.reading, view.isReading, let window = view.window else { throw CocoaError(.coderValueNotFound) }
        guard let layout else { throw CocoaError(.coderValueNotFound) }
        guard let targetArgument = arguments.first(where: { $0.hasPrefix("--narrow-width=") }),
              let target = Double(targetArgument.dropFirst("--narrow-width=".count)),
              [120.0, 150.0].contains(target) else { throw CocoaError(.coderInvalidValue) }
        let access = CGPreflightPostEventAccess()
        guard access else {
            throw NSError(domain: "ReadingInspection", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "真实窄宽度检查需要 Accessibility 键盘事件权限"])
        }
        let activated = await activateInspectionWindow(window)
        guard activated else {
            throw NSError(domain: "ReadingInspection", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: "窗口未取得键盘焦点，无法发送真实 ⌘E"])
        }
        let originalLeft = layout.size[.left]
        let originalRightVisible = layout.visible[.right]
        let originalFrame = window.frame
        defer {
            layout.size[.left] = originalLeft
            layout.visible[.right] = originalRightVisible
            window.setFrame(originalFrame, display: true)
        }
        let layoutState = try await applyNarrowLayout(layout: layout, document: document, target: target)
        let alt = arguments.first { $0.hasPrefix("--narrow-alt=") }
            .map { String($0.dropFirst("--narrow-alt=".count)) } ?? "欢迎.md"

        // 先在窄布局下离开笔记再回来，得到真正首次挂载在该宽度上的阅读表面。
        workspace.openNote(alt)
        for _ in 0..<400 {
            if workspace.selected == alt, workspace.current?.document != nil { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        workspace.openNote(path)
        let firstOpen = await waitForNarrow(document: document, reading: true, templates: 2, timeout: 5)
        guard let firstView = firstOpen else { throw CocoaError(.coderValueNotFound) }
        let first = narrowSnapshot(firstView, document: document, phase: "first_open", target: target,
                                   widthState: layoutState)

        let enterKey = try await pressCommandE()
        let editing = await waitForNarrow(document: document, reading: false, templates: nil, timeout: 5)
        guard let editingView = editing else { throw CocoaError(.coderValueNotFound) }
        let editingState = narrowSnapshot(editingView, document: document, phase: "toggle_editing", target: target,
                                          widthState: layoutState)
        let leaveKey = try await pressCommandE()
        let returned = await waitForNarrow(document: document, reading: true, templates: 2, timeout: 5)
        guard let returnView = returned else { throw CocoaError(.coderValueNotFound) }
        let returnedState = narrowSnapshot(returnView, document: document, phase: "toggle_return", target: target,
                                           widthState: layoutState)

        if returnView.blocks.contains(where: { $0.ordinal == 2 }) {
            try await revealDiagram(returnView, ordinal: 2)
        }
        let revealed = narrowSnapshot(returnView, document: document, phase: "scroll_reveal", target: target,
                                      widthState: layoutState)
        try await scrollToTop(returnView)

        window.setFrame(NSRect(x: originalFrame.minX, y: originalFrame.minY,
                               width: originalFrame.width + 180, height: originalFrame.height), display: true)
        let widened = await waitForNarrow(document: document, reading: true, templates: 2, timeout: 5)
        guard let widenedView = widened else { throw CocoaError(.coderValueNotFound) }
        let widenedState = narrowSnapshot(widenedView, document: document, phase: "widened", target: target,
                                          widthState: layoutState)
        window.setFrame(originalFrame, display: true)
        let restored = await waitForNarrow(document: document, reading: true, templates: 2, timeout: 5)
        guard let restoredView = restored else { throw CocoaError(.coderValueNotFound) }
        let restoredState = narrowSnapshot(restoredView, document: document, phase: "restored", target: target,
                                           widthState: layoutState)

        let disk = try await document.io.perform { try $0.snapshot().text }
        let firstViewID = String(describing: ObjectIdentifier(firstView))
        return [
            "mode": "diagram-narrow",
            "target_width": target,
            "keyboard_access": access,
            "activated": activated,
            "event_tap": "cghidEventTap",
            "key_events": [enterKey, leaveKey],
            "real_keyboard": true,
            "layout": layoutState,
            "first_view": firstViewID,
            "final_view": String(describing: ObjectIdentifier(restoredView)),
            "view_rebuilt": firstViewID != String(describing: ObjectIdentifier(restoredView)),
            "source_preserved": disk == source && document.text == source,
            "disk_matches_source": disk == source,
            "phases": [first, editingState, returnedState, revealed, widenedState, restoredState],
        ]
    }

    private static func applyNarrowLayout(layout: Layout, document: NoteDocument,
                                          target: Double) async throws -> [String: Any] {
        guard let window = DiagramInspection.current(document: document)?.window else { throw CocoaError(.coderValueNotFound) }
        layout.visible[.left] = true
        layout.visible[.right] = true
        let contentWidth = window.contentView?.bounds.width ?? window.frame.width
        let desiredScroll = CGFloat(target < 135 ? 140 : target + 60)
        let right = layout.size[.right] ?? 250
        layout.size[.left] = max(0, contentWidth - right - 32 - desiredScroll)
        var measured = 0.0
        var scrollWidth = 0.0
        for _ in 0..<60 {
            try await Task.sleep(for: .milliseconds(50))
            guard let view = DiagramInspection.current(document: document) else { continue }
            measured = Double(view.layoutContentWidth)
            let currentScroll = view.enclosingScrollView?.bounds.width ?? 0
            scrollWidth = Double(currentScroll)
            if abs(measured - target) <= 0.5 { break }
            // layoutContentWidth 与面板宽度反向同步；滚动条占位变化由下一步实测修正。
            layout.size[.left] = max(0, (layout.size[.left] ?? 0) + CGFloat(measured - target))
        }
        return ["target": target, "measured": measured, "scroll_width": scrollWidth,
                "left": Double(layout.size[.left] ?? 0), "right": Double(right),
                "window_content_width": Double(contentWidth)]
    }

    /// 有界等待：基线被门控拦住时返回最后的视图，由快照记录未渲染状态。
    private static func waitForNarrow(document: NoteDocument, reading: Bool, templates: Int?,
                                      timeout: Double) async -> DiagramTextView? {
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        var last: DiagramTextView?
        while ProcessInfo.processInfo.systemUptime < deadline {
            if let view = DiagramInspection.current(document: document), view.isReading == reading {
                last = view
                let rendered = view.blocks.filter { $0.prepared != nil }
                let complete = !view.diagramPending && view.readingCoordinator?.pending == false
                    && view.readingCoordinator?.model?.revision == document.revision
                    && (templates.map { view.blocks.count == $0 && rendered.count == $0 } ?? true)
                    && view.diagramViews.count >= rendered.count
                if complete { return view }
            }
            try? await Task.sleep(for: .milliseconds(5))
        }
        return last
    }

    private static func narrowSnapshot(_ view: DiagramTextView, document: NoteDocument, phase: String,
                                       target: Double, widthState: [String: Any]) -> [String: Any] {
        view.layoutSubtreeIfNeeded()
        let rows = view.blocks.sorted { $0.ordinal < $1.ordinal }.map { block -> [String: Any] in
            let diagram = view.diagramViews[block.ordinal]
            let frame = diagram?.frame ?? .zero
            let visible = view.visibleRect.intersection(frame)
            return [
                "ordinal": Int(block.ordinal),
                "prepared": block.prepared != nil,
                "scene_width": block.prepared.map { Double($0.scene.width) } ?? 0,
                "scene_height": block.prepared.map { Double($0.scene.height) } ?? 0,
                "nodes": block.prepared?.scene.hitRegions.count ?? 0,
                "primitives": block.prepared?.scene.primitives.count ?? 0,
                "frame": ["x": Double(frame.minX), "y": Double(frame.minY),
                          "width": Double(frame.width), "height": Double(frame.height)],
                "visible": !(diagram?.isHidden ?? true) && !visible.isNull && visible.height > 0.5,
            ]
        }
        return [
            "phase": phase,
            "target_width": target,
            "content_width": Double(view.layoutContentWidth),
            "scroll_width": Double(view.enclosingScrollView?.bounds.width ?? 0),
            "window_width": Double(view.window?.frame.width ?? 0),
            "document_height": Double(view.frame.height),
            "text_utf16": view.textStorage?.length ?? 0,
            "model_revision": (view.readingCoordinator?.model).map { Double($0.revision) } ?? -1,
            "document_revision": Double(document.revision),
            "pending": view.diagramPending || view.readingCoordinator?.pending == true,
            "reading": view.isReading,
            "blocks": view.blocks.count,
            "diagrams": view.diagramViews.count,
            "rows": rows,
            "layout": widthState,
        ]
    }

    private static func referenceScenes(_ view: DiagramTextView, source: String, path: String,
                                        width: Double) async throws -> [[String: Any]] {
        let blocks = view.blocks.sorted { $0.ordinal < $1.ordinal }
        let dark = view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let mmd = path.lowercased().hasSuffix(".mmd")
        let scenes = try await Task.detached {
            let engine = try DiagramEngine()
            return try blocks.map { block -> ReferenceScene in
                let identity = DiagramIdentity(document: block.documentIdentity, block: block.ordinal,
                                               revision: block.revision, dark: dark, fontGeneration: 0,
                                               requestGeneration: 0)
                let wire = try engine.renderRequestInspection(source: source, mmd: mmd, width: width,
                                                              collapsed: Array(block.collapsed), identity: identity,
                                                              previous: nil, viewport: [])
                let scene = try DiagramScene(wire)
                return ReferenceScene(ordinal: Int(block.ordinal), width: Double(scene.width),
                                      height: Double(scene.height), signature: sceneSignature(scene))
            }
        }.value
        return scenes.map { ["ordinal": $0.ordinal, "width": $0.width,
                             "height": $0.height, "signature": $0.signature] }
    }

    /// 与 DiagramInspection 相同的场景指纹；引用渲染在后台完成，不能调用主线程方法。
    private nonisolated static func sceneSignature(_ scene: DiagramScene) -> String {
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

    private static func diagramState(_ view: DiagramTextView, phase: String) -> [String: Any] {
        [
            "phase": phase,
            "reading": view.isReading,
            "content_width": Double(view.layoutContentWidth),
            "window_width": Double(view.window?.frame.width ?? 0),
            "document_height": Double(view.frame.height),
            "blocks": view.blocks.sorted { $0.ordinal < $1.ordinal }.map { block in
                [
                    "ordinal": Int(block.ordinal),
                    "source_start": Int(block.sourceStart),
                    "range": [block.range.location, block.range.length],
                    "tail_start": block.tailStart,
                    "collapsed": block.collapsed.count,
                    "scene_width": block.prepared.map { Double($0.scene.width) } ?? NSNull(),
                    "scene_height": block.prepared.map { Double($0.scene.height) } ?? NSNull(),
                ] as [String: Any]
            },
        ]
    }

    private static func activateInspectionWindow(_ window: NSWindow?) async -> Bool {
        guard let window else { return false }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        for _ in 0..<160 {
            if NSApp.isActive && window.isKeyWindow { return true }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return NSApp.isActive && window.isKeyWindow
    }

    /// 与既有编辑器真实按键检查相同：HID 事件走菜单快捷键，不调用 toggleView。
    private static func pressCommandE() async throws -> [String: Any] {
        let source = CGEventSource(stateID: .hidSystemState)
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: 14, keyDown: down) else {
                throw CocoaError(.coderInvalidValue)
            }
            event.flags = .maskCommand
            event.post(tap: .cghidEventTap)
            try await Task.sleep(for: .milliseconds(down ? 18 : 55))
        }
        return ["keycode": 14, "modifiers": ["command"], "tap": "cghidEventTap"]
    }

    private static func revealDiagram(_ view: DiagramTextView, ordinal: UInt32) async throws {
        guard let block = view.blocks.first(where: { $0.ordinal == ordinal }) else { throw CocoaError(.coderValueNotFound) }
        view.readingCoordinator?.reveal(source: Int(block.sourceStart))
        view.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(20))
        view.layoutSubtreeIfNeeded()
    }

    private static func scrollToTop(_ view: DiagramTextView) async throws {
        guard let scroll = view.enclosingScrollView else { throw CocoaError(.coderValueNotFound) }
        scroll.contentView.scroll(to: CGPoint(x: 0, y: 0))
        scroll.reflectScrolledClipView(scroll.contentView)
        view.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(20))
        view.layoutSubtreeIfNeeded()
    }

    /// 间距只由图 frame 计算；截图的可见性只作补充。
    private static func spacingSnapshot(_ view: DiagramTextView, document: NoteDocument, phase: String) -> [String: Any] {
        view.layoutSubtreeIfNeeded()
        let rows = view.blocks.sorted { $0.ordinal < $1.ordinal }.compactMap { block -> [String: Any]? in
            guard let diagram = view.diagramViews[block.ordinal] else { return nil }
            let frame = diagram.frame
            let visible = view.visibleRect.intersection(frame)
            let scene = diagram.prepared.scene
            return [
                "ordinal": Int(block.ordinal),
                "source_start": Int(block.sourceStart),
                "range": [block.range.location, block.range.length],
                "tail_start": block.tailStart,
                "revision": block.revision,
                "frame": ["x": Double(frame.minX), "y": Double(frame.minY),
                          "width": Double(frame.width), "height": Double(frame.height)],
                "frame_min_y": Double(frame.minY),
                "frame_max_y": Double(frame.maxY),
                "hidden": diagram.isHidden,
                "visible": !diagram.isHidden && !visible.isNull && visible.height > 0.5,
                "visible_height": visible.isNull ? 0 : Double(visible.height),
                "scene_width": Double(scene.width),
                "scene_height": Double(scene.height),
                "scene_signature": DiagramInspection.sceneSignature(scene),
                "expanded": view.expanded.contains(block.ordinal),
            ]
        }
        var spacing: Any = NSNull()
        if rows.count >= 2,
           let first = rows[0]["frame_max_y"] as? Double,
           let second = rows[1]["frame_min_y"] as? Double {
            spacing = second - first
        }
        var fragments: [[String: Any]] = []
        if let manager = view.textLayoutManager, let content = manager.textContentManager {
            let range = manager.textViewportLayoutController.viewportRange
            let start = range?.location ?? content.documentRange.location
            let end = range?.endLocation ?? content.documentRange.endLocation
            manager.enumerateTextLayoutFragments(from: start, options: [.ensuresLayout]) { fragment in
                if fragment.rangeInElement.location.compare(end) != .orderedAscending { return false }
                let offset = content.offset(from: content.documentRange.location, to: fragment.rangeInElement.location)
                var row: [String: Any] = ["offset": offset,
                                          "min_y": Double(fragment.layoutFragmentFrame.minY),
                                          "height": Double(fragment.layoutFragmentFrame.height)]
                if let note = fragment as? NoteLayoutFragment {
                    row["preview_height"] = Double(note.previewHeight)
                    if let replacement = note.replacement {
                        row["replacement"] = ["width": Double(replacement.width), "height": Double(replacement.height)]
                    }
                }
                fragments.append(row)
                return fragments.count < 200
            }
        }
        return [
            "phase": phase,
            "reading": view.isReading,
            "view_identity": String(describing: ObjectIdentifier(view)),
            "document_height": Double(view.frame.height),
            "bounds_height": Double(view.bounds.height),
            "usage_height": Double(view.textLayoutManager?.usageBoundsForTextContainer.maxY ?? 0),
            "visible_rect": ["x": Double(view.visibleRect.minX), "y": Double(view.visibleRect.minY),
                             "width": Double(view.visibleRect.width), "height": Double(view.visibleRect.height)],
            "content_width": Double(view.layoutContentWidth),
            "window_width": Double(view.window?.frame.width ?? 0),
            "diagrams": rows,
            "spacing": spacing,
            "source_utf16": document.text.utf16.count,
            "revision": document.revision,
            "fragments": fragments,
        ]
    }

    private static func residentBytes() throws -> UInt64 {
        var info = proc_taskinfo()
        let size = Int32(MemoryLayout.size(ofValue: info))
        guard proc_pidinfo(getpid(), PROC_PIDTASKINFO, 0, &info, size) == size else { throw CocoaError(.fileReadUnknown) }
        return info.pti_resident_size
    }
}
