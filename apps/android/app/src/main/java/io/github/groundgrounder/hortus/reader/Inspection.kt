// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import io.github.groundgrounder.hortus.core.EmbedContentInfo
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MathStyle
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * 验收 JSON 的跨屏汇总：嵌入状态、公式状态、图的失败和导出结果。
 * 只在调试入口读取，不改动阅读路径的语义。
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

/**
 * 版面检查数据：文字块与表格单元格逐行的基线、行框，以及每个公式的绘制外框。
 * 记录发生在准备版面时；调试入口只读取快照。
 */
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
        DiagramPreviewCache.reset()
    }

    fun styleName(style: MathStyle): String = style.name.lowercase()
}
