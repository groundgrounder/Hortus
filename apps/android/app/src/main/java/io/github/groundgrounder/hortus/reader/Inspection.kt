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
internal object Inspection {
    private val embeds = ConcurrentHashMap<String, JSONObject>()
    private val math = ConcurrentHashMap<String, JSONObject>()
    private val diagramFailures = ConcurrentHashMap<String, JSONObject>()
    private val exports = ConcurrentHashMap<String, JSONObject>()
    val mathMetrics = AtomicInteger()
    val composedDiagramCanvases = AtomicInteger()
    val maxComposedDiagramCanvases = AtomicInteger()
    val visibleDiagramCanvases = AtomicInteger()
    val maxVisibleDiagramCanvases = AtomicInteger()

    fun recordEmbed(key: String, info: EmbedContentInfo) {
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
        diagramFailures[key] = JSONObject()
            .put("key", key)
            .put("ordinal", ordinal)
            .put("reason", reason)
    }

    fun recordExport(format: String, name: String, bytes: Int, path: String?, extra: JSONObject? = null) {
        exports["$format|$name|$path"] = JSONObject()
            .put("format", format)
            .put("name", name)
            .put("bytes", bytes)
            .put("path", path)
            .put("extra", extra)
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
        mathMetrics.set(0)
        composedDiagramCanvases.set(0)
        maxComposedDiagramCanvases.set(0)
        visibleDiagramCanvases.set(0)
        maxVisibleDiagramCanvases.set(0)
        DiagramInspection.reset()
    }

    fun styleName(style: MathStyle): String = style.name.lowercase()
}
