// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import io.github.groundgrounder.hortus.AndroidFonts
import io.github.groundgrounder.hortus.CoreEnvironment
import io.github.groundgrounder.hortus.core.DiagramEngine
import io.github.groundgrounder.hortus.core.DiagramFailure
import io.github.groundgrounder.hortus.core.DocumentOutline
import io.github.groundgrounder.hortus.core.DocumentSession
import io.github.groundgrounder.hortus.core.EmbedContentInfo
import io.github.groundgrounder.hortus.core.KnowledgeMarkupInfo
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MathStyle
import io.github.groundgrounder.hortus.core.MediaBlock
import io.github.groundgrounder.hortus.core.ReadingModel
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.TreeBatch
import io.github.groundgrounder.hortus.core.TreeEntry
import io.github.groundgrounder.hortus.core.VaultHandle
import io.github.groundgrounder.hortus.core.WorkspaceCore
import io.github.groundgrounder.hortus.core.diagramBlocks
import io.github.groundgrounder.hortus.core.mathTypographyScale
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.cancel
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

internal data class NoteContent(
    val vaultPath: String,
    val path: String,
    val revision: Long,
    val model: DecodedReadingModel,
    val items: List<ReaderItem>,
    val media: List<MediaBlock>,
    val math: List<MathBlock>,
    val knowledge: KnowledgeMarkupInfo,
    val sourceText: String,
    val outline: DocumentOutline,
    val readOnlyReason: String?,
    val diagramBlocks: List<io.github.groundgrounder.hortus.core.DiagramBlock>,
    val width: Int,
    val bodySize: Float,
    val isMmd: Boolean,
) {
    val title: String = run {
        val heading = outline.headings.firstOrNull { it.level.toInt() == 1 }?.title
            ?: outline.headings.firstOrNull()?.title
        heading?.trim()?.takeIf { it.isNotEmpty() } ?: path.substringAfterLast('/').substringBeforeLast('.')
    }
    val diagramItems: List<DiagramItem> = items.filterIsInstance<DiagramItem>()
}

internal data class DiagramRenderResult(
    val scene: SceneData?,
    val failure: String?,
    val identity: io.github.groundgrounder.hortus.core.DiagramIdentity?,
    val counts: SceneCounts?,
    val width: Double,
    val height: Double,
)

internal object DiagramInspection {
    private val lock = Any()
    val total = SceneCounts()
    private val records = LinkedHashMap<String, org.json.JSONObject>()

    fun record(key: String, scene: SceneData, counts: SceneCounts, dark: Boolean, collapsed: List<String>) {
        synchronized(lock) {
            total.merge(counts)
            records[key] = org.json.JSONObject()
                .put("key", key)
                .put("width", scene.width)
                .put("height", scene.height)
                .put("dark", dark)
                .put("collapsed", org.json.JSONArray(collapsed))
                .put("primitives", scene.primitives.size)
                .put("hits", scene.hitRegions.size)
                .put("plusLabels", scene.primitives.count {
                    it is io.github.groundgrounder.hortus.core.ScenePrimitive.Text &&
                        it.span.text.trim().startsWith("+")
                })
                .put("counts", counts.toJson())
            while (records.size > 240) {
                val first = records.keys.first()
                records.remove(first)
            }
        }
    }

    fun snapshot(): org.json.JSONArray = synchronized(lock) {
        org.json.JSONArray().also { array ->
            for (value in records.values) array.put(value)
        }
    }

    fun reset() = synchronized(lock) {
        records.clear()
        val empty = SceneCounts()
        total.roundedRect = empty.roundedRect
        total.line = empty.line
        total.graphPath = empty.graphPath
        total.graphShape = empty.graphShape
        total.labelBackground = empty.labelBackground
        total.text = empty.text
        total.arrow = empty.arrow
        total.circle = empty.circle
        total.cross = empty.cross
        total.solid = empty.solid
        total.dotted = empty.dotted
        total.thick = empty.thick
        total.otherMarker = empty.otherMarker
        total.otherLine = empty.otherLine
        total.textSamples = empty.textSamples
        total.maxTextWidthError = empty.maxTextWidthError
        total.worstText = ""
        total.worstTextDiagram = ""
    }
}

