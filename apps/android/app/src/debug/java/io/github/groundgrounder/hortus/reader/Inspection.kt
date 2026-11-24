// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import android.graphics.Paint
import io.github.groundgrounder.hortus.core.EmbedContentInfo
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MathStyle
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.ScenePrimitive
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.abs

/**
 * 验收 JSON 的跨屏汇总：嵌入状态、公式状态、图的失败、命中区、面板与导出结果。
 * 仅编译进 debug 源集，Release 与 benchmark 彻底零开销。
 */

internal data class LineMetrics(
    val index: Int,
    val top: Float,
    val bottom: Float,
    val baselineY: Float,
)

internal data class MathBoxMetrics(
    val formulaKey: String,
    val latex: String,
    val lineIndex: Int,
    val x: Float,
    val top: Float,
    val baselineY: Float,
    val bottom: Float,
    val width: Float,
    val em: Float,
    val rendered: Boolean,
)

internal data class PanelHit(
    val kind: String,
    val label: String,
    val x: Float,
    val y: Float,
    val width: Float,
    val height: Float,
    val target: String?,
    val tab: String,
)

internal object PanelRegistry {
    private val entries = LinkedHashMap<String, PanelHit>()

    fun put(scope: String, key: String, hit: PanelHit) {
        synchronized(entries) {
            entries["$scope/$key"] = hit
        }
    }

    fun retain(scope: String, keys: Collection<String>) {
        synchronized(entries) {
            val wanted = keys.map { "$scope/$it" }.toSet()
            entries.keys.removeAll { it.startsWith("$scope/") && it !in wanted }
        }
    }

    fun clear(scope: String) {
        synchronized(entries) {
            entries.keys.removeAll { it.startsWith("$scope/") }
        }
    }

    fun snapshot(): List<PanelHit> = synchronized(entries) {
        entries.values.filter {
            it.x >= 0f && it.y >= 0f && it.width > 0f && it.height > 0f
        }
    }

    fun reset() = synchronized(entries) { entries.clear() }
}

internal data class ScreenHit(
    val kind: String,
    val label: String,
    val x: Float,
    val y: Float,
    val width: Float,
    val height: Float,
    val target: String?,
    val heading: String?,
    val canCollapse: Boolean,
    val nodeId: String?,
)

internal object HitRegistry {
    private val entries = LinkedHashMap<String, List<ScreenHit>>()

    fun replace(scope: String, hits: List<ScreenHit>) {
        synchronized(entries) {
            if (hits.isEmpty()) entries.remove(scope) else entries[scope] = hits
            while (entries.size > 256) entries.remove(entries.keys.first())
        }
    }

    fun snapshot(): List<ScreenHit> = synchronized(entries) { entries.values.flatten() }

    fun clear() = synchronized(entries) { entries.clear() }
}

internal object ReaderTiming {
    @Volatile var activityCreatedAt: Long = 0
    @Volatile var openStartedAt: Long = 0
    @Volatile var contentInstalledAt: Long = 0
    @Volatile var firstFrameAt: Long = 0
    @Volatile var fullyDrawnAt: Long = 0
    @Volatile var openCount: Int = 0
}

internal object DebugSessionMetrics {
    @Volatile var layoutMode: String = "phone"
    @Volatile var externalMode: String? = null
    @Volatile var sidePaneWidthPx: Int = 0
    @Volatile var detailPaneWidthPx: Int = 0
    @Volatile var querySummary: String = ""
    @Volatile var lastSearchMicros: Long = 0
    @Volatile var lastSaveIssue: String? = null
    @Volatile var lastCreatedNote: String? = null
    @Volatile var shortcutsPublished: List<String> = emptyList()
    @Volatile var lastShortcutLaunchNote: String? = null
    @Volatile var lastShortcutLaunchVault: String? = null
    @Volatile var lastAction: String = ""
    @Volatile var lastOpenTarget: ScrollTarget? = null
}

internal object ImageInspection {
    private val records = ConcurrentHashMap<String, LoadedImage>()

    fun record(path: String, image: LoadedImage) {
        if (!Inspection.recordingEnabled) return
        records[path] = image
    }

