import AppKit
import SwiftUI
import HortusCore
import HortusApple

private struct ReadingRefreshResult: Sendable {
    let model: ReadingViewModel
    let snapshot: DocumentSnapshot?
    let media: [PreparedMedia]
    var math: [PreparedMathBlock] = []
    var knowledge: KnowledgeMarkupInfo? = nil
    var phases: [String: Double]
}

struct ReadingText: NSViewRepresentable {
    var mathService = MathService.shared
    let document: NoteDocument
    let path, vault: String
    let fontSize: Double
    let service: DiagramService
    let core: WorkspaceCore?
    /// 阅读面点击图节点/边时进入编辑视图；由 Workspace 的统一切换队列处理。
    let openSource: (Int, Int) -> Void
    let remember: (UInt32, [String]) -> Void
    var onFocus: ((UInt32) -> Void)? = nil
    var refreshToken: Int = 0
    var knowledgeHandle: VaultHandle? = nil
    var knowledgeVersion = 0
    var openLink: ((String, String?) -> Void)? = nil
    var openResolvedLink: ((String, String?) -> Void)? = nil
    var showTag: ((String) -> Void)? = nil
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = DiagramScrollView()
        scroll.drawsBackground = false; scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        scroll.contentView.postsBoundsChangedNotifications = true
        let view = DiagramTextView(diagramLayout: true)
        view.isReading = true; view.isEditable = false; view.isSelectable = true; view.isRichText = true
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
        (scroll.documentView as? DiagramTextView)?.tearDownForRemoval()
        scroll.documentView = nil
    }

    @MainActor final class Coordinator: NSObject, @preconcurrency NSTextLayoutManagerDelegate {
        private struct RefreshKey: Equatable {
            let revision: UInt64
            let width: Double
            let dark: Bool
            let increasedContrast: Bool
            let size: Double
            let scrollerStyle: Int
            let forcedGeneration: UInt64
            let knowledgeVersion: Int
        }

        weak var view: DiagramTextView?
        var input: ReadingText?
        var model: ReadingViewModel?
        private var preparedMath: [PreparedMathBlock] = []
        var ranges: [NSRange] = []
        private var collapsedSpaceOffsets: [Int: [Int]] = [:]
        var task: Task<Void, Never>?
        private var debounceTask: Task<Void, Never>?
        private var scrollObserver: NSObjectProtocol?
        private var outlineScrollTask: Task<Void, Never>?
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
        private var increasedContrast = false
        private var size = 0.0
        private var scrollerStyle: NSScroller.Style?
        private var source = ""
        private var knowledge: KnowledgeMarkupInfo?
        private var installedKnowledgeVersion = 0
        private var pendingRevealLength = 0
        private var fallbacks: [Bool] = []
        var displayedAt = 0.0
        var firstRefreshStartedAt = 0.0
        var changedCharacters = 0
        var phaseMilliseconds: [String: Double] = [:]
        private var rendered: [NoteDiagram] = []
        private var initial = true
        private var pendingInitialSource: Int?
        private var pendingRevealSource: Int?
        private var revealGeneration = 0
        private var pendingAnchor: (id: UInt64, source: Int, offset: Double)?
        var revealPending: Bool { pendingRevealSource != nil }
        #if DEBUG
        var inspectionSource: String { source }
        var inspectionKnowledge: KnowledgeMarkupInfo? { knowledge }
        #endif

        func update(_ input: ReadingText) {
            let changed = self.input?.document !== input.document
            let tokenChanged = self.input?.refreshToken != input.refreshToken
            let staleText = input.document.textRevision != input.document.revision
            let indexChanged = self.input?.knowledgeVersion != input.knowledgeVersion
                && (staleText || input.document.text.contains("[["))
            if changed { detach(); model = nil; ranges = []; fallbacks = []; rendered = []; initial = true }
            self.input = input
            guard let view else { return }
            view.representedPath = input.path; view.representedDocument = ObjectIdentifier(input.document)
            view.bodyFontSize = input.fontSize
            view.updateColumnInset()
            view.sourceStyling = styling; view.textLayoutManager?.delegate = self
            view.setAccessibilityLabel(L10n.string("Note, Reading"))
            input.document.viewOwner = self
            input.document.onStateChange = nil
            input.document.onReload = { [weak self] _ in self?.refresh() }
            input.document.onClosing = { [weak self] in self?.detach() }
            view.onWidthOrThemeChange = { [weak self, weak view] in
                guard let self, let view else { return }
                let dark = view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                let contrast = view.increasedContrast
                let widthChanged = abs(width - view.layoutContentWidth) > 0.25
                let themeOrScrollerChanged = self.dark != dark || self.increasedContrast != contrast || scrollerStyle != view.formulaScrollerStyle
                let hasEmbeds = view.mediaBlocks.contains { $0.prepared.reason.hasPrefix("embed.") }
                if themeOrScrollerChanged || (widthChanged && (model == nil || !hasEmbeds)) {
                    refresh()
                } else if widthChanged {
                    scheduleDebouncedRefresh()
                }
            }
            view.onFreshRender = { [weak self] in
                guard let self else { return }
                await self.input?.service.discardPrevious()
                for index in self.rendered.indices { self.rendered[index].previous = nil }
                self.refresh(force: true)
            }
            view.onSourceRange = { [weak self, weak view] ordinal, range in
                guard let self, let view, !view.compositionActive, let input = self.input,
                      let block = self.rendered.first(where: { $0.ordinal == ordinal }),
                      block.revision == input.document.revision else { return }
                input.openSource(range.location, range.length)
            }
            view.onToggle = { [weak self] ordinal, id in
                guard let self, let index = rendered.firstIndex(where: { $0.ordinal == ordinal }) else { return }
                if rendered[index].collapsed.contains(id) { rendered[index].collapsed.remove(id) }
                else { rendered[index].collapsed.insert(id) }
                input.remember(ordinal, rendered[index].collapsed.sorted()); refresh(force: true)
            }
            view.onFocus = { [weak self] ordinal in self?.input?.onFocus?(ordinal) }
            view.onOpenLink = { [weak self] _, target, heading in
                self?.input?.openLink?(target, heading)
            }
            view.exportRequestProvider = { [weak self, weak view] ordinal in
                guard let self, let view, let input = self.input,
                      let block = self.rendered.first(where: { $0.ordinal == ordinal }),
                      let prepared = block.prepared, !self.source.isEmpty else { return nil }
                let source = self.source
                let mmd = input.path.lowercased().hasSuffix(".mmd")
                let width = prepared.scene.width
                let dark = view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                let collapsed = block.collapsed.sorted()
                return DiagramExportRequest(prepared: prepared, note: input.path, ordinal: ordinal) {
                    try await DiagramExport.renderSVG(source: source, mmd: mmd, block: ordinal,
                                                      width: width, dark: dark, collapsed: collapsed)
                }
            }
            installScrollObserver(for: view)
            if tokenChanged, view.window != nil, view.hasSettledContentWidth { refresh(force: true) }
            else if (changed || model?.revision != input.document.revision || size != input.fontSize),
               view.window != nil, view.hasSettledContentWidth { refresh() }
            else if indexChanged, view.window != nil, view.hasSettledContentWidth { scheduleDebouncedRefresh() }
        }
        private func installScrollObserver(for view: DiagramTextView) {
            guard scrollObserver == nil, let clip = view.enclosingScrollView?.contentView else { return }
            clip.postsBoundsChangedNotifications = true
            scrollObserver = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: clip, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.scheduleActiveHeadingSync() }
            }
        }
        private func scheduleActiveHeadingSync() {
            guard outlineScrollTask == nil, let input, !input.document.headings.isEmpty else { return }
            outlineScrollTask = Task { @MainActor [weak self] in
                try? await Task.sleep(for: .milliseconds(60))
                guard let self, !Task.isCancelled else { return }
                self.outlineScrollTask = nil
                self.syncActiveHeading()
            }
        }
        func syncActiveHeading() {
            guard let view, let input, !ranges.isEmpty, !input.document.headings.isEmpty else { return }
            let visible = view.visibleRect
            let x = view.textContainerInset.width + 1
            let y = max(visible.minY + 18, view.textContainerInset.height + 1)
            let rawPos = view.characterIndexForInsertion(at: CGPoint(x: x, y: y))
            let position: Int?
            if rawPos != NSNotFound, rawPos >= 0, rawPos < view.sourceUTF16Length {
                position = rawPos
            } else {
                position = visibleInsertionPosition()
            }
            if let position, let sourceOffset = sourcePosition(at: position) {
                input.document.updateActiveHeading(for: sourceOffset)
            }
        }
        func scheduleDebouncedRefresh(delay: Duration = .milliseconds(75)) {
            guard let view, view.hasSettledContentWidth else { return }
            debounceTask?.cancel()
            pending = true
            view.diagramPending = true
            debounceTask = Task { @MainActor [weak self] in
                try? await Task.sleep(for: delay)
                guard !Task.isCancelled, let self else { return }
                self.debounceTask = nil
                self.refresh()
            }
        }
        func detach() {
            generation += 1; task?.cancel(); task = nil
            debounceTask?.cancel(); debounceTask = nil
            outlineScrollTask?.cancel(); outlineScrollTask = nil
            if let scrollObserver { NotificationCenter.default.removeObserver(scrollObserver); self.scrollObserver = nil }
            inFlightToken = nil; inFlightKey = nil; refreshQueued = false
            styling.reset()
            view?.clearAttachedContent()
            input?.document.hasPreparedDiagrams = false
            if input?.document.viewOwner === self {
                input?.document.onReload = nil; input?.document.onClosing = nil
                input?.document.onStateChange = nil; input?.document.viewOwner = nil
            }
            input = nil; model = nil; ranges = []; collapsedSpaceOffsets = [:]; fallbacks = []; rendered = []; source = ""
            preparedMath = []; knowledge = nil
            pendingInitialSource = nil; pendingAnchor = nil
            pendingRevealSource = nil
        }
        func refresh(force: Bool = false) {
            debounceTask?.cancel(); debounceTask = nil
            guard let input, let view else { return }
            let width = view.layoutContentWidth
            guard view.hasSettledContentWidth else { return }
            let dark = view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            let contrast = view.increasedContrast
            if force { forcedGeneration &+= 1 }
            let sourceText = input.document.text
            // 编辑后镜像正文可能落后于 Rust 会话；此时不能按旧正文剪枝知识扫描。
            let staleText = input.document.textRevision != input.document.revision
            let hasLinks = staleText || sourceText.contains("[[")
            let version = hasLinks ? input.knowledgeVersion : 0
            let key = RefreshKey(revision: input.document.revision, width: width, dark: dark, increasedContrast: contrast,
                                 size: input.fontSize, scrollerStyle: view.formulaScrollerStyle.rawValue, forcedGeneration: forcedGeneration, knowledgeVersion: version)
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
            let themeChanged = self.dark != dark || self.increasedContrast != contrast || size != input.fontSize || scrollerStyle != view.formulaScrollerStyle
            let oldRendered = rendered
            let oldMath = size == input.fontSize ? preparedMath : []
            let oldTableMath = size == input.fontSize ? view.mediaBlocks.flatMap { $0.prepared.math.flatMap { $0 } } : []
            let oldModel = model
            let cachedKnowledge = model?.revision == input.document.revision && installedKnowledgeVersion == version ? knowledge : nil
            let handle = input.knowledgeHandle
            let scanKnowledge = staleText || hasLinks || sourceText.contains("#")
            let modelRevision = model?.revision ?? UInt64.max
            let io = input.document.io
            let queuedAt = ProcessInfo.processInfo.systemUptime
            submittedRequests += 1
            view.layoutRefreshCount += 1
            DiagramInspection.recordScrollStability(event: "reading_refresh", view: view,
                                                     details: ["content_width": width])
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
                    let math = try session.mathBlocks()
                    // 镜像正文过时或存在图/公式时，快照提供与修订对齐的 Markdown 原文。
                    let snapshot = staleText || model.blocks.contains(where: \.diagram) || !math.isEmpty
                        ? try session.snapshot() : nil
                    let workerEnd = ProcessInfo.processInfo.systemUptime
                    let media = try session.mediaBlocks(width: width, size: input.fontSize)
                    let knowledge = cachedKnowledge ?? (scanKnowledge
                        ? handle?.knowledgeMarkup(path: input.path, text: snapshot?.text ?? sourceText) : nil)
                    return (model, snapshot, modelTime, (workerEnd - snapshotStart) * 1000,
                            modelStart, workerEnd, media, math, knowledge)
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
                        var value = ReadingRefreshResult(model: reading.0, snapshot: reading.1, media: [], knowledge: reading.8, phases: phases)
                        if !reading.6.isEmpty || !reading.7.isEmpty || reading.1?.blocks.isEmpty == false {
                            self.task = Task { [weak self, weak view, oldRendered] in
                                do {
                                    let diagramStart = ProcessInfo.processInfo.systemUptime
                                    var diagrams: [NoteDiagram] = []
                                    if let snapshot = reading.1, !snapshot.blocks.isEmpty {
                                        diagrams = try await input.service.blocks(snapshot: snapshot, vault: input.vault,
                                                                                  note: input.path, core: input.core, documentIdentity: input.document.diagramIdentity)
                                        for i in diagrams.indices {
                                            if let old = oldRendered.first(where: { $0.ordinal == diagrams[i].ordinal }) {
                                                diagrams[i].inherit(old)
                                            }
                                            if let diagram = view?.diagramViews[diagrams[i].ordinal] {
                                                diagrams[i].viewport = diagram.viewportAnchors()
                                            }
                                        }
                                        diagrams = await input.service.render(source: snapshot.text,
                                            mmd: input.path.lowercased().hasSuffix(".mmd"), blocks: diagrams,
                                            width: width, dark: dark, requestGeneration: UInt64(token), fontGeneration: input.fontSize.bitPattern)
                                    }
                                    let media = await LocalImageCache.shared.prepare(reading.6, vault: input.vault,
                                        size: input.fontSize, mathService: input.mathService, previousMath: oldTableMath)
                                    var completed = ReadingRefreshResult(model: value.model, snapshot: value.snapshot, media: media, knowledge: value.knowledge, phases: value.phases)
                                    if !reading.7.isEmpty {
                                        completed.math = await input.mathService.prepare(reading.7, source: reading.1?.text ?? "", size: input.fontSize, width: width,
                                            displayText: value.model.text, previous: oldMath)
                                    }
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
                            value.phases = phases
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
            // 后台回调并非原生输入事件；及时释放 TextKit 生成的 autoreleased 视口片段。
            autoreleasepool {
                installPrepared(result, diagrams: diagrams, width: width, dark: dark,
                                themeChanged: themeChanged, token: token, began: began)
            }
        }
        private func installPrepared(_ result: ReadingRefreshResult, diagrams: [NoteDiagram], width: Double,
                                     dark: Bool, themeChanged: Bool, token: Int, began: Double) {
            guard let view, let input = self.input else { finishRefresh(token); return }
            guard token == generation, result.model.revision == input.document.revision else {
                finishRefresh(token)
                refresh()
                return
            }
            // 阅读模型的 text 是显示文本；源码、任务与知识偏移必须用同一修订的快照原文。
            let live = result.snapshot?.text ?? input.document.text
            self.width = width; self.dark = dark; self.increasedContrast = view.increasedContrast; size = input.fontSize; source = live
            input.document.adoptReadingText(live, revision: result.model.revision)
            scrollerStyle = view.formulaScrollerStyle
            let installStart = ProcessInfo.processInfo.systemUptime
            let oldUnresolved = unresolvedRanges(knowledge)
            knowledge = result.knowledge
            installedKnowledgeVersion = inFlightKey?.knowledgeVersion ?? 0
            let restyle = oldUnresolved != unresolvedRanges(knowledge) && model?.revision == result.model.revision
            let formulas = install(result.model, diagrams: diagrams, math: result.math, width: width,
                                   themeChanged: themeChanged || restyle)
            let blockIndices = Dictionary(uniqueKeysWithValues: result.model.blocks.enumerated().map { ($0.element.id, $0.offset) })
            let items = result.media.compactMap { item -> NoteMedia? in
                guard let index = blockIndices[item.block.blockId], index < ranges.count else { return nil }
                let block = result.model.blocks[index]
                let rawLocal = Int(item.block.displayStart - block.displayStart)
                let local = rawLocal - (collapsedSpaceOffsets[index]?.filter { $0 < rawLocal }.count ?? 0)
                return NoteMedia(range: NSRange(location: ranges[index].location + local, length: Int(item.block.displayLength)), prepared: item)
            }
            view.installMedia(items, width: width, size: input.fontSize, vault: input.vault,
                              knowledge: knowledge, knowledgeVersion: installedKnowledgeVersion,
                              onOpenLink: { [weak self] target, heading in self?.input?.openLink?(target, heading) },
                              onShowTag: { [weak self] tag in self?.input?.showTag?(tag) },
                              refresh: { [weak self] in self?.refresh(force: true) })
            let embeds = (knowledge?.links ?? []).filter {
                $0.embed && !$0.attachment && !$0.inDiagram
            }
            for (item, media) in zip(items, view.mediaViews) {
                guard item.prepared.block.imageWiki, item.prepared.reason.hasPrefix("embed."),
                      let handle = input.knowledgeHandle,
                      let offset = Self.utf16Offset(byte: Int(item.prepared.block.sourceStart), in: live),
                      let link = embeds.first(where: {
                          UInt64(offset) >= $0.startUtf16 && UInt64(offset) < $0.startUtf16 + $0.lengthUtf16
                      }) else { continue }
                let request = EmbedRequest(source: input.path, target: link.target, heading: link.heading,
                                           alias: link.alias, ancestors: [input.path], depth: 0,
                                           handle: handle, vault: input.vault, width: width, size: input.fontSize,
                                           dark: dark, service: input.service, mathService: input.mathService,
                                           knowledgeVersion: installedKnowledgeVersion)
                if let existing = media.existingEmbed(matching: request.reuseKey) {
                    existing.onOpen = { [weak self] path, heading in self?.input?.openResolvedLink?(path, heading) }
                    existing.onOpenLink = { [weak self] path, heading in self?.input?.openResolvedLink?(path, heading) }
                    existing.onShowTag = { [weak self] tag in self?.input?.showTag?(tag) }
                    continue
                }
                let card = EmbeddedNoteView(request)
                card.onOpen = { [weak self] path, heading in self?.input?.openResolvedLink?(path, heading) }
                card.onOpenLink = { [weak self] path, heading in self?.input?.openResolvedLink?(path, heading) }
                card.onShowTag = { [weak self] tag in self?.input?.showTag?(tag) }
                media.installEmbed(card)
            }
            view.installMath(formulas, width: width, size: input.fontSize)
            view.installFormulaAccessibility(formulas)
            view.readingFinderDidUpdateContent()
            preparedMath = result.math
            phaseMilliseconds = result.phases
            phaseMilliseconds["install_ms"] = (ProcessInfo.processInfo.systemUptime - installStart) * 1000
            rendered = diagrams; pending = false; view.diagramPending = false
            view.layoutRenderCount += 1
            DiagramInspection.recordScrollStability(event: "reading_render", view: view)
            let layoutStart = ProcessInfo.processInfo.systemUptime
            if let pending = pendingRevealSource {
                // 模型就绪前到达的更新 reveal 优先于初始锚点，较早的请求不能覆盖它。
                pendingRevealSource = nil
                pendingInitialSource = nil
                // 先让滚动容器按新内容增高，再迁移视口；否则文末 reveal 没有可滚动距离。
                view.layoutSubtreeIfNeeded()
                reveal(source: pending, length: pendingRevealLength)
            } else if let source = pendingInitialSource {
                pendingInitialSource = nil
                // 新视图已在文首；避免为零锚点重复迁移视口、重排首屏。
                view.layoutSubtreeIfNeeded()
                let length = input.document.sourceSelectionLength
                if source != 0 || length > 0 || view.visibleRect.minY > 0.5 { reveal(source: source, length: length) }
                if AnchorInspection.isActive {
                    var details = AnchorInspection.snapshot(view, document: input.document)
                    details["source"] = source
                    details["display"] = displayPosition(source: source)
                    details["eof_target"] = source >= input.document.text.utf16.count
                    AnchorInspection.record("initial_reveal", details)
                }
            } else {
                view.layoutSubtreeIfNeeded()
                if let anchor = pendingAnchor {
                    pendingAnchor = nil
                    restore(anchor)
                }
            }
            phaseMilliseconds["layout_ms"] = (ProcessInfo.processInfo.systemUptime - layoutStart) * 1000
            let displayStart = ProcessInfo.processInfo.systemUptime
            view.displayIfNeeded()
            phaseMilliseconds["display_ms"] = (ProcessInfo.processInfo.systemUptime - displayStart) * 1000
            phaseMilliseconds["main_block_ms"] = (ProcessInfo.processInfo.systemUptime - installStart) * 1000
            displayedAt = ProcessInfo.processInfo.systemUptime
            latencyMilliseconds = (displayedAt - began) * 1000
            syncActiveHeading()
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
        private func install(_ next: ReadingViewModel, diagrams: [NoteDiagram], math: [PreparedMathBlock], width: Double, themeChanged: Bool) -> [NoteMath] {
            guard let view, let storage = view.textStorage, let input else { return [] }
            let assemblyStart = ProcessInfo.processInfo.systemUptime
            let anchor = visibleAnchor()
            let old = model
            let hasProperties = next.text.hasPrefix("\u{e000}")
            var fallback = [Bool](repeating: false, count: next.blocks.count)
            var hasFallback = false
            var ordinal: UInt32 = 0
            for (index, block) in next.blocks.enumerated() where block.diagram {
                ordinal += 1
                if diagrams.first(where: { $0.ordinal == ordinal })?.prepared == nil {
                    fallback[index] = true
                    hasFallback = true
                }
            }
            var mutableDisplayText: NSMutableString?
            var blockLengths = [Int](repeating: 0, count: next.blocks.count)
            var labelDeltas = [Int](repeating: 0, count: next.blocks.count)
            var nextCollapsedSpaces: [Int: [Int]] = [:]
            var displayShift = 0
            let mayHaveInlineComments: Bool = {
                let utf8 = source.utf8
                var prev: UInt8 = 0
                for byte in utf8 {
                    if (prev == 0x25 && byte == 0x25) || (prev == 0x3C && byte == 0x21) { return true }
                    prev = byte
                }
                return false
            }()
            let sourceText: NSString = (hasFallback || mayHaveInlineComments) ? (source as NSString) : ""
            let nextNSString: NSString? = mayHaveInlineComments ? (next.text as NSString) : nil
            for (index, block) in next.blocks.enumerated() {
                let originalLength = Int(block.displayLength)
                let displayStart = Int(block.displayStart) + displayShift
                if fallback[index] {
                    let sourceRange = NSRange(location: Int(block.sourceStart), length: Int(block.sourceLength))
                    // 旧正文或损坏的块范围不能越过当前权威正文，宁可显示空块也不能抛 NSRangeException。
                    let replacement = NSMaxRange(sourceRange) <= sourceText.length
                        ? sourceText.substring(with: sourceRange) : ""
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
                    var removedLocal: [Int] = []
                    if let nextNSString, !block.diagram && block.runRange.count >= 2 {
                        let firstRun = next.run(at: block.runRange.lowerBound)
                        if firstRun.mask & 32 == 0 {
                            var prev = firstRun
                            for runIndex in (block.runRange.lowerBound + 1)..<block.runRange.upperBound {
                                let curr = next.run(at: runIndex)
                                let prevEnd = Int(block.displayStart + prev.start + prev.length)
                                let currStart = Int(block.displayStart + curr.start)
                                if prev.length > 0 && curr.length > 0 && prevEnd == currStart
                                    && prevEnd > 0 && currStart < nextNSString.length
                                    && nextNSString.character(at: prevEnd - 1) == 0x20
                                    && nextNSString.character(at: currStart) == 0x20 {
                                    let gapStart = Int(block.sourceStart + prev.sourceStart + prev.sourceLength)
                                    let gapEnd = Int(block.sourceStart + curr.sourceStart)
                                    if gapEnd > gapStart && gapEnd <= sourceText.length {
                                        let gap = sourceText.substring(with: NSRange(location: gapStart, length: gapEnd - gapStart))
                                        if gap.contains("%%") || gap.contains("<!--") {
                                            removedLocal.append(Int(curr.start))
                                        }
                                    }
                                }
                                prev = curr
                            }
                        }
                    }
                    if !removedLocal.isEmpty {
                        let mutable: NSMutableString
                        if let existing = mutableDisplayText { mutable = existing }
                        else { mutable = NSMutableString(string: next.text); mutableDisplayText = mutable }
                        for localOffset in removedLocal.reversed() {
                            let pos = displayStart + localOffset
                            if pos >= 0 && pos < mutable.length {
                                mutable.deleteCharacters(in: NSRange(location: pos, length: 1))
                            }
                        }
                        nextCollapsedSpaces[index] = removedLocal
                        blockLengths[index] = originalLength - removedLocal.count
                    } else {
                        blockLengths[index] = originalLength
                    }
                }
                displayShift += blockLengths[index] - originalLength
            }
            let unresolved = unresolvedRanges(knowledge)
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
                        } else if !nextCollapsedSpaces.isEmpty, let removed = nextCollapsedSpaces[index] {
                            let removedBefore = removed.filter { $0 < start }.count
                            let removedInside = removed.filter { $0 >= start && $0 < start + runLength }.count
                            start -= removedBefore
                            runLength = max(0, runLength - removedInside)
                        }
                        if runLength == 0 { continue }
                        let sourceStart = Int(block.sourceStart + run.sourceStart)
                        var mask = run.mask
                        if mask & (1 << 18) != 0, let index = unresolved.firstIndex(endingAfter: sourceStart),
                           unresolved[index].contains(sourceStart) { mask |= 1 << 19 }
                        runs.append(SourceStyleRun(start: UInt64(offset + start), length: UInt64(runLength), mask: mask))
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
            let indices = Dictionary(uniqueKeysWithValues: next.blocks.enumerated().map { ($0.element.id, $0.offset) })
            let formulas = math.compactMap { item -> NoteMath? in
                guard let index = indices[item.block.blockId] else { return nil }
                let rawLocal = Int(item.block.displayStart - next.blocks[index].displayStart)
                let local = rawLocal - (nextCollapsedSpaces[index]?.filter { $0 < rawLocal }.count ?? 0)
                return NoteMath(range: NSRange(location: nextRanges[index].location + local, length: Int(item.block.displayLength)),
                                prepared: item, offset: local)
            }
            let replacedStart = prefix < ranges.count ? ranges[prefix].location : storage.length
            let replacedEnd = suffix > 0 ? ranges[ranges.count - suffix].location : storage.length
            view.installInlineMath(formulas.filter { !$0.prepared.block.block }, width: width, size: input.fontSize,
                replacing: NSRange(location: replacedStart, length: replacedEnd - replacedStart))
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
            view.invalidateInlineMath()
            phaseMilliseconds["swift_assembly_ms"] = assemblyMilliseconds
            phaseMilliseconds["text_storage_ms"] = (ProcessInfo.processInfo.systemUptime - assemblyStart) * 1000 - assemblyMilliseconds
            model = next; ranges = nextRanges; collapsedSpaceOffsets = nextCollapsedSpaces; fallbacks = fallback; view.sourceUTF16Length = storage.length
            view.install(nextDiagrams)
            input.document.hasPreparedDiagrams = view.blocks.contains { $0.prepared != nil }
            view.needsLayout = true
            if initial {
                initial = false; pendingInitialSource = input.document.sourceAnchor
            } else if let anchor {
                pendingAnchor = anchor
            }
            return formulas
        }
        func restore(_ anchor: (id: UInt64, source: Int, offset: Double)) {
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
            if AnchorInspection.isActive {
                var details = AnchorInspection.snapshot(view, document: input?.document)
                details["restore_block"] = String(describing: anchor.id)
                details["restore_source"] = anchor.source
                details["restore_offset"] = anchor.offset
                AnchorInspection.record("restore", details)
            }
        }
        var sourceLength: Int { source.utf16.count }

        /// 文末只在滚动视口确实到达底部时才是有效锚点；首次打开时视图高度尚小于内容高度。
        func atDocumentBottom() -> Bool {
            guard let view, view.visibleRect.minY > 0.5 else { return false }
            let content = max(view.frame.height,
                              (view.textLayoutManager?.usageBoundsForTextContainer.maxY ?? 0) + view.textContainerInset.height * 2)
            return view.visibleRect.maxY >= content - view.textContainerInset.height - 0.5
        }

        /// 命中测试在顶部内边距或图块上可能返回文末；先换到正文内的点，再用实际片段校验。
        func visibleInsertionPosition() -> Int? {
            guard let view, !ranges.isEmpty else { return nil }
            let visible = view.visibleRect
            let x = view.textContainerInset.width + 1
            let candidates = [max(visible.minY + 1, view.textContainerInset.height + 1),
                              visible.minY + 1]
            for y in candidates {
                let position = view.characterIndexForInsertion(at: CGPoint(x: x, y: y))
                guard position != NSNotFound, position >= 0, position < view.sourceUTF16Length,
                      ranges.contains(where: { NSLocationInRange(position, $0) }) else { continue }
                let target = rect(at: position)
                if target.maxY > visible.minY + 0.5, target.minY < visible.maxY - 0.5 { return position }
            }
            // 命中测试仍不可信时，退到与视口顶部相交的块起点。
            let content = max(view.frame.height,
                              (view.textLayoutManager?.usageBoundsForTextContainer.maxY ?? 0) + view.textContainerInset.height * 2)
            for index in ranges.indices {
                let top = rect(at: ranges[index].location).minY
                let bottom = index + 1 < ranges.count ? rect(at: ranges[index + 1].location).minY : content
                if bottom > visible.minY + 0.5, top < visible.maxY - 0.5 { return ranges[index].location }
            }
            return nil
        }

        func sourcePosition(at position: Int) -> Int? {
            guard let model, !ranges.isEmpty, let view,
                  position != NSNotFound, position >= 0, position <= view.sourceUTF16Length else { return nil }
            if let index = ranges.firstIndex(where: { NSLocationInRange(position, $0) }) {
                let block = model.blocks[index]
                var local = position - ranges[index].location
                if let removed = collapsedSpaceOffsets[index] {
                    for offset in removed where offset <= local { local += 1 }
                }
                guard let run = model.firstRun(in: block.runRange, matching: {
                    local >= Int($0.start) && local < Int($0.start + $0.length)
                }) else { return Int(block.sourceStart) }
                return Int(block.sourceStart + run.sourceStart) + (run.sourceLength == run.length ? local - Int(run.start) : 0)
            }
            // 真正的文末插入点只在视口位于底部时保留；其余越界位置不能解释为 EOF。
            if position == view.sourceUTF16Length, atDocumentBottom() { return sourceLength }
            return nil
        }
        func displayPosition(source: Int) -> Int {
            guard let view else { return 0 }
            guard let model, source >= 0, source <= sourceLength else { return 0 }
            if source == sourceLength { return view.sourceUTF16Length }
            guard let index = model.blocks.firstIndex(where: { source < Int($0.sourceStart + $0.sourceLength) })
            else { return view.sourceUTF16Length }
            let block = model.blocks[index], local = max(0, source - Int(block.sourceStart))
            let run = model.firstRun(in: block.runRange, matching: {
                local < Int($0.sourceStart + $0.sourceLength)
            })
            let adjustment = run.map { $0.length == $0.sourceLength ? max(0, local - Int($0.sourceStart)) : 0 } ?? 0
            var displayLocal = Int(run?.start ?? 0) + adjustment
            if let removed = collapsedSpaceOffsets[index] {
                displayLocal -= removed.filter { $0 < displayLocal }.count
            }
            return min(view.sourceUTF16Length, ranges[index].location + max(0, displayLocal))
        }
        func reveal(source: Int, length: Int = 0) {
            guard let view else { return }
            revealGeneration += 1
            let request = revealGeneration
            let inspect = AnchorInspection.isActive
            let before = inspect ? AnchorInspection.snapshot(view, document: input?.document) : [:]
            guard model != nil, !ranges.isEmpty else {
                pendingRevealSource = source
                pendingRevealLength = length
                if inspect {
                    var details = before
                    details["source"] = source
                    details["model_ready"] = false
                    AnchorInspection.record("reveal_pending", details)
                }
                return
            }
            guard view.window != nil, let clip = view.enclosingScrollView?.contentView else {
                pendingRevealSource = source
                pendingRevealLength = length
                if inspect {
                    var details = before
                    details["source"] = source
                    details["window"] = false
                    AnchorInspection.record("reveal_pending", details)
                }
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
            if length > 0 {
                let end = min(view.sourceUTF16Length, max(position + 1, displayPosition(source: source + length)))
                if end > position { view.showFindIndicator(for: NSRange(location: position, length: end - position)) }
            }
            input?.document.updateActiveHeading(for: source)
            if inspect {
                var details = AnchorInspection.snapshot(view, document: input?.document)
                details["source"] = source
                details["display"] = position
                details["before_visible"] = before["visible"] ?? NSNull()
                details["eof_target"] = position >= view.sourceUTF16Length
                details["request"] = request
                AnchorInspection.record("reveal", details)
            }
        }
        func didMoveToWindow() {
            guard let view, view.window != nil else { return }
            if model == nil, view.hasSettledContentWidth {
                refresh()
                return
            }
            guard pendingRevealSource != nil else { return }
            let request = revealGeneration
            DispatchQueue.main.async { [weak self] in
                guard let self, self.revealGeneration == request,
                      let source = self.pendingRevealSource, self.view?.window != nil else { return }
                self.reveal(source: source, length: self.pendingRevealLength)
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
            guard let view, let model, let position = visibleInsertionPosition(),
                  let index = ranges.firstIndex(where: { NSLocationInRange(position, $0) }) else { return nil }
            return (model.blocks[index].id, Int(model.blocks[index].sourceStart), rect(at: ranges[index].location).minY - view.visibleRect.minY)
        }
        func copyMarkdown() {
            guard let view else { return }
            let selected = view.selectedRange()
            if let math = view.mathViews.first(where: { $0.item.range == selected }) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(math.item.prepared.source, forType: .string)
                return
            }
            guard selected.length > 0, let model,
                  let first = ranges.firstIndex(where: { NSIntersectionRange($0, selected).length > 0 }),
                  let last = ranges.lastIndex(where: { NSIntersectionRange($0, selected).length > 0 }) else { return }
            // 块边界包含语法外壳，复制整个块时仍是合法 Markdown。
            let start = selected.location <= ranges[first].location ? Int(model.blocks[first].sourceStart)
                : (sourcePosition(at: selected.location) ?? Int(model.blocks[first].sourceStart))
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
        private func unresolvedRanges(_ knowledge: KnowledgeMarkupInfo?) -> [Range<Int>] {
            knowledge?.links.compactMap {
                guard !$0.inDiagram, !$0.attachment, $0.resolvedPath == nil else { return nil }
                let start = Int($0.startUtf16)
                return start..<(start + Int($0.lengthUtf16))
            } ?? []
        }

        static func utf16Offset(byte: Int, in text: String) -> Int? {
            guard byte >= 0, byte <= text.utf8.count else { return nil }
            let utf8 = text.utf8
            guard let end = utf8.index(utf8.startIndex, offsetBy: byte, limitedBy: utf8.endIndex) else { return nil }
            return String(decoding: utf8[utf8.startIndex..<end], as: UTF8.self).utf16.count
        }

        func clickKnowledge(_ event: NSEvent) -> Bool {
            guard let view, let input, !pending, let knowledge,
                  model?.revision == input.document.revision,
                  let position = view.knowledgeCharacter(at: event),
                  let source = sourcePosition(at: position) else { return false }
            let mask = styling.mask(at: position)
            if mask & (1 << 18) != 0, let link = knowledge.links.first(where: {
                !$0.inDiagram && !$0.attachment && UInt64(source) >= $0.startUtf16
                    && UInt64(source) < $0.startUtf16 + $0.lengthUtf16
            }) {
                input.openLink?(link.target, link.heading)
                return true
            }
            if mask & 512 != 0, let tag = knowledge.tags.first(where: {
                UInt64(source) >= $0.startUtf16 && UInt64(source) < $0.startUtf16 + $0.lengthUtf16
            }) {
                input.showTag?(tag.tag)
                return true
            }
            return false
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
            let sourceText = source as NSString
            guard sourcePosition >= 0, sourcePosition < sourceText.length else { return false }
            let character = sourceText.substring(with: NSRange(location: sourcePosition, length: 1))
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

private extension Array where Element == Range<Int> {
    func firstIndex(endingAfter position: Int) -> Int? {
        var low = 0, high = count
        while low < high {
            let mid = low + (high - low) / 2
            if self[mid].upperBound <= position { low = mid + 1 }
            else { high = mid }
        }
        return low < count ? low : nil
    }
}
