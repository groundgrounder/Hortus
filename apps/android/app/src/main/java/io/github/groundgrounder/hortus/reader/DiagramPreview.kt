// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.graphics.RenderNode
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.ScenePrimitive
import java.util.TreeMap
import java.util.concurrent.ConcurrentHashMap

/** 图表引擎的主文字逻辑字号；屏幕上应按正文像素字号绘制。 */
internal const val DIAGRAM_LOGICAL_FONT_SIZE = 15f

/** 逻辑点到屏幕像素的系数：15 逻辑点主文字对应阅读正文的像素字号。 */
internal fun diagramUnitScale(bodySizePx: Float, density: Float): Float =
    (bodySizePx / DIAGRAM_LOGICAL_FONT_SIZE).takeIf { it > 0f && it.isFinite() }
        ?: (density * (16f / DIAGRAM_LOGICAL_FONT_SIZE))

/** 图中出现次数最多的文本字号，即节点主文字的逻辑字号。 */
internal fun diagramMainFontSize(scene: SceneData): Float {
    val counts = TreeMap<Float, Int>()
    for (primitive in scene.primitives) {
        if (primitive !is ScenePrimitive.Text) continue
        val size = primitive.span.fontSize.toFloat()
        counts[size] = (counts[size] ?: 0) + 1
    }
    return counts.maxByOrNull { it.value }?.key ?: 0f
}

/**
 * 正文里的图只记录一次矢量绘制命令，滚动时重放同一份记录。
 * 身份包含文档、块、宽度、深浅色与折叠状态，任一变化才重新录制。
 */
internal object DiagramPreviewCache {
    private val entries = object : LinkedHashMap<String, RenderNode>(6, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, RenderNode>): Boolean =
            size > 12
    }
    @Volatile var recordings: Int = 0
        private set
    @Volatile var replays: Int = 0
        private set
    val counts = SceneCounts()

    fun size(): Int = synchronized(entries) { entries.size }

    fun renderNode(
        key: String,
        scene: SceneData,
        width: Int,
        height: Int,
        zoom: Float,
        foreground: Int,
    ): RenderNode = synchronized(entries) {
        val cached = entries[key]
        if (cached != null) return@synchronized cached
        val nodeWidth = width.coerceAtLeast(1)
        val nodeHeight = height.coerceAtLeast(1)
        val node = RenderNode("hortus-diagram")
        node.setPosition(0, 0, nodeWidth, nodeHeight)
        val canvas = node.beginRecording(nodeWidth, nodeHeight)
        SceneRenderer.draw(
            canvas,
            scene,
            foreground,
            zoom = zoom,
            counts = if (Inspection.recordingEnabled) counts else null,
            diagramKey = key,
        )
        node.endRecording()
        recordings += 1
        entries[key] = node
        node
    }

    fun noteReplay() {
        replays += 1
    }

    fun reset() = synchronized(entries) {
        entries.clear()
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
        counts.worstText = empty.worstText
        counts.worstTextDiagram = empty.worstTextDiagram
    }
}

/**
 * 全屏页改变折叠状态后，正文预览不用重新打开笔记就能看到新状态。
 * 只在内存里传递，状态仍由内核写进缓存目录。
 */
internal object DiagramStateBus {
    var version by mutableIntStateOf(0)
        private set
    private val collapsed = ConcurrentHashMap<String, List<String>>()

    private fun key(notePath: String, ordinal: Int): String = "$notePath|$ordinal"

    fun publish(notePath: String, ordinal: Int, nodes: List<String>) {
        collapsed[key(notePath, ordinal)] = nodes.toList()
        version += 1
    }

    fun nodes(notePath: String, ordinal: Int): List<String>? = collapsed[key(notePath, ordinal)]
}