    fun snapshot(): List<Pair<String, LoadedImage>> = records.entries.map { it.key to it.value }

    fun clear() = records.clear()
}

internal class SceneCounts {
    var roundedRect = 0
    var line = 0
    var graphPath = 0
    var graphShape = 0
    var labelBackground = 0
    var text = 0
    var arrow = 0
    var circle = 0
    var cross = 0
    var solid = 0
    var dotted = 0
    var thick = 0
    var otherMarker = 0
    var otherLine = 0
    var textSamples = 0
    var maxTextWidthError = 0.0
    var worstTextDiagram = ""
    var worstText = ""

    fun merge(other: SceneCounts) {
        roundedRect += other.roundedRect
        line += other.line
        graphPath += other.graphPath
        graphShape += other.graphShape
        labelBackground += other.labelBackground
        text += other.text
        arrow += other.arrow
        circle += other.circle
        cross += other.cross
        solid += other.solid
        dotted += other.dotted
        thick += other.thick
        otherMarker += other.otherMarker
        otherLine += other.otherLine
        textSamples += other.textSamples
        if (other.maxTextWidthError > maxTextWidthError) {
            maxTextWidthError = other.maxTextWidthError
            worstTextDiagram = other.worstTextDiagram
            worstText = other.worstText
        }
    }

    fun toJson(): JSONObject = JSONObject()
        .put("RoundedRect", roundedRect)
        .put("Line", line)
        .put("GraphPath", graphPath)
        .put("GraphShape", graphShape)
        .put("LabelBackground", labelBackground)
        .put("Text", text)
        .put("Arrow", arrow)
        .put("Circle", circle)
        .put("Cross", cross)
        .put("Solid", solid)
        .put("Dotted", dotted)
        .put("Thick", thick)
        .put("otherMarker", otherMarker)
        .put("otherLine", otherLine)
        .put("textSamples", textSamples)
        .put("maxTextWidthErrorPx", maxTextWidthError)
        .put("worstTextDiagram", worstTextDiagram)
        .put("worstText", worstText)
}

internal object DiagramInspection {
    private val lock = Any()
    val total = SceneCounts()
    private val records = LinkedHashMap<String, JSONObject>()

    fun record(key: String, scene: SceneData, counts: SceneCounts, dark: Boolean, collapsed: List<String>) {
        synchronized(lock) {
            total.merge(counts)
            records[key] = JSONObject()
                .put("key", key)
                .put("width", scene.width)
                .put("height", scene.height)
                .put("dark", dark)
                .put("collapsed", JSONArray(collapsed))
                .put("primitives", scene.primitives.size)
                .put("hits", scene.hitRegions.size)
                .put("plusLabels", scene.primitives.count {
                    it is ScenePrimitive.Text &&
                        it.span.text.trim().startsWith("+")
                })
                .put("counts", counts.toJson())
            while (records.size > 240) {
                val first = records.keys.first()
                records.remove(first)
            }
        }
    }

