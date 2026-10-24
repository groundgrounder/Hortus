import AppKit
import HortusCore

struct SourceStyleRun: Sendable {
    var start, length: UInt64
    var mask: UInt32
}
private struct StyleBatch: Sendable {
    let revision: UInt64
    let runs: [SourceStyleRun]
    let coreMilliseconds, ffiMilliseconds: Double
    let queuedMilliseconds, decodedAt: Double
}

extension NSAttributedString.Key {
    static let sourceStyle = NSAttributedString.Key("HortusSourceStyle")
}

/// 仅为原生显示设置属性。文档会话始终只接收字符变更，保存不序列化属性。
@MainActor
final class SourceStyling {
    var task: Task<Void, Never>?
    private var again = false
    private var force = false
    private var revision: UInt64 = .max
    var appliedRevision: UInt64 { revision }
    private var generation = 0
    var pending = false
    var applications = 0
    var appliedCharacters = 0
    var lastLatencyMilliseconds = 0.0
    var openMilliseconds = 0.0
    var openDisplayedAt = 0.0
    var openCoreMilliseconds = 0.0
    var openFFIMilliseconds = 0.0
    var lastCoreMilliseconds = 0.0
    var lastFFIMilliseconds = 0.0
    var lastPhases: [String: Double] = [:]
    private var requestedAt = 0.0
    private var requestedRevision: UInt64 = .max
    private var openedAt = 0.0
    private(set) var runs: [SourceStyleRun] = []
    private var attributes: [UInt32: [NSAttributedString.Key: Any]] = [:]
    private var fontSize = 0.0
    private var contrast = false

