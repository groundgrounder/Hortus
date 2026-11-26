import AppKit
import os

/// 阅读视图查找适配器：将正文与公式（行内/块级 LaTeX 源码）统一映射到虚拟检索串，
/// 复用系统原生 NSTextFinder 查找栏实现 ⌘F / ⌘G / ⇧⌘G，不影响编辑视图的查找与替换。
final class ReadingTextFinderClient: NSObject, NSTextFinderClient, @unchecked Sendable {
    enum Segment {
        case text(virtual: NSRange, real: NSRange)
        case formula(virtual: NSRange, real: NSRange, item: NoteMath)

        var virtualRange: NSRange {
            switch self {
            case .text(let v, _), .formula(let v, _, _): return v
            }
        }
        var realRange: NSRange {
            switch self {
            case .text(_, let r), .formula(_, let r, _): return r
            }
        }
        var formulaItem: NoteMath? {
            if case .formula(_, _, let item) = self { return item }
            return nil
        }
    }

    @MainActor weak var textView: DiagramTextView?
    @MainActor let finder = NSTextFinder()
    private let textLock = OSAllocatedUnfairLock<String>(initialState: "")
    @MainActor private var segments: [Segment] = []
    @MainActor private var dirty = true

    @MainActor
    init(textView: DiagramTextView) {
        self.textView = textView
        super.init()
        finder.client = self
        finder.isIncrementalSearchingEnabled = true
        finder.incrementalSearchingShouldDimContentView = false
    }

    @MainActor
    func willChangeContent() {
        dirty = true
        finder.noteClientStringWillChange()
    }

    @MainActor
    func didUpdateContent() {
        dirty = true
        ensureIndex()
    }

    @MainActor
    func invalidate() {
        willChangeContent()
    }

    @MainActor
    func attachContainerIfNeeded() {
        guard let scroll = textView?.enclosingScrollView else { return }
        if finder.findBarContainer !== scroll {
            finder.findBarContainer = scroll
        }
    }

    @MainActor
    func performAction(_ sender: Any?) {
        attachContainerIfNeeded()
        ensureIndex()
        guard let raw = (sender as? NSMenuItem)?.tag ?? (sender as? NSControl)?.tag,
              let action = NSTextFinder.Action(rawValue: raw) else { return }
        if finder.validateAction(action) {
            finder.performAction(action)
        }
    }

    @MainActor
    func performAction(_ action: NSTextFinder.Action) {
        attachContainerIfNeeded()
        ensureIndex()
        if finder.validateAction(action) {
            finder.performAction(action)
        }
    }

    @MainActor
    func matches(for query: String) -> [(virtualRange: NSRange, realRange: NSRange, isFormula: Bool)] {
        ensureIndex()
        let snapshot = textLock.withLock { $0 } as NSString
        guard !query.isEmpty else { return [] }
        var results: [(NSRange, NSRange, Bool)] = []
        var searchRange = NSRange(location: 0, length: snapshot.length)
        while searchRange.length > 0 {
            let found = snapshot.range(of: query, options: [.caseInsensitive], range: searchRange)
            guard found.location != NSNotFound else { break }
            let real = realRange(for: found)
            let isFormula = segments.contains { seg in
                seg.formulaItem != nil && NSIntersectionRange(seg.virtualRange, found).length > 0
            }
            results.append((found, real, isFormula))
            let nextStart = NSMaxRange(found)
            searchRange = NSRange(location: nextStart, length: snapshot.length - nextStart)
        }
        return results
    }