    fun measureScene(scene: SceneData, documentKey: String, zoom: Float = 1f): SceneCounts {
        val counts = SceneCounts()
        for (primitive in scene.primitives) {
            when (primitive) {
                is ScenePrimitive.RoundedRect -> counts.roundedRect += 1
                is ScenePrimitive.Line -> counts.line += 1
                is ScenePrimitive.GraphShape -> counts.graphShape += 1
                is ScenePrimitive.LabelBackground -> counts.labelBackground += 1
                is ScenePrimitive.GraphPath -> {
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
                is ScenePrimitive.Text -> {
                    counts.text += 1
                    val span = primitive.span
                    if (span.width > 0.0) {
                        val paint = SceneTextMetrics.paint(span, zoom, android.graphics.Color.BLACK)
                        val measured = paint.measureText(span.text).toDouble() / 16.0 / zoom
                        val error = abs(measured - span.width)
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

    fun snapshot(): JSONArray = synchronized(lock) {
        JSONArray().also { array ->
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

internal object DebugPreviewInspection {
    private val lock = Any()
    @Volatile var recordings: Int = 0
        private set
    @Volatile var replays: Int = 0
        private set
    val counts = SceneCounts()

    fun record(key: String, scene: SceneData, zoom: Float) {
        val measured = DiagramInspection.measureScene(scene, key, zoom)
        synchronized(lock) {
            counts.merge(measured)
            recordings += 1
        }
    }

    fun noteReplay() {
        replays += 1
    }

    fun reset() = synchronized(lock) {
        recordings = 0
        replays = 0
        val empty = SceneCounts()
        counts.roundedRect = empty.roundedRect
        counts.line = empty.line
        counts.graphPath = empty.graphPath
        counts.graphShape = empty.graphShape
        counts.labelBackground = empty.labelBackground
        counts.text = empty.text
        counts.arrow = empty.arrow
        counts.circle = empty.circle
        counts.cross = empty.cross
        counts.solid = empty.solid
        counts.dotted = empty.dotted
        counts.thick = empty.thick
        counts.otherMarker = empty.otherMarker
        counts.otherLine = empty.otherLine
        counts.textSamples = empty.textSamples
        counts.maxTextWidthError = empty.maxTextWidthError
        counts.worstText = ""
        counts.worstTextDiagram = ""
    }
}

internal object LayoutInspection {
    private val recordLock = Any()
    private val textLayouts = LinkedHashMap<String, JSONObject>()
    private val tableLayouts = LinkedHashMap<String, JSONObject>()
    private val blockMath = LinkedHashMap<String, JSONObject>()
    private val itemBounds = LinkedHashMap<String, JSONObject>()

    private fun lineJson(line: LineMetrics): JSONObject = JSONObject()
        .put("line", line.index)
        .put("top", line.top)
        .put("bottom", line.bottom)
        .put("baselineY", line.baselineY)

    private fun formulaJson(box: MathBoxMetrics): JSONObject = JSONObject()
        .put("key", box.formulaKey)
        .put("latex", box.latex)
        .put("line", box.lineIndex)
        .put("x", box.x)
        .put("top", box.top)
        .put("baselineY", box.baselineY)
        .put("bottom", box.bottom)
        .put("width", box.width)
        .put("em", box.em)
        .put("rendered", box.rendered)

    fun recordText(
        key: String,
        note: String,
        blockId: Long,
        lines: List<LineMetrics>,
        formulas: List<MathBoxMetrics>,
    ) {
        if (!Inspection.recordingEnabled) return
        val value = JSONObject()
            .put("key", key)
            .put("note", note)
            .put("blockId", blockId)
            .put("lines", JSONArray().also { array -> lines.forEach { array.put(lineJson(it)) } })
            .put("formulas", JSONArray().also { array -> formulas.forEach { array.put(formulaJson(it)) } })
        synchronized(recordLock) {
            textLayouts[key] = value
            while (textLayouts.size > 400) textLayouts.remove(textLayouts.keys.first())
        }
    }

    fun recordTable(
        key: String,
        note: String,
        blockId: Long,
        columns: List<Float>,
        rowHeights: List<Float>,
        cells: List<JSONObject>,
    ) {
        if (!Inspection.recordingEnabled) return
        val value = JSONObject()
            .put("key", key)
            .put("note", note)
            .put("blockId", blockId)
            .put("columns", JSONArray(columns))
            .put("rowHeights", JSONArray(rowHeights))
            .put("cells", JSONArray().also { array -> cells.forEach { array.put(it) } })
        synchronized(recordLock) {
            tableLayouts[key] = value
            while (tableLayouts.size > 120) tableLayouts.remove(tableLayouts.keys.first())
        }
    }

    fun recordBlockMath(key: String, note: String, blockId: Long, value: JSONObject) {
        if (!Inspection.recordingEnabled) return
        synchronized(recordLock) {
            blockMath[key] = value.put("key", key).put("note", note).put("blockId", blockId)
            while (blockMath.size > 200) blockMath.remove(blockMath.keys.first())
        }
    }

    fun recordItemBounds(key: String, blockId: Long, y: Float, height: Float) {
        if (!Inspection.recordingEnabled) return
        synchronized(recordLock) {
            itemBounds[key] = JSONObject()
                .put("key", key)
                .put("blockId", blockId)
                .put("y", y)
                .put("height", height)
            while (itemBounds.size > 400) itemBounds.remove(itemBounds.keys.first())
        }
    }

    fun textJson(): JSONArray = synchronized(recordLock) {
        JSONArray().also { array -> textLayouts.values.forEach(array::put) }
    }

    fun tableJson(): JSONArray = synchronized(recordLock) {
        JSONArray().also { array -> tableLayouts.values.forEach(array::put) }
    }

    fun blockMathJson(): JSONArray = synchronized(recordLock) {
        JSONArray().also { array -> blockMath.values.forEach(array::put) }
    }

    fun boundsJson(): JSONArray = synchronized(recordLock) {
        JSONArray().also { array -> itemBounds.values.forEach(array::put) }
    }

    fun reset() = synchronized(recordLock) {
        textLayouts.clear()
        tableLayouts.clear()
        blockMath.clear()
        itemBounds.clear()
    }
}

internal object Inspection {
    @Volatile var recordingEnabled: Boolean = false
    private val embeds = ConcurrentHashMap<String, JSONObject>()
    private val math = ConcurrentHashMap<String, JSONObject>()
    private val diagramFailures = ConcurrentHashMap<String, JSONObject>()
    private val exports = ConcurrentHashMap<String, JSONObject>()
    private val shares = ConcurrentHashMap<String, JSONObject>()
    private val inlinePreviews = ConcurrentHashMap<String, JSONObject>()
    private val diagramRects = ConcurrentHashMap<String, JSONObject>()
    val mathMetrics = AtomicInteger()
    val composedDiagramCanvases = AtomicInteger()
    val maxComposedDiagramCanvases = AtomicInteger()
    val visibleDiagramCanvases = AtomicInteger()
    val maxVisibleDiagramCanvases = AtomicInteger()

    fun writeDebugFile(context: Context, name: String, bytes: ByteArray): File {
        val directory = checkNotNull(context.getExternalFilesDir("exports")) { "external files unavailable" }
        directory.mkdirs()
        val file = File(directory, name)
        val temporary = File(directory, ".$name.tmp")
        temporary.writeBytes(bytes)
        check(temporary.setReadable(true, false)) { "cannot make export readable" }
        try {
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
                java.nio.file.StandardCopyOption.ATOMIC_MOVE,
            )
        } catch (_: Exception) {
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
            )
        }
        return file
    }

    fun recordEmbed(key: String, info: EmbedContentInfo) {
        if (!recordingEnabled) return
        embeds[key] = JSONObject()
            .put("key", key)
            .put("kind", info.kind)
            .put("resolvedPath", info.resolvedPath)
            .put("title", info.displayTitle)
            .put("heading", info.heading)
            .put("headingFound", info.headingFound)
            .put("isMmd", info.isMmd)
            .put("blocks", info.blocks.size)
            .put("candidates", JSONArray(info.candidates))
    }

    fun recordMath(note: String, formula: MathBlock, result: MathRenderResult, bodySize: Float) {
        if (!recordingEnabled) return
        val prepared = result.prepared
        val key = "$note|${formula.blockId}|${formula.displayStart}|${formula.style}|${formula.latex}"
        if (result.metricsCodepoints > 0) mathMetrics.addAndGet(result.metricsCodepoints)
        math[key] = JSONObject()
            .put("note", note)
            .put("latex", formula.latex)
            .put("source", formula.displaySource)
            .put("style", formula.style.name.lowercase())
            .put("block", formula.block)
            .put("rendered", prepared != null)
            .put("reason", result.reason)
            .put("em", prepared?.em)
            .put("bodySize", bodySize)
            .put("emRatio", prepared?.let { it.em / bodySize })
            .put("width", prepared?.width)
            .put("height", prepared?.height)
            .put("ascent", prepared?.ascent)
            .put("descent", prepared?.descent)
            .put("glyphs", prepared?.glyphCount)
            .put("fallbackGlyphs", prepared?.fallbackGlyphCount)
            .put("unresolvedGlyphs", prepared?.unresolvedGlyphCount)
            .put("metricsCodepoints", result.metricsCodepoints)
    }

    fun recordDiagramFailure(key: String, ordinal: Int, reason: String) {
        if (!recordingEnabled) return
        diagramFailures[key] = JSONObject()
            .put("key", key)
            .put("ordinal", ordinal)
            .put("reason", reason)
    }

    fun recordExport(format: String, name: String, bytes: Int, path: String?, extra: JSONObject? = null) {
        if (!recordingEnabled) return
        exports["$format|$name|$path"] = JSONObject()
            .put("format", format)
            .put("name", name)
            .put("bytes", bytes)
            .put("path", path)
            .put("extra", extra)
    }

    fun recordShare(
        action: String,
        mime: String,
        uri: String,
        name: String,
        bytes: Int,
        contentMatches: Boolean,
        contentBytes: Int,
    ) {
        if (!recordingEnabled) return
        shares["$action|$name|${shares.size}"] = JSONObject()
            .put("action", action)
            .put("mime", mime)
            .put("uri", uri)
            .put("name", name)
            .put("bytes", bytes)
            .put("contentBytes", contentBytes)
            .put("contentMatches", contentMatches)
    }

    fun recordInlinePreview(
        key: String,
        hits: Int,
        plusLabels: Int,
        collapsed: List<String>,
        pictureKey: String,
        heightPx: Float,
        layoutWidth: Double,
        unitScale: Float,
        drawZoom: Float,
        drawnMainFontPx: Float,
    ) {
        if (!recordingEnabled) return
        inlinePreviews[key] = JSONObject()
            .put("key", key)
            .put("hits", hits)
            .put("plusLabels", plusLabels)
            .put("collapsed", JSONArray(collapsed))
            .put("pictureKey", pictureKey)
            .put("heightPx", heightPx)
            .put("layoutWidth", layoutWidth)
            .put("unitScale", unitScale)
            .put("drawZoom", drawZoom)
            .put("drawnMainFontPx", drawnMainFontPx)
            .put("recordedAt", System.nanoTime())
    }

    fun recordDiagramRect(key: String, x: Float, y: Float, width: Float, height: Float) {
        if (!recordingEnabled) return
        diagramRects[key] = JSONObject()
            .put("key", key)
            .put("x", x)
            .put("y", y)
            .put("width", width)
            .put("height", height)
    }

    fun diagramRectsJson(): JSONArray = JSONArray().also { array ->
        diagramRects.values.sortedBy { it.optString("key") }.forEach(array::put)
    }

    fun inlinePreviewsJson(): JSONArray = JSONArray().also { array ->
        inlinePreviews.values.sortedBy { it.optString("key") }.forEach(array::put)
    }

    fun sharesJson(): JSONArray = JSONArray().also { array ->
        shares.values.sortedBy { it.optString("name") }.forEach(array::put)
    }

    fun embedsJson(): JSONArray = JSONArray().also { array ->
        embeds.values.sortedBy { it.optString("key") }.forEach(array::put)
    }

    fun mathJson(): JSONArray = JSONArray().also { array ->
        math.values.sortedBy { it.optString("key") }.forEach(array::put)
    }

    fun failuresJson(): JSONArray {
        val array = JSONArray()
        diagramFailures.values.sortedBy { it.optString("key") }.forEach(array::put)
        return array
    }

    fun exportsJson(): JSONArray = JSONArray().also { array ->
        exports.values.sortedBy { it.optString("name") }.forEach(array::put)
    }

    fun reset() {
        embeds.clear()
        math.clear()
        diagramFailures.clear()
        exports.clear()
        shares.clear()
        inlinePreviews.clear()
        diagramRects.clear()
        mathMetrics.set(0)
        composedDiagramCanvases.set(0)
        maxComposedDiagramCanvases.set(0)
        visibleDiagramCanvases.set(0)
        maxVisibleDiagramCanvases.set(0)
        DiagramInspection.reset()
        LayoutInspection.reset()
        DebugPreviewInspection.reset()
        DiagramPreviewCache.reset()
        ImageInspection.clear()
        PanelRegistry.reset()
        HitRegistry.clear()
    }

    fun styleName(style: MathStyle): String = style.name.lowercase()
}
