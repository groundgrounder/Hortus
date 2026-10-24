import AppKit
import SwiftUI
import HortusCore

struct ReadingRun: Sendable {
    let start, length, sourceStart, sourceLength: UInt64
    let mask: UInt32
}

struct ReadingBlock: Sendable {
    let id, sourceStart, sourceLength, displayStart, displayLength: UInt64
    let runRange: Range<Int>
    let diagram: Bool
}

struct ReadingViewModel: Sendable {
    let revision, rebuilt: UInt64
    let text: String
    let blocks: [ReadingBlock]
    private let packedRuns: Data
    private let runRecordSize: Int

    private init(revision: UInt64, rebuilt: UInt64, text: String, blocks: [ReadingBlock],
                 packedRuns: Data, runRecordSize: Int) {
        self.revision = revision; self.rebuilt = rebuilt; self.text = text; self.blocks = blocks
        self.packedRuns = packedRuns; self.runRecordSize = runRecordSize
    }

    init(_ wire: HortusCore.ReadingModel) throws {
        func layout(_ data: Data, legacySize: Int, compactSize: Int) throws -> (Int, Int, Int) {
            guard data.count >= 8 else { throw CocoaError(.fileReadCorruptFile) }
            let (version, count) = data.withUnsafeBytes { raw in
                (UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 0, as: UInt32.self)),
                 UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 4, as: UInt32.self)))
            }
            let recordSize: Int
            switch version {
            case 1: recordSize = legacySize
            case 2: recordSize = compactSize
            default: throw CocoaError(.fileReadCorruptFile)
            }
            let (size, overflow) = Int(count).multipliedReportingOverflow(by: recordSize)
            guard !overflow, data.count == 8 + size else { throw CocoaError(.fileReadCorruptFile) }
            return (Int(version), Int(count), recordSize)
        }
        let (blockVersion, blockCount, _) = try layout(wire.packedBlocks, legacySize: 64, compactSize: 36)
        let (runVersion, runCount, runRecordSize) = try layout(wire.packedRuns, legacySize: 40, compactSize: 20)
        guard blockVersion == runVersion else { throw CocoaError(.fileReadCorruptFile) }
        let blocks = wire.packedBlocks.withUnsafeBytes { raw in
            (0..<blockCount).map { index -> ReadingBlock in
                let base = 8 + index * (blockVersion == 2 ? 36 : 64)
                func value64(_ offset: Int) -> UInt64 {
                    UInt64(littleEndian: raw.loadUnaligned(fromByteOffset: base + offset, as: UInt64.self))
                }
                func value32(_ offset: Int) -> UInt64 {
                    UInt64(UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: base + offset, as: UInt32.self)))
                }
                if blockVersion == 2 {
                    let first = Int(value32(24)), length = Int(value32(28))
                    return ReadingBlock(id: value64(0), sourceStart: value32(8), sourceLength: value32(12),
                        displayStart: value32(16), displayLength: value32(20), runRange: first..<(first + length),
                        diagram: value32(32) != 0)
                }
                let first = Int(value64(40)), length = Int(value64(48))
                return ReadingBlock(id: value64(0), sourceStart: value64(8), sourceLength: value64(16),
                    displayStart: value64(24), displayLength: value64(32), runRange: first..<(first + length),
                    diagram: value64(56) != 0)
            }
        }
        guard blocks.allSatisfy({ $0.runRange.upperBound <= runCount }) else { throw CocoaError(.fileReadCorruptFile) }
        self.init(revision: wire.revision, rebuilt: wire.rebuilt, text: wire.text, blocks: blocks,
                  packedRuns: wire.packedRuns, runRecordSize: runRecordSize)
    }

    func applying(_ wire: HortusCore.ReadingModel) throws -> ReadingViewModel {
        if wire.deltaIndex == UInt64.max - 1 { return self }
        guard wire.deltaIndex < UInt64(blocks.count), !wire.packedRuns.isEmpty else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let index = Int(wire.deltaIndex)
        guard let layout = Self.runLayout(wire.packedRuns), layout.version == (runRecordSize == 20 ? 2 : 1),
              wire.packedRuns.count == 8 + layout.count * layout.recordSize else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let oldBlock = blocks[index]
        let newRunCount = layout.count
        let oldRunCount = oldBlock.runRange.count
        let runDelta = newRunCount - oldRunCount
        let sourceDelta = Int64(wire.sourceLength) - Int64(oldBlock.sourceLength)
        let displayDelta = Int64(wire.displayLength) - Int64(oldBlock.displayLength)
        let replaced = (text as NSString).replacingCharacters(
            in: NSRange(location: Int(oldBlock.displayStart), length: Int(oldBlock.displayLength)),
            with: wire.text)
        var nextBlocks = blocks
        for blockIndex in nextBlocks.indices {
            if blockIndex == index {
                nextBlocks[blockIndex] = ReadingBlock(id: wire.blockId,
                    sourceStart: wire.sourceStart, sourceLength: wire.sourceLength,
                    displayStart: wire.displayStart, displayLength: wire.displayLength,
                    runRange: oldBlock.runRange.lowerBound..<(oldBlock.runRange.lowerBound + newRunCount),
                    diagram: wire.diagram)
            } else if blockIndex > index {
                let block = nextBlocks[blockIndex]
                nextBlocks[blockIndex] = ReadingBlock(id: block.id,
                    sourceStart: UInt64(Int64(block.sourceStart) + sourceDelta),
                    sourceLength: block.sourceLength,
                    displayStart: UInt64(Int64(block.displayStart) + displayDelta),
                    displayLength: block.displayLength,
                    runRange: (block.runRange.lowerBound + runDelta)..<(block.runRange.upperBound + runDelta),
                    diagram: block.diagram)
            }
        }
        var packed = Data()
        let oldBodyStart = 8 + oldBlock.runRange.lowerBound * runRecordSize
        let oldBodyEnd = 8 + oldBlock.runRange.upperBound * runRecordSize
        packed.append(packedRuns.prefix(oldBodyStart))
        packed.append(wire.packedRuns.dropFirst(8))
        packed.append(packedRuns.suffix(from: oldBodyEnd))
        var count = UInt32(blocks.reduce(0) { $0 + $1.runRange.count } + runDelta)
        withUnsafeBytes(of: &count) { packed.replaceSubrange(4..<8, with: $0) }
        return ReadingViewModel(revision: wire.revision, rebuilt: wire.rebuilt, text: replaced,
            blocks: nextBlocks, packedRuns: packed, runRecordSize: runRecordSize)
    }

    private static func runLayout(_ data: Data) -> (version: Int, count: Int, recordSize: Int)? {
        guard data.count >= 8 else { return nil }
        let (version, count) = data.withUnsafeBytes { raw in
            (UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 0, as: UInt32.self)),
             UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: 4, as: UInt32.self)))
        }
        let recordSize: Int
        switch version {
        case 1: recordSize = 40
        case 2: recordSize = 20
        default: return nil
        }
        return (Int(version), Int(count), recordSize)
    }

    func run(at index: Int) -> ReadingRun {
        let base = 8 + index * runRecordSize
        return packedRuns.withUnsafeBytes { raw in
            if runRecordSize == 20 {
                func value(_ offset: Int) -> UInt64 {
                    UInt64(UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: base + offset, as: UInt32.self)))
                }
                return ReadingRun(start: value(0), length: value(4), sourceStart: value(8),
                    sourceLength: value(12), mask: UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: base + 16, as: UInt32.self)))
            } else {
                func value(_ field: Int) -> UInt64 {
                    UInt64(littleEndian: raw.loadUnaligned(fromByteOffset: base + field * 8, as: UInt64.self))
                }
                return ReadingRun(start: value(0), length: value(1), sourceStart: value(2),
                    sourceLength: value(3), mask: UInt32(truncatingIfNeeded: value(4)))
            }
        }
    }

    func firstRun(in range: Range<Int>, matching predicate: (ReadingRun) -> Bool) -> ReadingRun? {
        for index in range {
            let run = run(at: index)
            if predicate(run) { return run }
        }
        return nil
    }
}