    @MainActor
    private func ensureIndex() {
        guard dirty, let textView, let storage = textView.textStorage else { return }
        dirty = false
        let raw = storage.string as NSString
        var formulaByLocation: [Int: NoteMath] = [:]
        for (loc, attachment) in textView.inlineMath {
            formulaByLocation[loc] = attachment.item
        }
        for math in textView.mathViews {
            formulaByLocation[math.item.range.location] = math.item
        }
        let sortedLocations = formulaByLocation.keys.sorted()
        let built = NSMutableString()
        var nextSegments: [Segment] = []
        var cursor = 0
        for loc in sortedLocations {
            guard loc >= cursor, loc < raw.length, let item = formulaByLocation[loc] else { continue }
            if loc > cursor {
                let realRange = NSRange(location: cursor, length: loc - cursor)
                let piece = raw.substring(with: realRange)
                let vStart = built.length
                built.append(piece)
                nextSegments.append(.text(virtual: NSRange(location: vStart, length: built.length - vStart),
                                          real: realRange))
            }
            let realFormulaRange = NSRange(location: loc, length: min(item.range.length, raw.length - loc))
            let latex = item.prepared.block.latex
            let vStart = built.length
            built.append(latex)
            nextSegments.append(.formula(virtual: NSRange(location: vStart, length: built.length - vStart),
                                         real: realFormulaRange, item: item))
            cursor = NSMaxRange(realFormulaRange)
        }
        if cursor < raw.length {
            let realRange = NSRange(location: cursor, length: raw.length - cursor)
            let piece = raw.substring(with: realRange)
            let vStart = built.length
            built.append(piece)
            nextSegments.append(.text(virtual: NSRange(location: vStart, length: built.length - vStart),
                                      real: realRange))
        }
        let immutable = built as String
        textLock.withLock { $0 = immutable }
        segments = nextSegments
    }

    @MainActor
    private func virtualRange(for realRange: NSRange) -> NSRange {
        ensureIndex()
        let length = textLock.withLock { $0.utf16.count }
        guard realRange.location != NSNotFound else { return NSRange(location: 0, length: 0) }
        if realRange.length == 0 {
            for seg in segments {
                let r = seg.realRange
                if realRange.location >= r.location && realRange.location <= NSMaxRange(r) {
                    if case .text(let v, _) = seg {
                        return NSRange(location: v.location + (realRange.location - r.location), length: 0)
                    }
                    return NSRange(location: seg.virtualRange.location, length: 0)
                }
            }
            return NSRange(location: length, length: 0)
        }
        var start: Int?
        var end = 0
        for seg in segments {
            let inter = NSIntersectionRange(seg.realRange, realRange)
            guard inter.length > 0 else { continue }
            let mapped: NSRange
            if case .text(let v, let r) = seg {
                mapped = NSRange(location: v.location + (inter.location - r.location), length: inter.length)
            } else {
                mapped = seg.virtualRange
            }
            if start == nil { start = mapped.location }
            end = max(end, NSMaxRange(mapped))
        }
        if let start { return NSRange(location: start, length: max(0, end - start)) }
        return NSRange(location: 0, length: 0)
    }

    @MainActor
    func realRange(for vRange: NSRange) -> NSRange {
        ensureIndex()
        guard vRange.location != NSNotFound else { return NSRange(location: 0, length: 0) }
        if vRange.length == 0 {
            for seg in segments {
                let v = seg.virtualRange
                if vRange.location >= v.location && vRange.location <= NSMaxRange(v) {
                    if case .text(_, let r) = seg {
                        return NSRange(location: r.location + (vRange.location - v.location), length: 0)
                    }
                    return NSRange(location: seg.realRange.location, length: 0)
                }
            }
            return NSRange(location: textView?.sourceUTF16Length ?? 0, length: 0)
        }
        var start: Int?
        var end = 0
        for seg in segments {
            let inter = NSIntersectionRange(seg.virtualRange, vRange)
            guard inter.length > 0 else { continue }
            let mapped: NSRange
            if case .text(let v, let r) = seg {
                mapped = NSRange(location: r.location + (inter.location - v.location), length: inter.length)
            } else {
                mapped = seg.realRange
            }
            if start == nil { start = mapped.location }
            end = max(end, NSMaxRange(mapped))
        }
        if let start { return NSRange(location: start, length: max(0, end - start)) }
        return NSRange(location: 0, length: 0)
    }

    // MARK: NSTextFinderClient

    override var isSelectable: Bool { true }
    var allowsMultipleSelection: Bool { false }
    var isEditable: Bool { false }

    func stringLength() -> Int {
        if Thread.isMainThread {
            MainActor.assumeIsolated { ensureIndex() }
        }
        return textLock.withLock { $0.utf16.count }
    }