    func reset() {
        generation += 1; task?.cancel(); task = nil
        runs = []; revision = .max; force = true; again = false; pending = false
        openedAt = ProcessInfo.processInfo.systemUptime
        openMilliseconds = 0
        openDisplayedAt = 0
    }
    func schedule(view: DiagramTextView, document: NoteDocument?, size: Double, force: Bool = false) {
        guard let document, !view.compositionActive else { return }
        let contrast = view.increasedContrast
        if fontSize != size || self.contrast != contrast {
            fontSize = size; self.contrast = contrast; attributes = [:]; self.force = true
        }
        self.force = self.force || force
        guard self.force || revision != document.revision else { return }
        // 原生字符通知、提交通知与 SwiftUI 更新可能指向同一次编辑。
        guard !pending || requestedRevision != document.revision || self.force else { return }
        requestedRevision = document.revision
        pending = true; again = true; requestedAt = ProcessInfo.processInfo.systemUptime
        guard task == nil else { return }
        let token = generation
        let first = Self.request(document.io, since: revision, force: self.force)
        self.force = false; again = false
        task = Task { [weak self, weak view, weak document] in
            guard let self, let view, let document else { return }
            defer { if token == generation { task = nil; pending = false } }
            var next = first
            while token == generation && !Task.isCancelled {
                guard !view.compositionActive else { return }
                do {
                    let styles = try await next.value
                    guard token == generation, !Task.isCancelled else { return }
                    guard !view.compositionActive else { self.force = true; return }
                    if styles.revision == document.revision {
                        lastCoreMilliseconds = styles.coreMilliseconds; lastFFIMilliseconds = styles.ffiMilliseconds
                        let applyStart = ProcessInfo.processInfo.systemUptime
                        apply(styles.runs, to: view)
                        revision = styles.revision
                        view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
                        lastPhases = ["queue_ms": styles.queuedMilliseconds, "core_ms": styles.coreMilliseconds,
                            "ffi_ms": styles.ffiMilliseconds, "main_wait_ms": (applyStart - styles.decodedAt) * 1000,
                            "apply_display_ms": (ProcessInfo.processInfo.systemUptime - applyStart) * 1000]
                        lastLatencyMilliseconds = (ProcessInfo.processInfo.systemUptime - requestedAt) * 1000
                        if openMilliseconds == 0 {
                            openDisplayedAt = ProcessInfo.processInfo.systemUptime
                            openMilliseconds = (openDisplayedAt - openedAt) * 1000
                            openCoreMilliseconds = lastCoreMilliseconds; openFFIMilliseconds = lastFFIMilliseconds
                        }
                    } else { again = true }
                } catch { view.diagramIssue = L10n.string("Diagram analysis failed"); return }
                guard again else { return }
                again = false
                next = Self.request(document.io, since: revision, force: self.force)
                self.force = false
            }
        }
    }
    private nonisolated static func request(_ io: DocumentIO, since: UInt64, force: Bool) -> Task<StyleBatch, Error> {
        // 在字符通知里立即启动后台分析，与原生编辑事务余下的主线程工作并行。
        let queuedAt = ProcessInfo.processInfo.systemUptime
        return Task.detached(priority: .userInitiated) {
            try await io.perform { session in
                let start = ProcessInfo.processInfo.systemUptime
                let wire = try session.sourceStyles(since: since, force: force)
                let ffi = (ProcessInfo.processInfo.systemUptime - start) * 1000
                let runs = wire.packedRuns.withUnsafeBytes { buffer in
                    var result: [SourceStyleRun] = []
                    result.reserveCapacity(buffer.count / 20)
                    for offset in stride(from: 0, to: buffer.count, by: 20) {
                        result.append(SourceStyleRun(start: UInt64(littleEndian: buffer.loadUnaligned(fromByteOffset: offset, as: UInt64.self)),
                            length: UInt64(littleEndian: buffer.loadUnaligned(fromByteOffset: offset + 8, as: UInt64.self)),
                            mask: UInt32(littleEndian: buffer.loadUnaligned(fromByteOffset: offset + 16, as: UInt32.self))))
                    }
                    return result
                }
                return StyleBatch(revision: wire.revision, runs: runs, coreMilliseconds: Double(wire.analysisMicros) / 1000, ffiMilliseconds: ffi,
                    queuedMilliseconds: (start - queuedAt) * 1000, decodedAt: ProcessInfo.processInfo.systemUptime)
            }
        }
    }
    private func firstRun(endingAfter position: UInt64) -> Int {
        var low = 0, high = runs.count
        while low < high {
            let mid = (low + high) / 2
            if runs[mid].start + runs[mid].length <= position { low = mid + 1 } else { high = mid }
        }
        return low
    }
    func mask(at position: Int) -> UInt32 {
        let index = firstRun(endingAfter: UInt64(max(0, position)))
        guard index < runs.count, runs[index].start <= UInt64(max(0, position)) else { return 0 }
        return runs[index].mask
    }
    func overlapping(_ range: NSRange, _ body: (SourceStyleRun, NSRange) -> Void) {
        var index = firstRun(endingAfter: UInt64(range.location))
        while index < runs.count, runs[index].start < UInt64(NSMaxRange(range)) {
            let run = runs[index]
            let intersection = NSIntersectionRange(range, NSRange(location: Int(run.start), length: Int(run.length)))
            if intersection.length > 0 { body(run, intersection) }
            index += 1
        }
    }
    func shift(for range: NSRange, delta: Int) {
        let end = NSMaxRange(range)
        for index in firstRun(endingAfter: UInt64(range.location))..<runs.count {
            var run = runs[index]
            var start = Int(run.start), tail = Int(run.start + run.length)
            if start >= end { start += delta } else if start > range.location { start = range.location }
            if tail >= end { tail += delta } else if tail > range.location { tail = range.location }
            run.start = UInt64(max(0, start)); run.length = UInt64(max(0, tail - start))
            runs[index] = run
        }
    }
    func apply(_ patch: [SourceStyleRun], to view: DiagramTextView) {
        guard !view.compositionActive, let storage = view.textStorage else { return }
        let base = style(view.representedPath.lowercased().hasSuffix(".mmd") ? 32 : 0)
        view.sourceTypingAttributes = base; view.defaultParagraphStyle = base[.paragraphStyle] as? NSParagraphStyle
        view.typingAttributes = base
        if storage.length == 0 { runs = []; return }
        guard let head = patch.first, let tail = patch.last else { return }
        let start = ProcessInfo.processInfo.systemUptime
        let lower = head.start, upper = tail.start + tail.length
        let range = NSRange(location: Int(lower), length: Int(upper - lower))
        guard NSMaxRange(range) <= storage.length else { return }
        let initial = runs.isEmpty
        if lower == 0 && upper == UInt64(storage.length) { runs = patch }
        else {
            let a = firstRun(endingAfter: lower)
            var b = firstRun(endingAfter: upper)
            if b < runs.count && runs[b].start < upper { b += 1 }
            var replacement: [SourceStyleRun] = []
            if a < runs.count && runs[a].start < lower {
                replacement.append(SourceStyleRun(start: runs[a].start, length: lower - runs[a].start, mask: runs[a].mask))
            }
            replacement.append(contentsOf: patch)
            if b > 0 && runs[b - 1].start + runs[b - 1].length > upper {
                let old = runs[b - 1]
                replacement.append(SourceStyleRun(start: upper, length: old.start + old.length - upper, mask: old.mask))
            }
            runs.replaceSubrange(a..<b, with: replacement)
        }
        // 原始存储只保留统一的正文属性；语义样式在等长的显示段落中生成。
        // 变更通知仅使受影响段落失效，不把样式写进文本或撤销记录。
        view.diagramStorage?.performEditingTransaction {
            if initial { storage.setAttributes(base, range: NSRange(location: 0, length: storage.length)) }
            storage.edited(.editedAttributes, range: range, changeInLength: 0)
        }
        appliedCharacters += range.length; applications += 1
        view.needsLayout = true; view.needsDisplay = true
        view.recordMainWork(since: start)
    }
    func paragraph(_ source: NSAttributedString, range: NSRange, in view: DiagramTextView) -> NSAttributedString {
        let result = NSMutableAttributedString(attributedString: source)
        let marked = view.compositionActive ? view.markedRange() : NSRange(location: NSNotFound, length: 0)
        // setMarkedText 调用原生实现前，组合范围尚未建立，此时完整保留原生属性。
        if view.compositionActive && marked.location == NSNotFound { return result }
        let codeMask: UInt32 = view.representedPath.lowercased().hasSuffix(".mmd") ? 32 : 0
        func apply(_ attributes: [NSAttributedString.Key: Any], _ target: NSRange) {
            let blocked = marked.location == NSNotFound ? NSRange(location: 0, length: 0) : NSIntersectionRange(marked, target)
            if blocked.length == 0 { result.setAttributes(attributes, range: NSRange(location: target.location - range.location, length: target.length)) }
            else {
                if target.location < blocked.location { result.setAttributes(attributes, range: NSRange(location: target.location - range.location, length: blocked.location - target.location)) }
                if NSMaxRange(target) > NSMaxRange(blocked) { result.setAttributes(attributes, range: NSRange(location: NSMaxRange(blocked) - range.location, length: NSMaxRange(target) - NSMaxRange(blocked))) }
            }
        }
        apply(style(codeMask), range)
        overlapping(range) { run, intersection in apply(style(run.mask | codeMask), intersection) }
        return result
    }
    func installReading(_ next: [SourceStyleRun], size: Double) {
        fontSize = size; attributes = [:]; runs = next
    }
    func style(_ mask: UInt32) -> [NSAttributedString.Key: Any] {
        if let cached = attributes[mask] { return cached }
        let heading = Int(mask >> 24)
        let code = mask & 48 != 0
        let size = fontSize * ([1: 1.6, 2: 1.35, 3: 1.15][heading] ?? 1)
        var font = code ? NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
            : NSFont.systemFont(ofSize: size, weight: heading > 0 || mask & 2 != 0 ? .bold : .regular)
        if mask & 4 != 0 { font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask) }
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = 4
        paragraph.paragraphSpacing = heading > 0 ? 6 : 0
        paragraph.paragraphSpacingBefore = heading > 0 ? 10 : 0
        var color = NSColor.textColor
        if mask & (128 | 256 | 512 | 8192) != 0 { color = .controlAccentColor }
        if mask & (64 | 2048 | 16384) != 0 { color = .secondaryLabelColor }
        if mask & 1 != 0 { color = contrast ? .secondaryLabelColor : .tertiaryLabelColor }
        var result: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color,
            .paragraphStyle: paragraph, .sourceStyle: mask]
        if mask & 4 != 0 && !font.fontDescriptor.symbolicTraits.contains(.italic) { result[.obliqueness] = 0.12 }
        if mask & 8 != 0 { result[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
        if mask & 16 != 0 { result[.backgroundColor] = NSColor.quaternaryLabelColor.withAlphaComponent(0.10) }
        if mask & 1024 != 0 { result[.backgroundColor] = NSColor.systemYellow.withAlphaComponent(0.19) }
        attributes[mask] = result
        return result
    }
}