internal class ReaderRepository(context: Context) : AutoCloseable {
    private val appContext = context.applicationContext
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val lock = Mutex()
    private var workspace: WorkspaceCore? = null
    private var vault: VaultHandle? = null
    private var document: DocumentSession? = null
    private var vaultPath: String? = null
    private var notePath: String? = null
    private var scanJob: Job? = null
    private var scanReady = CompletableDeferred<Unit>()
    private val entries = LinkedHashMap<String, TreeEntry>()
    private val _tree = MutableStateFlow<List<TreeEntry>>(emptyList())
    val tree: StateFlow<List<TreeEntry>> = _tree.asStateFlow()
    private val _externalChanges = MutableSharedFlow<String>(extraBufferCapacity = 4)
    val externalChanges: SharedFlow<String> = _externalChanges.asSharedFlow()

    private var diagramEngine: DiagramEngine? = null
    private var mathRenderer: MathSceneRenderer? = null
    private val requestGeneration = AtomicLong()
    private val diagramCache = object : LinkedHashMap<String, DiagramRenderResult>(48, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, DiagramRenderResult>): Boolean =
            size > 24
    }
    private val embedCache = object : LinkedHashMap<String, EmbeddedContent>(24, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, EmbeddedContent>): Boolean =
            size > 16
    }
    private val fontPaths: List<String> by lazy {
        AndroidFonts.discover().mapNotNull { it.file?.path }.distinct()
    }

    val vaultRoot: String? get() = vaultPath
    val currentPath: String? get() = notePath

    private fun requireWorkspace(): WorkspaceCore = workspace ?: throw IllegalStateException("workspace not opened")

    suspend fun openVault(path: String) = lock.withLock {
        if (vaultPath == path && vault != null) return@withLock
        closeDocument()
        stopScan()
        vault?.stopWatching()
        vault?.close()
        vault = null
        vaultPath = null
        entries.clear()
        _tree.value = emptyList()
        val core = workspace ?: CoreEnvironment.createWorkspaceCore(appContext).also { workspace = it }
        val handle = core.openVault(path)
        vault = handle
        vaultPath = path
        scanReady = CompletableDeferred()
        scanJob = scope.launch { scan(handle) }
    }

    private fun scan(handle: VaultHandle) {
        try {
            while (true) {
                val batches: List<TreeBatch> = MainThreadCounters.record("vaultScan") {
                    handle.waitForBatches()
                }
                if (batches.isEmpty()) continue
                var changed = false
                for (batch in batches) {
                    if (batch.upserted.isNotEmpty() || batch.removed.isNotEmpty()) {
                        handle.updateSearchIndex(
                            batch.upserted.map { it.path },
                            batch.removed,
                        )
                    }
                    for (entry in batch.upserted) {
                        entries[entry.path] = entry
                        changed = true
                    }
                    for (removed in batch.removed) {
                        entries.remove(removed)
                        changed = true
                    }
                    if (batch.scanComplete) scanReady.complete(Unit)
                    val current = notePath
                    if (current != null && (batch.removed.contains(current) ||
                            batch.upserted.any { it.path == current })
                    ) {
                        _externalChanges.tryEmit(current)
                    }
                }
                if (changed) {
                    _tree.value = entries.values.sortedWith(
                        compareBy<TreeEntry> { !it.directory }
                            .thenBy { it.path.lowercase() },
                    )
                }
            }
        } catch (_: Exception) {
            // 关闭库或进程退出时阻塞调用会失败；扫描线程不再有用。
        }
    }

    suspend fun awaitScan() {
        withTimeoutOrNull(15_000) { scanReady.await() }
    }

    private fun closeDocument() {
        document?.close()
        document = null
        notePath = null
    }

    private fun stopScan() {
        scanJob?.cancel()
        scanJob = null
    }

    private fun ensureDiagramEngine(): DiagramEngine {
        diagramEngine?.let { return it }
        val engine = MainThreadCounters.record("diagramRender") {
            DiagramEngine.withFontPaths(fontPaths)
        }
        diagramEngine = engine
        return engine
    }

    private fun ensureMathRenderer(): MathSceneRenderer {
        mathRenderer?.let { return it }
        val engine = MainThreadCounters.record("mathRender") {
            io.github.groundgrounder.hortus.core.MathEngine()
        }
        val renderer = MathSceneRenderer(appContext.assets, engine)
        mathRenderer = renderer
        return renderer
    }

    suspend fun openNote(
        path: String,
        width: Int,
        bodySize: Float,
    ): NoteContent = lock.withLock {
        val root = vaultPath ?: throw IllegalStateException("vault not opened")
        awaitScan()
        closeDocument()
        val core = requireWorkspace()
        val session = try {
            core.openDocument(root, path)
        } catch (error: Exception) {
            throw error
        }
        document = session
        notePath = path
        val content = buildContent(session, root, path, width, bodySize)
        content
    }

    private fun buildContent(
        session: DocumentSession,
        root: String,
        path: String,
        width: Int,
        bodySize: Float,
    ): NoteContent {
        val initial = session.initial()
        val wire: ReadingModel = MainThreadCounters.record("readingModel") {
            session.readingModel()
        }
        val model = ReadingModelDecoder.decodeFull(wire)
        val snapshot = MainThreadCounters.record("readingModel") { session.snapshot() }
        val math = MainThreadCounters.record("readingModel") { session.mathBlocks() }
        val media = MainThreadCounters.record("mediaBlocks") {
            session.mediaBlocks(width.toDouble(), bodySize.toDouble())
        }
        val knowledge = MainThreadCounters.record("readingModel") {
            vault?.knowledgeMarkup(path, snapshot.text)
                ?: KnowledgeMarkupInfo(emptyList(), emptyList())
        }
        val outline = MainThreadCounters.record("readingModel") { session.outline() }
        val diagrams = MainThreadCounters.record("readingModel") { diagramBlocks(snapshot.text) }
        val isMmd = path.lowercase().endsWith(".mmd")
        val items = buildReaderItems(
            model = model,
            sourceText = snapshot.text,
            media = media,
            math = math,
            knowledge = knowledge,
            diagramBlocks = diagrams,
            documentKey = path,
            documentIsMmd = isMmd,
        )
        return NoteContent(
            vaultPath = root,
            path = path,
            revision = model.revision,
            model = model,
            items = items,
            media = media,
            math = math,
            knowledge = knowledge,
            sourceText = snapshot.text,
            outline = outline,
            readOnlyReason = initial.readOnlyReason,
            diagramBlocks = diagrams,
            width = width,
            bodySize = bodySize,
            isMmd = isMmd,
        )
    }

    suspend fun rebuildForWidth(content: NoteContent, width: Int, bodySize: Float): NoteContent =
        lock.withLock {
            if (content.width == width && content.bodySize == bodySize) return@withLock content
            val media = MainThreadCounters.record("mediaBlocks") {
                document?.mediaBlocks(width.toDouble(), bodySize.toDouble()) ?: content.media
            }
            val items = buildReaderItems(
                model = content.model,
                sourceText = content.sourceText,
                media = media,
                math = content.math,
                knowledge = content.knowledge,
                diagramBlocks = content.diagramBlocks,
                documentKey = content.path,
                documentIsMmd = content.isMmd,
            )
            content.copy(
                items = items,
                media = media,
                width = width,
                bodySize = bodySize,
            )
        }

    suspend fun refresh(content: NoteContent, width: Int, bodySize: Float): NoteContent? = lock.withLock {
        val session = document ?: return@withLock null
        if (notePath != content.path) return@withLock null
        val update = MainThreadCounters.record("readingModel") { session.refresh() }
        var revised = update.revision.toLong()
        if (update.text != null) {
            MainThreadCounters.record("readingModel") { session.acceptExternal(update.revision) }
        }
        if (revised == content.revision && update.text == null) return@withLock null
        var wire = MainThreadCounters.record("readingModel") {
            session.readingModelSince(content.revision.toULong())
        }
        val model = try {
            when {
                ReadingModelDecoder.isFull(wire) -> ReadingModelDecoder.decodeFull(wire)
                ReadingModelDecoder.isEmptyDelta(wire) -> content.model.copy(revision = wire.revision.toLong(), rebuilt = wire.rebuilt.toLong())
                else -> ReadingModelDecoder.applyDelta(content.model, wire)
            }
        } catch (_: Exception) {
            wire = MainThreadCounters.record("readingModel") { session.readingModel() }
            ReadingModelDecoder.decodeFull(wire)
        }
        val snapshot = MainThreadCounters.record("readingModel") { session.snapshot() }
        val math = MainThreadCounters.record("readingModel") { session.mathBlocks() }
        val media = MainThreadCounters.record("mediaBlocks") {
            session.mediaBlocks(width.toDouble(), bodySize.toDouble())
        }
        val knowledge = MainThreadCounters.record("readingModel") {
            vault?.knowledgeMarkup(content.path, snapshot.text)
                ?: KnowledgeMarkupInfo(emptyList(), emptyList())
        }
        val outline = MainThreadCounters.record("readingModel") { session.outline() }
        val diagrams = MainThreadCounters.record("readingModel") { diagramBlocks(snapshot.text) }
        val items = buildReaderItems(
            model = model,
            sourceText = snapshot.text,
            media = media,
            math = math,
            knowledge = knowledge,
            diagramBlocks = diagrams,
            documentKey = content.path,
            documentIsMmd = content.isMmd,
        )
        content.copy(
            revision = model.revision,
            model = model,
            items = items,
            media = media,
            math = math,
            knowledge = knowledge,
            sourceText = snapshot.text,
            outline = outline,
            diagramBlocks = diagrams,
            width = width,
            bodySize = bodySize,
        )
    }

    suspend fun prepareMath(formulas: List<MathBlock>, bodySize: Float): List<MathRenderResult> = lock.withLock {
        if (formulas.isEmpty()) return@withLock emptyList()
        val renderer = ensureMathRenderer()
        val em = (bodySize * mathTypographyScale()).toFloat()
        val note = notePath ?: ""
        formulas.map { formula ->
            val result = if (formula.latex.isBlank()) {
                MathRenderResult(null, null, 0)
            } else {
                renderer.render(formula.latex, formula.style, em)
            }
            Inspection.recordMath(note, formula, result, bodySize)
            result
        }
    }

    suspend fun renderDiagram(
        documentKey: String,
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        revision: Long,
    ): DiagramRenderResult = lock.withLock {
        val key = "${engineText.hashCode()}:${engineText.length}:$mmd:$ordinal:${width.toInt()}:$dark:${collapsed.sorted()}:$documentKey:$revision"
        synchronized(diagramCache) {
            diagramCache[key]?.let { return@withLock it }
        }
        val engine = ensureDiagramEngine()
        val request = requestGeneration.incrementAndGet()
        val identity = io.github.groundgrounder.hortus.core.DiagramIdentity(
            document = documentKey,
            block = ordinal.toUInt(),
            revision = revision.toULong(),
            dark = dark,
            fontGeneration = fontPaths.size.toULong(),
            requestGeneration = request.toULong(),
        )
        try {
            val scene = MainThreadCounters.record("diagramRender") {
                engine.renderRequest(
                    source = engineText,
                    mmd = mmd,
                    width = width,
                    collapsed = collapsed,
                    identity = identity,
                    previous = null,
                    viewport = emptyList(),
                )
            }
            val counts = measureScene(scene, documentKey)
            DiagramInspection.record(
                "$documentKey#$ordinal@${width.toInt()}/$dark",
                scene,
                counts,
                dark,
                collapsed,
            )
            val result = DiagramRenderResult(scene, null, scene.identity, counts, scene.width, scene.height)
            synchronized(diagramCache) { diagramCache[key] = result }
            result
        } catch (failure: DiagramFailure) {
            val reason = diagramFailureText(failure)
            Inspection.recordDiagramFailure("${documentKey}#${ordinal}", ordinal, reason)
            DiagramRenderResult(null, reason, identity, null, 0.0, 0.0)
        } catch (error: Exception) {
            val reason = error.message ?: error.javaClass.simpleName
            Inspection.recordDiagramFailure("${documentKey}#${ordinal}", ordinal, reason)
            DiagramRenderResult(null, reason, identity, null, 0.0, 0.0)
        }
    }

    private fun measureScene(scene: SceneData, documentKey: String): SceneCounts {
        val counts = SceneCounts()
        for (primitive in scene.primitives) {
            when (primitive) {
                is io.github.groundgrounder.hortus.core.ScenePrimitive.RoundedRect -> counts.roundedRect += 1
                is io.github.groundgrounder.hortus.core.ScenePrimitive.Line -> counts.line += 1
                is io.github.groundgrounder.hortus.core.ScenePrimitive.GraphShape -> counts.graphShape += 1
                is io.github.groundgrounder.hortus.core.ScenePrimitive.LabelBackground -> counts.labelBackground += 1
                is io.github.groundgrounder.hortus.core.ScenePrimitive.GraphPath -> {
                    counts.graphPath += 1
                    when (primitive.line) {
                        "Solid" -> counts.solid += 1
                        "Dotted" -> counts.dotted += 1
                        "Thick" -> counts.thick += 1
                        else -> counts.otherLine += 1
                    }
                    when (primitive.start) {
                        "Arrow" -> counts.arrow += 1
                        "Circle" -> counts.circle += 1
                        "Cross" -> counts.cross += 1
                        "None" -> Unit
                        else -> counts.otherMarker += 1
                    }
                    when (primitive.end) {
                        "Arrow" -> counts.arrow += 1
                        "Circle" -> counts.circle += 1
                        "Cross" -> counts.cross += 1
                        "None" -> Unit
                        else -> counts.otherMarker += 1
                    }
                }
                is io.github.groundgrounder.hortus.core.ScenePrimitive.Text -> {
                    counts.text += 1
                    val span = primitive.span
                    if (span.width > 0.0) {
                        val paint = android.graphics.Paint(
                            android.graphics.Paint.ANTI_ALIAS_FLAG or
                                android.graphics.Paint.SUBPIXEL_TEXT_FLAG or
                                android.graphics.Paint.LINEAR_TEXT_FLAG,
                        ).apply {
                            typeface = DiagramFonts.typeface(span.fontFamily, span.fontWeight.toInt())
                            textSize = span.fontSize.toFloat() * 16f
                            isLinearText = true
                            isSubpixelText = true
                            hinting = android.graphics.Paint.HINTING_OFF
                            textLocale = DiagramFonts.localeFor(span.fontFamily)
                        }
                        if (span.width > 0.0) {
                            val target = span.width * 16.0
                            repeat(5) {
                                val raw = paint.measureText(span.text).toDouble()
                                if (raw <= 0.0) return@repeat
                                if (kotlin.math.abs(target - raw) <= 0.01) return@repeat
                                paint.textScaleX *= (target / raw).toFloat()
                            }
                            SceneTextMetrics.put(span, 1f, paint.textScaleX)
                        }
                        val measured = paint.measureText(span.text).toDouble() / 16.0
                        val error = kotlin.math.abs(measured - span.width)
                        counts.textSamples += 1
                        if (error > counts.maxTextWidthError) {
                            counts.maxTextWidthError = error
                            counts.worstTextDiagram = documentKey
                            counts.worstText = span.text.take(80)
                        }
                    }
                }
            }
        }
        return counts
    }

    private fun diagramFailureText(failure: DiagramFailure): String = when (failure) {
        is DiagramFailure.NotATree -> "diagram.not_a_tree: ${failure.multiParent.joinToString()}"
        is DiagramFailure.Unsupported -> "diagram.unsupported: ${failure.diagramType}"
        is DiagramFailure.UnsupportedConstruct -> "diagram.unsupported: ${failure.kind} (line ${failure.line})"
        is DiagramFailure.Syntax -> "diagram.syntax: ${failure.kind} (line ${failure.line})"
        is DiagramFailure.Engine -> "diagram.engine: ${failure.kind}"
        else -> failure.toString()
    }

    suspend fun resolveEmbed(
        sourcePath: String,
        target: String,
        heading: String?,
        alias: String?,
        ancestors: List<String>,
        width: Double,
        size: Double,
    ): EmbeddedContent = lock.withLock {
        val handle = vault ?: return@withLock EmbeddedContent(
            EmbedContentInfo(
                kind = "broken",
                resolvedPath = null,
                displayTitle = target,
                heading = heading,
                headingFound = false,
                isMmd = false,
                content = "",
                text = "",
                blocks = emptyList(),
                media = emptyList(),
                math = emptyList(),
                diagrams = emptyList(),
                links = emptyList(),
                tags = emptyList(),
                candidates = emptyList(),
            ),
            emptyList(),
            emptyList(),
            emptyList(),
        )
        val key = "$sourcePath|$target|$heading|$alias|${ancestors.joinToString(">")}|${width.toInt()}|${size.toInt()}"
        synchronized(embedCache) { embedCache[key]?.let { return@withLock it } }
        val info = MainThreadCounters.record("resolveEmbed") {
            handle.resolveEmbed(
                sourcePath = sourcePath,
                rawTarget = target,
                heading = heading,
                alias = alias,
                ancestors = ancestors,
                maxDepth = 8u,
                width = width,
                size = size,
            )
        }
        val content = buildEmbeddedItems(info, "$sourcePath->$target")
        synchronized(embedCache) { embedCache[key] = content }
        content
    }

    suspend fun collapsedNodes(note: String, block: Int): List<String> = lock.withLock {
        val root = vaultPath ?: return@withLock emptyList()
        try {
            requireWorkspace().collapsedNodes(root, note, block.toUInt())
        } catch (_: Exception) {
            emptyList()
        }
    }

    suspend fun rememberCollapsed(note: String, block: Int, collapsed: List<String>) = lock.withLock {
        val root = vaultPath ?: return@withLock
        try {
            requireWorkspace().rememberDiagram(root, note, block.toUInt(), collapsed)
        } catch (_: Exception) {
            Unit
        }
    }

    suspend fun focusState(note: String, block: Int): io.github.groundgrounder.hortus.core.DiagramFocus? =
        lock.withLock {
            val root = vaultPath ?: return@withLock null
            try {
                requireWorkspace().diagramFocus(root, note, block.toUInt())
            } catch (_: Exception) {
                null
            }
        }

    suspend fun rememberFocus(note: String, block: Int, scale: Double, offsetX: Double, offsetY: Double) =
        lock.withLock {
            val root = vaultPath ?: return@withLock
            try {
                requireWorkspace().rememberDiagramFocus(
                    root,
                    note,
                    block.toUInt(),
                    scale,
                    offsetX,
                    offsetY,
                )
            } catch (_: Exception) {
                Unit
            }
        }

    suspend fun exportSvg(
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
    ): String = lock.withLock {
        val engine = ensureDiagramEngine()
        MainThreadCounters.record("diagramRender") {
            engine.renderSvg(engineText, mmd, ordinal.toUInt(), width, dark, collapsed)
        }
    }

    suspend fun resolveLink(sourcePath: String, rawTarget: String): String? = lock.withLock {
        try {
            vault?.resolveLink(sourcePath, rawTarget)?.resolvedPath
        } catch (_: Exception) {
            null
        }
    }

    suspend fun readNote(path: String): String? = lock.withLock {
        try {
            vault?.readNote(path)
        } catch (_: Exception) {
            null
        }
    }

    override fun close() {
        stopScan()
        closeDocument()
        try {
            vault?.stopWatching()
            vault?.close()
        } catch (_: Exception) {
            Unit
        }
        vault = null
        try {
            document?.close()
        } catch (_: Exception) {
            Unit
        }
        document = null
        try {
            diagramEngine?.close()
        } catch (_: Exception) {
            Unit
        }
        diagramEngine = null
        try {
            workspace?.close()
        } catch (_: Exception) {
            Unit
        }
        workspace = null
        scope.cancel()
    }
}
