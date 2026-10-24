import AppKit
import Darwin
import Quartz

/// 真实窗口计量只由显式验收参数启动，日常运行不增加轮询。
@MainActor
enum ReadingInspection {
    @MainActor
    private final class WeakView {
        weak var value: DiagramTextView?
        weak var readingCoordinator: ReadingText.Coordinator?
        weak var sourceStyling: SourceStyling?
        weak var editingCoordinator: NoteText.Coordinator?
        init(_ value: DiagramTextView) {
            self.value = value; readingCoordinator = value.readingCoordinator
            sourceStyling = value.sourceStyling; editingCoordinator = value.editingCoordinator
        }
    }
    static func settled(_ reading: Bool, document: NoteDocument) async throws -> DiagramTextView {
        for _ in 0..<10000 {
            if let view = DiagramInspection.current(), view.isReading == reading,
               view.representedDocument == ObjectIdentifier(document), !view.diagramPending,
               reading ? view.readingCoordinator?.pending == false && view.readingCoordinator?.revealPending == false
                    && view.readingCoordinator?.model?.revision == document.revision
                    : view.sourceStyling?.pending == false {
                view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
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
    static func run(workspace: Workspace, arguments: [String]) async throws -> [String: Any] {
        guard let document = workspace.current?.document, let path = workspace.selected else { throw CocoaError(.coderValueNotFound) }
        let mode = arguments.first { $0.hasPrefix("--reading-check=") }.map { String($0.dropFirst("--reading-check=".count)) } ?? "visual"
        var view = try await settled(document.reading, document: document)
        let openedAt = view.isReading ? view.readingCoordinator?.displayedAt ?? 0 : view.sourceStyling?.openDisplayedAt ?? 0
        let source = try await document.io.perform { try $0.snapshot().text }
        var result: [String: Any] = ["reading": view.isReading, "textkit2": view.textLayoutManager != nil,
            "utf8_bytes": source.utf8.count, "open_ms": (openedAt - workspace.noteOpenStartedAt) * 1000,
            "resident_bytes": try residentBytes(), "display_utf16": view.sourceUTF16Length,
            "diagrams": view.diagramViews.count, "rebuilt": view.readingCoordinator?.model?.rebuilt ?? 0]
        result["reading_phases"] = view.readingCoordinator?.phaseMilliseconds ?? [:]
        result["media"] = view.mediaBlocks.count
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
                        "selected_source": coordinator.sourcePosition(at: view.selectedRange().location),
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
    private static func residentBytes() throws -> UInt64 {
        var info = proc_taskinfo()
        let size = Int32(MemoryLayout.size(ofValue: info))
        guard proc_pidinfo(getpid(), PROC_PIDTASKINFO, 0, &info, size) == size else { throw CocoaError(.fileReadUnknown) }
        return info.pti_resident_size
    }
}