private struct ReadingRefreshResult: Sendable {
    let model: ReadingViewModel
    let snapshot: DocumentSnapshot?
    let media: [PreparedMedia]
    var phases: [String: Double]
}

struct ReadingText: NSViewRepresentable {
    let document: NoteDocument
    let path, vault: String
    let fontSize: Double
    let service: DiagramService
    let core: WorkspaceCore?
    let remember: (UInt32, [String]) -> Void
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.drawsBackground = false; scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        let view = DiagramTextView(diagramLayout: true)
        view.isReading = true; view.isEditable = false; view.isSelectable = true; view.isRichText = false
        view.drawsBackground = false; view.usesFindBar = true; view.allowsUndo = false
        view.isVerticallyResizable = true; view.isHorizontallyResizable = false
        view.textContainerInset = NSSize(width: 30, height: 24)
        view.textContainer?.lineFragmentPadding = 0; view.textContainer?.widthTracksTextView = true
        view.autoresizingMask = [.width]; scroll.documentView = view
        context.coordinator.view = view; view.readingCoordinator = context.coordinator
        return scroll
    }
    func updateNSView(_ scroll: NSScrollView, context: Context) { context.coordinator.update(self) }
    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.detach()
        guard let view = scroll.documentView as? DiagramTextView else { return }
        view.onWidthOrThemeChange = nil; view.onToggle = nil; view.readingCoordinator = nil
        for diagram in view.diagramViews.values { diagram.detach() }
        for media in view.mediaViews { media.detach(); media.removeFromSuperview() }
        view.mediaViews = []; view.mediaBlocks = []
        view.diagramViews = [:]; view.string = ""; view.textLayoutManager?.delegate = nil
        if let manager = view.textLayoutManager { manager.textContentManager?.removeTextLayoutManager(manager) }
        view.diagramStorage = nil; scroll.documentView = nil
        NSWorkspace.shared.notificationCenter.removeObserver(view,
            name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil)
    }

    @MainActor final class Coordinator: NSObject, @preconcurrency NSTextLayoutManagerDelegate {
        private struct RefreshKey: Equatable {
            let revision: UInt64
            let width: Double
            let dark: Bool
            let size: Double
            let forcedGeneration: UInt64
        }

        weak var view: DiagramTextView?
        var input: ReadingText?
        var model: ReadingViewModel?
        var ranges: [NSRange] = []
        var task: Task<Void, Never>?
        let styling = SourceStyling()
        var pending = false
        var submittedRequests = 0
        var latencyMilliseconds = 0.0
        private var generation = 0
        private var inFlightToken: Int?
        private var inFlightKey: RefreshKey?
        private var refreshQueued = false
        private var forcedGeneration: UInt64 = 0
        private var width = 0.0
        private var dark = false
        private var size = 0.0
        private var source = ""
        private var fallbacks: [Bool] = []
        var displayedAt = 0.0
        var firstRefreshStartedAt = 0.0
        var changedCharacters = 0
        var phaseMilliseconds: [String: Double] = [:]
        private var rendered: [NoteDiagram] = []
        private var initial = true
        private var pendingInitialSource: Int?
        private var pendingRevealSource: Int?
        private var pendingAnchor: (id: UInt64, source: Int, offset: Double)?
        var revealPending: Bool { pendingRevealSource != nil }

        func update(_ input: ReadingText) {
            let changed = self.input?.document !== input.document
            if changed { detach(); model = nil; ranges = []; fallbacks = []; rendered = []; initial = true }
            self.input = input
            guard let view else { return }
            view.representedPath = input.path; view.representedDocument = ObjectIdentifier(input.document)
            view.sourceStyling = styling; view.textLayoutManager?.delegate = self
            view.setAccessibilityLabel(L10n.string("Note, Reading"))
            input.document.viewOwner = self
            input.document.onStateChange = nil
            input.document.onReload = { [weak self] _ in self?.refresh() }
            input.document.onClosing = { [weak self] in self?.detach() }
            view.onWidthOrThemeChange = { [weak self, weak view] in
                guard let self, let view else { return }
                let dark = view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                if abs(width - max(120, view.bounds.width - 60)) > 0.25 || self.dark != dark { refresh() }
            }
            view.onToggle = { [weak self] ordinal, id in
                guard let self, let index = rendered.firstIndex(where: { $0.ordinal == ordinal }) else { return }
                if rendered[index].collapsed.contains(id) { rendered[index].collapsed.remove(id) }
                else { rendered[index].collapsed.insert(id) }
                input.remember(ordinal, rendered[index].collapsed.sorted()); refresh(force: true)
            }
            if (changed || model?.revision != input.document.revision || size != input.fontSize),
               view.window != nil, view.bounds.width > 180 { refresh() }
        }
        func detach() {
            generation += 1; task?.cancel(); task = nil
            inFlightToken = nil; inFlightKey = nil; refreshQueued = false
            styling.reset()
            view?.mediaViews.forEach { $0.detach(); $0.removeFromSuperview() }
            view?.mediaViews = []; view?.mediaBlocks = []; view?.mediaProjection = []
            if input?.document.viewOwner === self {
                input?.document.onReload = nil; input?.document.onClosing = nil
                input?.document.onStateChange = nil; input?.document.viewOwner = nil
            }
            input = nil; model = nil; ranges = []; fallbacks = []; rendered = []; source = ""
            pendingInitialSource = nil; pendingAnchor = nil
            pendingRevealSource = nil
        }
        func refresh(force: Bool = false) {
            guard let input, let view else { return }
            let width = max(120, view.bounds.width - 60)
            let dark = view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            if force { forcedGeneration &+= 1 }
            let key = RefreshKey(revision: input.document.revision, width: width, dark: dark,
                                 size: input.fontSize, forcedGeneration: forcedGeneration)
            if let inFlightKey {
                guard key != inFlightKey, !refreshQueued else { return }
                refreshQueued = true
                generation += 1
                task?.cancel(); task = nil
                pending = true; view.diagramPending = true
                return
            }

            generation += 1; let token = generation
            inFlightToken = token; inFlightKey = key
            pending = true; view.diagramPending = true
            let began = ProcessInfo.processInfo.systemUptime
            if firstRefreshStartedAt == 0 { firstRefreshStartedAt = began }
            let themeChanged = self.dark != dark || size != input.fontSize
            let oldRendered = rendered
            let oldModel = model
            let modelRevision = model?.revision ?? UInt64.max
            let io = input.document.io
            let queuedAt = ProcessInfo.processInfo.systemUptime
            submittedRequests += 1
            io.submit({ session in
                    let modelStart = ProcessInfo.processInfo.systemUptime
                    let wire = try session.readingModelSince(since: modelRevision)
                    let model: ReadingViewModel
                    if wire.deltaIndex == UInt64.max {
                        model = try ReadingViewModel(wire)
                    } else if let oldModel {
                        model = try oldModel.applying(wire)
                    } else {
                        throw CocoaError(.fileReadCorruptFile)
                    }
                    let modelTime = (ProcessInfo.processInfo.systemUptime - modelStart) * 1000
                    let snapshotStart = ProcessInfo.processInfo.systemUptime
                    let snapshot = model.blocks.contains(where: \.diagram) ? try session.snapshot() : nil
                    let workerEnd = ProcessInfo.processInfo.systemUptime
                    let media = try session.mediaBlocks(width: width, size: input.fontSize)
                    return (model, snapshot, modelTime, (workerEnd - snapshotStart) * 1000,
                            modelStart, workerEnd, media)
                }, completion: { [weak self, weak view] result in
                    guard let self, self.inFlightToken == token else { return }
                    guard let view, let input = self.input else { self.finishRefresh(token); return }
                    guard token == self.generation else { self.finishRefresh(token); return }
                    switch result {
                    case .success(let reading):
                        var phases: [String: Double] = [
                            "model_ffi_ms": reading.2,
                            "snapshot_ms": reading.3,
                            "io_queue_ms": (reading.4 - queuedAt) * 1000,
                            "io_resume_ms": (ProcessInfo.processInfo.systemUptime - reading.5) * 1000,
                        ]
                        let value = ReadingRefreshResult(model: reading.0, snapshot: reading.1, media: [], phases: phases)
                        if !reading.6.isEmpty || reading.1?.blocks.isEmpty == false {
                            self.task = Task { [weak self, weak view, oldRendered] in
                                do {
                                    let diagramStart = ProcessInfo.processInfo.systemUptime
                                    var diagrams: [NoteDiagram] = []
                                    if let snapshot = reading.1, !snapshot.blocks.isEmpty {
                                        diagrams = try await input.service.blocks(snapshot: snapshot, vault: input.vault,
                                                                                  note: input.path, core: input.core)
                                        for i in diagrams.indices {
                                            if let old = oldRendered.first(where: { $0.ordinal == diagrams[i].ordinal }) {
                                                diagrams[i].collapsed = old.collapsed
                                            }
                                        }
                                        diagrams = await input.service.render(source: snapshot.text,
                                            mmd: input.path.lowercased().hasSuffix(".mmd"), blocks: diagrams,
                                            width: width, dark: dark)
                                    }
                                    let media = await LocalImageCache.shared.prepare(reading.6, vault: input.vault)
                                    var completed = ReadingRefreshResult(model: value.model, snapshot: value.snapshot, media: media, phases: value.phases)
                                    completed.phases["diagrams_ms"] = (ProcessInfo.processInfo.systemUptime - diagramStart) * 1000
                                    guard let self else { return }
                                    guard !Task.isCancelled, token == self.generation else {
                                        self.finishRefresh(token)
                                        return
                                    }
                                    self.install(completed, diagrams: diagrams, width: width, dark: dark,
                                                 themeChanged: themeChanged, token: token, began: began)
                                } catch {
                                    guard let self else { return }
                                    if token == self.generation {
                                        self.pending = false; view?.diagramPending = false
                                        view?.diagramIssue = String(describing: error)
                                    }
                                    self.finishRefresh(token)
                                }
                            }
                        } else {
                            phases["diagrams_ms"] = 0
                            self.install(value, diagrams: [], width: width, dark: dark,
                                         themeChanged: themeChanged, token: token, began: began)
                        }
                    case .failure(let error):
                        self.pending = false; view.diagramPending = false
                        view.diagramIssue = String(describing: error)
                        self.finishRefresh(token)
                    }
                }
            )
        }
        private func install(_ result: ReadingRefreshResult, diagrams: [NoteDiagram], width: Double,
                             dark: Bool, themeChanged: Bool, token: Int, began: Double) {
            guard let view, let input = self.input else { finishRefresh(token); return }
            guard token == generation, result.model.revision == input.document.revision else {
                finishRefresh(token)
                refresh()
                return
            }
            self.width = width; self.dark = dark; size = input.fontSize; source = input.document.text
            let installStart = ProcessInfo.processInfo.systemUptime
            install(result.model, diagrams: diagrams, themeChanged: themeChanged)
            let items = result.media.compactMap { item -> NoteMedia? in
                guard let index = result.model.blocks.firstIndex(where: { $0.id == item.block.blockId }), index < ranges.count else { return nil }
                let block = result.model.blocks[index]
                let local = Int(item.block.displayStart - block.displayStart)
                return NoteMedia(range: NSRange(location: ranges[index].location + local, length: Int(item.block.displayLength)), prepared: item)
            }
            view.installMedia(items, width: width, size: input.fontSize, vault: input.vault, refresh: { [weak self] in self?.refresh(force: true) })
            phaseMilliseconds = result.phases
            phaseMilliseconds["install_ms"] = (ProcessInfo.processInfo.systemUptime - installStart) * 1000
            rendered = diagrams; pending = false; view.diagramPending = false
            let layoutStart = ProcessInfo.processInfo.systemUptime
            view.layoutSubtreeIfNeeded()
            if let source = pendingInitialSource {
                pendingInitialSource = nil
                reveal(source: source)
                view.layoutSubtreeIfNeeded()
            } else if let anchor = pendingAnchor {
                pendingAnchor = nil
                restore(anchor)
            }
            phaseMilliseconds["layout_ms"] = (ProcessInfo.processInfo.systemUptime - layoutStart) * 1000
            let displayStart = ProcessInfo.processInfo.systemUptime
            view.displayIfNeeded()
            phaseMilliseconds["display_ms"] = (ProcessInfo.processInfo.systemUptime - displayStart) * 1000
            displayedAt = ProcessInfo.processInfo.systemUptime
            latencyMilliseconds = (displayedAt - began) * 1000
            (NSApp.delegate as? AppDelegate)?.inspectWhenReady()
            finishRefresh(token)
        }

        private func finishRefresh(_ token: Int) {
            guard inFlightToken == token else { return }
            inFlightToken = nil; inFlightKey = nil
            guard refreshQueued else { return }
            refreshQueued = false
            refresh()
        }
        private func install(_ next: ReadingViewModel, diagrams: [NoteDiagram], themeChanged: Bool) {
            guard let view, let storage = view.textStorage, let input else { return }
            let assemblyStart = ProcessInfo.processInfo.systemUptime
            let anchor = visibleAnchor()
            let old = model
            let sourceText = source as NSString
            let hasProperties = next.text.hasPrefix("\u{e000}")
            var fallback = [Bool](repeating: false, count: next.blocks.count)
            var ordinal: UInt32 = 0
            for (index, block) in next.blocks.enumerated() where block.diagram {
                ordinal += 1
                if diagrams.first(where: { $0.ordinal == ordinal })?.prepared == nil {
                    fallback[index] = true
                }
            }
            var mutableDisplayText: NSMutableString?
            var blockLengths = [Int](repeating: 0, count: next.blocks.count)
            var labelDeltas = [Int](repeating: 0, count: next.blocks.count)
            var displayShift = 0
            for (index, block) in next.blocks.enumerated() {
                let originalLength = Int(block.displayLength)
                let displayStart = Int(block.displayStart) + displayShift
                if fallback[index] {
                    let sourceRange = NSRange(location: Int(block.sourceStart), length: Int(block.sourceLength))
                    let replacement = sourceText.substring(with: sourceRange)
                    let mutable: NSMutableString
                    if let existing = mutableDisplayText { mutable = existing }
                    else { mutable = NSMutableString(string: next.text); mutableDisplayText = mutable }
                    mutable.replaceCharacters(in: NSRange(location: displayStart, length: originalLength), with: replacement)
                    blockLengths[index] = (replacement as NSString).length
                } else if hasProperties && block.displayStart == 0 {
                    let label = L10n.string("Properties")
                    labelDeltas[index] = (label as NSString).length - 1
                    let mutable: NSMutableString
                    if let existing = mutableDisplayText { mutable = existing }
                    else { mutable = NSMutableString(string: next.text); mutableDisplayText = mutable }
                    mutable.replaceCharacters(in: NSRange(location: displayStart, length: 1), with: label)
                    blockLengths[index] = originalLength + labelDeltas[index]
                } else {
                    blockLengths[index] = originalLength
                }
                displayShift += blockLengths[index] - originalLength
            }
            var nextRanges: [NSRange] = [], nextDiagrams: [NoteDiagram] = [], runs: [SourceStyleRun] = []
            nextRanges.reserveCapacity(next.blocks.count)
            var offset = 0
            ordinal = 0
            for (index, block) in next.blocks.enumerated() {
                var diagram: NoteDiagram?
                if block.diagram { ordinal += 1; diagram = diagrams.first { $0.ordinal == ordinal } }
                let length = blockLengths[index]
                let range = NSRange(location: offset, length: length)
                nextRanges.append(range)
                if var diagram {
                    diagram.range = range; diagram.tailStart = offset; nextDiagrams.append(diagram)
                }
                if fallback[index] { runs.append(SourceStyleRun(start: UInt64(offset), length: UInt64(length), mask: 32)) }
                else {
                    for runIndex in block.runRange {
                        let run = next.run(at: runIndex)
                        var start = Int(run.start)
                        var runLength = Int(run.length)
                        if labelDeltas[index] != 0 {
                            if start == 0 && run.mask == 16384 {
                                runLength += labelDeltas[index]
                            } else if start >= 1 {
                                start += labelDeltas[index]
                            }
                        }
                        runs.append(SourceStyleRun(start: UInt64(offset + start), length: UInt64(runLength), mask: run.mask))
                    }
                }
                offset += length
            }
            let displayedText = mutableDisplayText.map { $0 as String } ?? next.text
            let assemblyMilliseconds = (ProcessInfo.processInfo.systemUptime - assemblyStart) * 1000
            var prefix = 0, suffix = 0
            if !themeChanged, let old {
                while prefix < min(old.blocks.count, next.blocks.count), old.blocks[prefix].id == next.blocks[prefix].id,
                      self.fallbacks[prefix] == fallback[prefix] { prefix += 1 }
                while suffix < min(old.blocks.count, next.blocks.count) - prefix {
                    let a = old.blocks.count - 1 - suffix, b = next.blocks.count - 1 - suffix
                    if old.blocks[a].id != next.blocks[b].id || self.fallbacks[a] != fallback[b] { break }
                    suffix += 1
                }
            }
            styling.installReading(runs, size: input.fontSize)
            changedCharacters = 0
            if prefix + suffix < next.blocks.count || old?.blocks.count != next.blocks.count || themeChanged {
                let start = old == nil ? 0 : (prefix < ranges.count ? ranges[prefix].location : storage.length)
                let end = suffix > 0 ? ranges[ranges.count - suffix].location : storage.length
                let textLength = (displayedText as NSString).length
                let replacementStart = prefix < nextRanges.count ? nextRanges[prefix].location : textLength
                let replacementEnd = suffix > 0 ? nextRanges[nextRanges.count - suffix].location : textLength
                let replacementText = (displayedText as NSString).substring(with: NSRange(location: replacementStart, length: replacementEnd - replacementStart))
                let replacement = NSMutableAttributedString(string: replacementText, attributes: styling.style(0))
                // 存储只含显示文字；段落被原生视口请求时才附加同一套语义样式。
                changedCharacters = replacement.length
                view.diagramStorage?.performEditingTransaction { storage.replaceCharacters(in: NSRange(location: start, length: end - start), with: replacement) }
            }
            phaseMilliseconds["swift_assembly_ms"] = assemblyMilliseconds
            phaseMilliseconds["text_storage_ms"] = (ProcessInfo.processInfo.systemUptime - assemblyStart) * 1000 - assemblyMilliseconds
            model = next; ranges = nextRanges; fallbacks = fallback; view.sourceUTF16Length = storage.length
            view.install(nextDiagrams)
            view.needsLayout = true
            if initial {
                initial = false; pendingInitialSource = input.document.sourceAnchor
            } else if let anchor {
                pendingAnchor = anchor
            }
        }
        private func restore(_ anchor: (id: UInt64, source: Int, offset: Double)) {
            guard let view, let clip = view.enclosingScrollView?.contentView, let model else { return }
            let index = model.blocks.firstIndex(where: { $0.id == anchor.id })
                ?? model.blocks.firstIndex(where: { Int($0.sourceStart + $0.sourceLength) > anchor.source })
            guard let index else { return }
            for _ in 0..<3 {
                view.layoutSubtreeIfNeeded()
                let target = rect(at: ranges[index].location)
                let delta = target.minY - view.visibleRect.minY - anchor.offset
                if abs(delta) <= 0.25 { break }
                clip.scroll(to: CGPoint(x: clip.bounds.origin.x, y: max(0, clip.bounds.origin.y + delta)))
                view.enclosingScrollView?.reflectScrolledClipView(clip)
            }
        }
        func sourcePosition(at position: Int) -> Int {
            guard let model, let index = ranges.firstIndex(where: { NSLocationInRange(position, $0) }) else { return source.utf16.count }
            let block = model.blocks[index], local = position - ranges[index].location
            guard let run = model.firstRun(in: block.runRange, matching: {
                local >= Int($0.start) && local < Int($0.start + $0.length)
            }) else { return Int(block.sourceStart) }
            return Int(block.sourceStart + run.sourceStart) + (run.sourceLength == run.length ? local - Int(run.start) : 0)
        }
        func displayPosition(source: Int) -> Int {
            guard let model, let index = model.blocks.firstIndex(where: { source < Int($0.sourceStart + $0.sourceLength) }) else { return view?.sourceUTF16Length ?? 0 }
            let block = model.blocks[index], local = source - Int(block.sourceStart)
            let run = model.firstRun(in: block.runRange, matching: {
                local < Int($0.sourceStart + $0.sourceLength)
            })
            let adjustment = run.map { $0.length == $0.sourceLength ? max(0, local - Int($0.sourceStart)) : 0 } ?? 0
            return ranges[index].location + Int(run?.start ?? 0) + adjustment
        }
        func reveal(source: Int) {
            guard let view else { return }
            guard view.window != nil, let clip = view.enclosingScrollView?.contentView else {
                pendingRevealSource = source
                return
            }
            pendingRevealSource = nil
            let position = min(displayPosition(source: source), view.sourceUTF16Length)
            view.revealCharacter(at: position)
            for _ in 0..<3 {
                view.layoutSubtreeIfNeeded()
                let target = rect(at: position)
                let visible = view.visibleRect
                guard !target.isEmpty, !visible.contains(target) else { break }
                let delta = target.minY < visible.minY
                    ? target.minY - visible.minY
                    : target.maxY - visible.maxY
                clip.scroll(to: CGPoint(x: clip.bounds.origin.x, y: max(0, clip.bounds.origin.y + delta)))
                view.enclosingScrollView?.reflectScrolledClipView(clip)
            }
        }
        func didMoveToWindow() {
            guard let view, view.window != nil else { return }
            if model == nil, view.bounds.width > 180 {
                refresh()
                return
            }
            guard pendingRevealSource != nil else { return }
            DispatchQueue.main.async { [weak self] in
                guard let self, let source = self.pendingRevealSource, self.view?.window != nil else { return }
                self.reveal(source: source)
            }
        }
        func rect(at position: Int) -> CGRect {
            guard let view, let manager = view.textLayoutManager, let content = manager.textContentManager,
                  let location = content.location(content.documentRange.location, offsetBy: position) else { return .zero }
            if let end = content.location(location, offsetBy: min(1, view.sourceUTF16Length - position)),
               let range = NSTextRange(location: location, end: end) { manager.ensureLayout(for: range) }
            guard let fragment = manager.textLayoutFragment(for: location) else { return .zero }
            return fragment.layoutFragmentFrame.offsetBy(dx: view.textContainerInset.width, dy: view.textContainerInset.height)
        }
        func visibleAnchor() -> (id: UInt64, source: Int, offset: Double)? {
            guard let view, let model else { return nil }
            let position = view.characterIndexForInsertion(at: CGPoint(x: 31, y: view.visibleRect.minY + 1))
            guard let index = ranges.firstIndex(where: { NSLocationInRange(position, $0) }) else { return nil }
            return (model.blocks[index].id, Int(model.blocks[index].sourceStart), rect(at: ranges[index].location).minY - view.visibleRect.minY)
        }
        func copyMarkdown() {
            guard let view else { return }
            let selected = view.selectedRange()
            guard selected.length > 0, let model,
                  let first = ranges.firstIndex(where: { NSIntersectionRange($0, selected).length > 0 }),
                  let last = ranges.lastIndex(where: { NSIntersectionRange($0, selected).length > 0 }) else { return }
            // 块边界包含语法外壳，复制整个块时仍是合法 Markdown。
            let start = selected.location <= ranges[first].location ? Int(model.blocks[first].sourceStart) : sourcePosition(at: selected.location)
            let end = NSMaxRange(selected) >= NSMaxRange(ranges[last]) ? Int(model.blocks[last].sourceStart + model.blocks[last].sourceLength) : sourceEnd(at: NSMaxRange(selected) - 1)
            let text = source as NSString
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text.substring(with: NSRange(location: min(start, text.length), length: max(0, min(end, text.length) - start))), forType: .string)
        }
        private func sourceEnd(at position: Int) -> Int {
            guard let model, let i = ranges.firstIndex(where: { NSLocationInRange(position, $0) }) else { return source.utf16.count }
            let block = model.blocks[i], local = position - ranges[i].location
            guard let run = model.firstRun(in: block.runRange, matching: {
                local >= Int($0.start) && local < Int($0.start + $0.length)
            }) else { return Int(block.sourceStart + block.sourceLength) }
            return Int(block.sourceStart + run.sourceStart) + (run.sourceLength == run.length ? local - Int(run.start) + 1 : Int(run.sourceLength))
        }
        func clickTask(_ event: NSEvent) -> Bool {
            guard let view, let input, input.document.readOnlyReason == nil, !input.document.dirty, !pending, let model else { return false }
            let position = view.characterIndexForInsertion(at: view.convert(event.locationInWindow, from: nil))
            guard let i = ranges.firstIndex(where: { NSLocationInRange(position, $0) }),
                  let run = model.firstRun(in: model.blocks[i].runRange, matching: {
                      $0.mask & 131072 != 0 && position - ranges[i].location >= Int($0.start)
                          && position - ranges[i].location < Int($0.start + $0.length)
                  }) else { return false }
            let checkRange = NSRange(location: ranges[i].location + Int(run.start), length: 1)
            let rect = view.firstRect(forCharacterRange: checkRange, actualRange: nil)
            guard let window = view.window, rect.insetBy(dx: -2, dy: -2).contains(window.convertPoint(toScreen: event.locationInWindow)) else { return false }
            let sourcePosition = Int(model.blocks[i].sourceStart + run.sourceStart) + 1
            let character = (source as NSString).substring(with: NSRange(location: sourcePosition, length: 1))
            input.document.edited(range: NSRange(location: sourcePosition, length: 1), replacement: character == " " ? "x" : " ")
            Task { await input.document.saveNow(); refresh() }
            return true
        }
        func textLayoutManager(_ manager: NSTextLayoutManager, textLayoutFragmentFor location: any NSTextLocation, in element: NSTextElement) -> NSTextLayoutFragment {
            let fragment = NoteLayoutFragment(textElement: element, range: nil)
            if let content = manager.textContentManager { view?.configure(fragment, at: content.offset(from: content.documentRange.location, to: location)) }
            return fragment
        }
    }
}
