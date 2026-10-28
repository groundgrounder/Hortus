import AppKit
import Darwin

/// 显式检查参数才采样；正常阅读不安装计时器。
@MainActor
enum MathInspection {
    @MainActor
    private final class WeakView {
        weak var value: DiagramTextView?
        init(_ value: DiagramTextView) { self.value = value }
    }
    private static func residentBytes() throws -> UInt64 {
        var info = proc_taskinfo()
        let size = Int32(MemoryLayout.size(ofValue: info))
        guard proc_pidinfo(getpid(), PROC_PIDTASKINFO, 0, &info, size) == size else { throw CocoaError(.fileReadUnknown) }
        return info.pti_resident_size
    }
    private static func footprintBytes() throws -> UInt64 {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout.size(ofValue: info) / MemoryLayout<integer_t>.size)
        let status = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard status == KERN_SUCCESS else { throw CocoaError(.fileReadUnknown) }
        return info.phys_footprint
    }
    private static func reopen(_ path: String, workspace: Workspace) async throws -> WeakView {
        workspace.closeTab(path)
        for _ in 0..<10000 {
            if !workspace.tabs.contains(where: { $0.path == path }) { break }
            try await Task.sleep(for: .milliseconds(1))
        }
        // 等关闭事务完成原生视图拆卸，再模拟下一次打开。
        try await Task.sleep(for: .milliseconds(200))
        workspace.openNote(path)
        for _ in 0..<10000 {
            if workspace.current?.document != nil { break }
            try await Task.sleep(for: .milliseconds(1))
        }
        guard let current = workspace.current?.document else { throw CocoaError(.coderValueNotFound) }
        return WeakView(try await ReadingInspection.settled(true, document: current))
    }
    static func run(workspace: Workspace, document: NoteDocument, view initial: DiagramTextView,
                    source: String, mode: String, arguments: [String], initial result: [String: Any]) async throws -> [String: Any] {
        var result = result
        let view = initial
        guard let coordinator = view.readingCoordinator else { throw CocoaError(.coderValueNotFound) }
        result["math_blocks"] = view.mathViews.count
        result["math_errors"] = view.mathViews.compactMap { $0.item.prepared.reason }
        result["glyphs"] = view.mathViews.reduce(0) { $0 + ($1.item.prepared.math?.glyphCount ?? 0) }
        result["maximum_metric_error_px"] = (view.mathViews.compactMap { $0.item.prepared.math?.maximumMetricError }.max() ?? 0)
            * (view.window?.backingScaleFactor ?? 2)
        result["main_thread_preparations"] = await MathService.shared.mainThreadPreparations
        result["font_registrations"] = await MathService.shared.registrationCount
        result["fallback_measurements"] = await MathService.shared.measurementCount
        result["main_thread_phases_ms"] = coordinator.phaseMilliseconds.filter { ["install_ms", "layout_ms", "display_ms", "main_block_ms"].contains($0.key) }
        if let arg = arguments.first(where: { $0.hasPrefix("--math-index=") }),
           let index = Int(arg.dropFirst("--math-index=".count)), view.mathViews.indices.contains(index) {
            let math = view.mathViews[index]
            coordinator.reveal(source: Int(math.item.prepared.block.sourceStart))
            try await Task.sleep(for: .milliseconds(50))
            view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
            if arguments.contains("--math-scroll-end"), let canvas = math.scroll.documentView {
                math.scroll.contentView.scroll(to: CGPoint(x: max(0, canvas.bounds.width - math.scroll.contentSize.width), y: 0))
                math.scroll.reflectScrolledClipView(math.scroll.contentView)
                math.displayIfNeeded()
            }
            result["formula_frame"] = NSStringFromRect(math.frame)
            result["horizontal_scroll"] = math.scroll.hasHorizontalScroller
            result["formula_latex"] = math.item.prepared.block.latex
            if let capture = arguments.first(where: { $0.hasPrefix("--math-capture=") }) {
                let png = try math.capture()
                try png.write(to: URL(fileURLWithPath: String(capture.dropFirst("--math-capture=".count))))
            }
        }
        if let arg = arguments.first(where: { $0.hasPrefix("--math-reveal=") }) {
            let query = String(arg.dropFirst("--math-reveal=".count))
            let range = (source as NSString).range(of: query)
            if range.location != NSNotFound { coordinator.reveal(source: range.location); view.layoutSubtreeIfNeeded(); view.displayIfNeeded() }
        }
        if mode == "math-performance" {
            var frames: [Double] = []
            for index in 0..<240 {
                let began = ProcessInfo.processInfo.systemUptime
                let y = max(0, view.bounds.height - view.visibleRect.height) * Double(index < 120 ? index : 239-index) / 119
                view.enclosingScrollView?.contentView.scroll(to: CGPoint(x: 0, y: y))
                view.enclosingScrollView?.reflectScrolledClipView(view.enclosingScrollView!.contentView)
                view.needsLayout = true; view.layoutSubtreeIfNeeded(); view.window?.displayIfNeeded()
                frames.append((ProcessInfo.processInfo.systemUptime - began) * 1000)
                try await Task.sleep(for: .milliseconds(1))
            }
            frames.sort(); result["scroll_p95_ms"] = frames[Int(Double(frames.count - 1) * 0.95)]
            result["scroll_max_ms"] = frames.last
            result["maximum_main_work_ms"] = view.maximumMainWorkMilliseconds
            result["maximum_formula_draw_ms"] = view.mathViews.map(\.maximumDrawMilliseconds).max()
        } else if mode == "math-external" {
            guard let formula = view.mathViews.dropFirst(view.mathViews.count / 2).first else { throw CocoaError(.coderValueNotFound) }
            coordinator.reveal(source: Int(formula.item.prepared.block.sourceStart))
            try await Task.sleep(for: .milliseconds(50))
            view.layoutSubtreeIfNeeded()
            let before = try coordinator.visibleAnchor().unwrap()
            result["anchor_before"] = ["id": String(before.id), "source": String(before.source), "offset": String(before.offset)]
            let old = view.mathViews
            let changed = (source as NSString).replacingCharacters(in: NSRange(location: Int(formula.item.prepared.block.sourceStart),
                length: Int(formula.item.prepared.block.sourceLength)), with: "$$x^3+y^3=z^3$$")
            try Data(changed.utf8).write(to: document.io.url)
            for _ in 0..<10000 {
                if document.text == changed, coordinator.model?.revision == document.revision, !coordinator.pending { break }
                try await Task.sleep(for: .milliseconds(1))
            }
            guard document.text == changed, coordinator.model?.revision == document.revision, !coordinator.pending else { throw CocoaError(.coderValueNotFound) }
            let after = try coordinator.visibleAnchor().unwrap()
            result["anchor_after"] = ["id": String(after.id), "source": String(after.source), "offset": String(after.offset)]
            result["after_debounce_ms"] = (coordinator.displayedAt - document.refreshStartedAt) * 1000
            result["rebuilt"] = coordinator.model?.rebuilt
            result["reused_math_views"] = view.mathViews.filter { current in old.contains { $0 === current } }.count
            result["anchor_shift_pt"] = abs(before.offset - after.offset)
            result["same_anchor"] = before.id == after.id
        } else if mode == "math-memory" {
            var memory: [UInt64] = []
            var footprints: [UInt64] = []
            var peaks: [Int] = []
            var oldViews: [WeakView] = []
            var retained: [Int] = []
            let path = workspace.selected!
            let cycles = arguments.first { $0.hasPrefix("--math-memory-cycles=") }
                .flatMap { Int($0.dropFirst("--math-memory-cycles=".count)) } ?? 20
            for _ in 0..<max(1, min(100, cycles)) {
                // 每轮结束异步任务，避免自检持有上一轮的临时存储。
                oldViews.append(try await Task { @MainActor in try await reopen(path, workspace: workspace) }.value)
                DiagramInspection.finishEvent()
                try await Task.sleep(for: .milliseconds(100))
                retained.append(oldViews.dropLast().compactMap(\.value).count)
                memory.append(try residentBytes())
                footprints.append(try footprintBytes())
                var usage = rusage()
                getrusage(RUSAGE_SELF, &usage)
                peaks.append(usage.ru_maxrss)
            }
            result["memory_bytes"] = memory
            result["footprint_bytes"] = footprints
            result["peak_bytes"] = peaks
            result["retained_old_views"] = retained
        }
        return result
    }
}

private extension Optional {
    func unwrap() throws -> Wrapped {
        guard let value = self else { throw CocoaError(.coderValueNotFound) }
        return value
    }
}