    func string(at characterIndex: Int, effectiveRange outRange: NSRangePointer, endsWithSearchBoundary outFlag: UnsafeMutablePointer<ObjCBool>) -> String {
        if Thread.isMainThread {
            MainActor.assumeIsolated { ensureIndex() }
        }
        let snapshot = textLock.withLock { $0 }
        outRange.pointee = NSRange(location: 0, length: snapshot.utf16.count)
        outFlag.pointee = true
        return snapshot
    }

    var firstSelectedRange: NSRange {
        MainActor.assumeIsolated {
            guard let textView else { return NSRange(location: 0, length: 0) }
            return virtualRange(for: textView.selectedRange())
        }
    }

    var selectedRanges: [NSValue] {
        get {
            let ranges: [NSRange] = MainActor.assumeIsolated {
                guard let textView else { return [] }
                return textView.selectedRanges.map { virtualRange(for: $0.rangeValue) }
            }
            return ranges.map { NSValue(range: $0) }
        }
        set {
            let rawRanges = newValue.map(\.rangeValue)
            MainActor.assumeIsolated {
                guard let textView else { return }
                textView.selectedRanges = rawRanges.map { NSValue(range: realRange(for: $0)) }
            }
        }
    }

    func scrollRangeToVisible(_ range: NSRange) {
        MainActor.assumeIsolated {
            guard let textView else { return }
            ensureIndex()
            let real = realRange(for: range)
            textView.revealCharacter(at: real.location)
            if let mathSegment = segments.first(where: { $0.formulaItem != nil && NSIntersectionRange($0.virtualRange, range).length > 0 }),
               let item = mathSegment.formulaItem {
                if let blockView = textView.mathViews.first(where: { $0.item.identity == item.identity }), !blockView.frame.isEmpty {
                    _ = textView.scrollToVisible(blockView.frame.insetBy(dx: 0, dy: -12))
                } else if let attachment = textView.inlineMath[item.range.location],
                          let inlineView = textView.inlineMathViews.first(where: { $0.attachment === attachment }), !inlineView.frame.isEmpty {
                    _ = textView.scrollToVisible(inlineView.frame.insetBy(dx: 0, dy: -12))
                }
            }
        }
    }

    func contentView(at index: Int, effectiveCharacterRange outRange: NSRangePointer) -> NSView {
        let (view, length): (NSView, Int) = MainActor.assumeIsolated {
            ensureIndex()
            let length = textLock.withLock { $0.utf16.count }
            return (textView ?? NSView(), length)
        }
        outRange.pointee = NSRange(location: 0, length: length)
        return view
    }

    func rects(forCharacterRange range: NSRange) -> [NSValue]? {
        let boxes: [CGRect] = MainActor.assumeIsolated {
            guard let textView, let window = textView.window else { return [] }
            ensureIndex()
            var rects: [CGRect] = []
            for seg in segments {
                let inter = NSIntersectionRange(seg.virtualRange, range)
                guard inter.length > 0 else { continue }
                switch seg {
                case .text(let v, let r):
                    let realSub = NSRange(location: r.location + (inter.location - v.location), length: inter.length)
                    let screenRect = textView.firstRect(forCharacterRange: realSub, actualRange: nil)
                    if !screenRect.isEmpty {
                        let viewRect = textView.convert(window.convertFromScreen(screenRect), from: nil)
                        rects.append(viewRect)
                    }
                case .formula(_, let r, let item):
                    if let blockView = textView.mathViews.first(where: { $0.item.identity == item.identity }), !blockView.frame.isEmpty {
                        rects.append(blockView.frame)
                    } else if let attachment = textView.inlineMath[item.range.location],
                              let inlineView = textView.inlineMathViews.first(where: { $0.attachment === attachment }), !inlineView.frame.isEmpty {
                        rects.append(inlineView.frame)
                    } else {
                        let screenRect = textView.firstRect(forCharacterRange: r, actualRange: nil)
                        if !screenRect.isEmpty {
                            let viewRect = textView.convert(window.convertFromScreen(screenRect), from: nil)
                            rects.append(viewRect)
                        }
                    }
                }
            }
            return rects
        }
        return boxes.isEmpty ? nil : boxes.map { NSValue(rect: $0) }
    }
}
