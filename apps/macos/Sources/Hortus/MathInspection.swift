import AppKit
import Darwin

/// 显式检查参数才采样；正常阅读不安装计时器。
@MainActor
enum MathInspection {
    static func pageHasHorizontalOverflow(_ view: NSView) -> Bool {
        view.bounds.width > (view.enclosingScrollView?.contentView.bounds.width ?? view.bounds.width) + 0.5
    }
    private static func selectionBackgroundDifference(_ view: DiagramTextView, _ formula: InlineMathView) throws -> Double {
        // 公开选区夹具用 \quad 留出无墨迹区域，对比公式内外整条选区的像素。
        let area = CGRect(x: formula.frame.minX - 3, y: formula.frame.minY - 2, width: 8, height: formula.frame.height + 6)
        guard let bitmap = view.bitmapImageRepForCachingDisplay(in: area) else { throw CocoaError(.coderValueNotFound) }
        view.cacheDisplay(in: area, to: bitmap)
        let scale = Double(bitmap.pixelsWide) / area.width
        var difference = 0.0
        for y in 0..<bitmap.pixelsHigh {
            guard let a = bitmap.colorAt(x: Int(scale), y: y)?.usingColorSpace(.sRGB),
                  let b = bitmap.colorAt(x: Int(5 * scale), y: y)?.usingColorSpace(.sRGB) else {
                throw CocoaError(.coderValueNotFound)
            }
            difference = max(difference, abs(a.redComponent - b.redComponent), abs(a.greenComponent - b.greenComponent), abs(a.blueComponent - b.blueComponent))
        }
        return difference
    }
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
    static func runMemory(workspace: Workspace, path: String, arguments: [String]) async throws -> [String: Any] {
        var memory: [UInt64] = []
        var footprints: [UInt64] = []
        var peaks: [Int] = []
        var oldViews: [WeakView] = []
        var retained: [Int] = []
        var retainedMathViews: [Int] = []
        let cycles = arguments.first { $0.hasPrefix("--math-memory-cycles=") }
            .flatMap { Int($0.dropFirst("--math-memory-cycles=".count)) } ?? 20
        for _ in 0..<max(1, min(100, cycles)) {
            oldViews.append(try await Task { @MainActor in try await reopen(path, workspace: workspace) }.value)
            DiagramInspection.finishEvent()
            try await Task.sleep(for: .milliseconds(100))
            retained.append(oldViews.dropLast().compactMap(\.value).count)
            let active = oldViews.last?.value
            retainedMathViews.append(oldViews.dropLast().compactMap(\.value)
                .filter { $0 !== active }.reduce(0) { $0 + $1.mathViews.count + $1.inlineMath.count + $1.inlineMathViews.count })
            memory.append(try residentBytes())
            footprints.append(try footprintBytes())
            var usage = rusage()
            getrusage(RUSAGE_SELF, &usage)
            peaks.append(usage.ru_maxrss)
        }
        var result: [String: Any] = [
            "reading": true,
            "memory_bytes": memory,
            "footprint_bytes": footprints,
            "peak_bytes": peaks,
            "retained_old_views": retained,
            "retained_old_math_views": retainedMathViews,
        ]
        if let view = oldViews.last?.value {
            result["math_blocks"] = view.mathViews.count
            result["glyphs"] = view.mathViews.reduce(0) { $0 + ($1.item.prepared.math?.glyphCount ?? 0) }
            result["resident_bytes"] = memory.last
        }
        return result
    }
    static func run(workspace: Workspace, document: NoteDocument, view initial: DiagramTextView,
                    source: String, mode: String, arguments: [String], initial result: [String: Any]) async throws -> [String: Any] {
        var result = result
        let view = initial
        guard let coordinator = view.readingCoordinator else { throw CocoaError(.coderValueNotFound) }
        result["math_blocks"] = view.mathViews.count
        let formulas = view.mathViews.map { $0.item.prepared } + view.inlineMath.values.map { $0.item.prepared }
        result["inline_formulas"] = view.inlineMath.count
        result["visible_inline_views"] = view.inlineMathViews.count
        result["math_scale"] = MathTypography.scale
        result["math_errors"] = view.mathViews.compactMap { $0.item.prepared.reason }
        result["inline_errors"] = view.inlineMath.values.compactMap { $0.item.prepared.reason }
        result["glyphs"] = formulas.reduce(0) { $0 + ($1.math?.glyphCount ?? 0) }
        result["system_fallback_glyphs"] = formulas.reduce(0) { $0 + ($1.math?.fallbackGlyphCount ?? 0) }
        result["missing_glyphs"] = formulas.reduce(0) { $0 + ($1.math?.unresolvedGlyphCount ?? 0) }
        result["unresolved_metrics"] = formulas.reduce(0) { $0 + ($1.math?.unresolvedMetricCount ?? 0) }
        result["maximum_metric_error_px"] = (formulas.compactMap { $0.math?.maximumMetricError }.max() ?? 0)
            * (view.window?.backingScaleFactor ?? 2)
        result["main_thread_preparations"] = await MathService.shared.mainThreadPreparations
        result["font_registrations"] = await MathService.shared.registrationCount
        result["fallback_measurements"] = await MathService.shared.measurementCount
        result["measured_fallback_codepoints"] = await MathService.shared.measuredFallbackCodepoints
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
        if arguments.contains("--inline-scroll-end") || arguments.contains("--inline-scroll-start"),
           let attachment = view.inlineMath.values.first(where: { $0.wide }) {
            coordinator.reveal(source: Int(attachment.item.prepared.block.sourceStart))
            view.needsLayout = true; view.layoutSubtreeIfNeeded()
            if let formula = view.inlineMathViews.first(where: { $0.attachment.wide }), let canvas = formula.scroll.documentView {
                let x = arguments.contains("--inline-scroll-end") ? max(0, canvas.bounds.width - formula.scroll.contentSize.width) : 0
                formula.scroll.contentView.scroll(to: CGPoint(x: x, y: 0))
                formula.scroll.reflectScrolledClipView(formula.scroll.contentView)
                formula.displayIfNeeded()
                result["inline_scroll_offset"] = formula.scroll.contentView.bounds.minX
                result["inline_frame"] = NSStringFromRect(formula.frame)
                result["page_horizontal_scroll"] = pageHasHorizontalOverflow(view)
            }
        }
        if arguments.contains("--inline-select"),
           let attachment = view.inlineMath.values.min(by: { $0.item.range.location < $1.item.range.location }) {
            coordinator.reveal(source: Int(attachment.item.prepared.block.sourceStart))
            NSApp.activate(ignoringOtherApps: true); view.window?.makeKeyAndOrderFront(nil)
            for _ in 0..<100 {
                if NSApp.isActive, view.window?.isKeyWindow == true { break }
                try await Task.sleep(for: .milliseconds(10))
            }
            view.setSelectedRange((view.string as NSString).lineRange(for: attachment.item.range))
            view.needsLayout = true; view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
            if arguments.contains("--inline-sidebar-focus"), let root = view.window?.contentView {
                func focusSidebar(_ candidate: NSView) -> Bool {
                    if candidate !== view, candidate is NSOutlineView, view.window?.makeFirstResponder(candidate) == true { return true }
                    return candidate.subviews.contains { focusSidebar($0) }
                }
                result["sidebar_focused"] = focusSidebar(root)
            } else { view.window?.makeFirstResponder(view) }
            view.needsDisplay = true; view.displayIfNeeded()
            result["selected_inline_length"] = view.selectedRange().length
            result["selection_window_key"] = view.window?.isKeyWindow == true && NSApp.isActive
            result["selection_focus_body"] = view.window?.firstResponder === view
            guard let formula = view.inlineMathViews.first(where: { $0.attachment === attachment }) else {
                throw CocoaError(.coderValueNotFound)
            }
            result["selection_background_delta"] = try selectionBackgroundDifference(view, formula)
        }
        if mode == "math-performance" {
            var frames: [Double] = []
            var slowest: [String: Double] = [:]
            for index in 0..<240 {
                let began = ProcessInfo.processInfo.systemUptime
                let y = max(0, view.bounds.height - view.visibleRect.height) * Double(index < 120 ? index : 239-index) / 119
                view.enclosingScrollView?.contentView.scroll(to: CGPoint(x: 0, y: y))
                view.enclosingScrollView?.reflectScrolledClipView(view.enclosingScrollView!.contentView)
                let scrolled = ProcessInfo.processInfo.systemUptime
                view.needsLayout = true; view.layoutSubtreeIfNeeded()
                let laidOut = ProcessInfo.processInfo.systemUptime
                view.window?.displayIfNeeded()
                let ended = ProcessInfo.processInfo.systemUptime
                let elapsed = (ended - began) * 1000
                if elapsed > (frames.max() ?? 0) {
                    slowest = ["index": Double(index), "scroll_ms": (scrolled - began) * 1000,
                        "layout_ms": (laidOut - scrolled) * 1000, "display_ms": (ended - laidOut) * 1000]
                    for (key, value) in view.layoutPhases { slowest["text_" + key] = value }
                }
                frames.append(elapsed)
                try await Task.sleep(for: .milliseconds(1))
            }
            frames.sort(); result["scroll_p95_ms"] = frames[Int(Double(frames.count - 1) * 0.95)]
            result["scroll_max_ms"] = frames.last
            result["slowest_scroll_phases_ms"] = slowest
            result["maximum_main_work_ms"] = view.maximumMainWorkMilliseconds
            result["maximum_formula_draw_ms"] = (view.mathViews.map(\.maximumDrawMilliseconds) + view.inlineMath.values.map(\.maximumDrawMilliseconds)).max()
            let layouts = await MathService.shared.layoutCount
            var reflows: [Double] = []
            if let window = view.window {
                let original = window.contentView!.frame.size
                for index in 0..<30 {
                    let began = ProcessInfo.processInfo.systemUptime
                    window.setContentSize(CGSize(width: index.isMultiple(of: 2) ? original.width - 100 : original.width, height: original.height))
                    window.contentView?.layoutSubtreeIfNeeded(); coordinator.refresh()
                    while coordinator.pending { try await Task.sleep(for: .milliseconds(1)) }
                    view.needsLayout = true; view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
                    reflows.append((ProcessInfo.processInfo.systemUptime - began) * 1000)
                }
            }
            reflows.sort()
            result["resize_p95_ms"] = reflows.isEmpty ? 0 : reflows[Int(Double(reflows.count - 1) * 0.95)]
            result["resize_formula_layouts"] = await MathService.shared.layoutCount - layouts
        } else if mode == "math-external" {
            let inline = view.inlineMath.values.sorted { $0.item.range.location < $1.item.range.location }
            guard let formula = inline.dropFirst(inline.count / 2).first?.item ?? view.mathViews.first?.item else { throw CocoaError(.coderValueNotFound) }
            coordinator.reveal(source: Int(formula.prepared.block.sourceStart))
            try await Task.sleep(for: .milliseconds(50))
            view.layoutSubtreeIfNeeded()
            let before = try coordinator.visibleAnchor().unwrap()
            result["anchor_before"] = ["id": String(before.id), "source": String(before.source), "offset": String(before.offset)]
            let old = view.mathViews
            let oldInline = Array(view.inlineMath.values)
            let oldInlineRanges = Dictionary(oldInline.map { (ObjectIdentifier($0), $0.item.range) }, uniquingKeysWith: { first, _ in first })
            let prefixEdit = arguments.contains("--math-insert-prefix")
            let changed = prefixEdit ? "Inserted prefix " + source : (source as NSString).replacingCharacters(in: NSRange(location: Int(formula.prepared.block.sourceStart),
                length: Int(formula.prepared.block.sourceLength)), with: formula.prepared.block.block ? "$$x^3+y^3=z^3$$" : "$x^3+y^3=z^3$")
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
            result["attachment_invalidation"] = view.inlineMathInvalidation.map { ["location": $0.location, "length": $0.length] } ?? [:]
            result["shifted_inline_attachments"] = view.inlineMath.values.filter { current in
                oldInlineRanges[ObjectIdentifier(current)].map { $0 != current.item.range } ?? false
            }.count
            result["reused_math_views"] = view.mathViews.filter { current in old.contains { $0 === current } }.count
            result["reused_inline_attachments"] = view.inlineMath.values.filter { current in oldInline.contains { $0 === current } }.count
            result["anchor_shift_pt"] = abs(before.offset - after.offset)
            result["same_anchor"] = before.id == after.id
        }
        result["visible_inline_views"] = view.inlineMathViews.count
        return result
    }
}

private extension Optional {
    func unwrap() throws -> Wrapped {
        guard let value = self else { throw CocoaError(.coderValueNotFound) }
        return value
    }
}
