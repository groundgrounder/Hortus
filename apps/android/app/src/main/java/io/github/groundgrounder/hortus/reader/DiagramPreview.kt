// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.graphics.RenderNode
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import io.github.groundgrounder.hortus.core.SceneData
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.ceil

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
        foreground: Int,
    ): RenderNode = synchronized(entries) {
        val cached = entries[key]
        if (cached != null) return@synchronized cached
        val scale = if (scene.width > 0.0) width.toFloat() / scene.width.toFloat() else 1f
        val height = ceil(scene.height.toFloat() * scale).toInt().coerceAtLeast(1)
        val node = RenderNode("hortus-diagram")
        node.setPosition(0, 0, width.coerceAtLeast(1), height)
        val canvas = node.beginRecording(width.coerceAtLeast(1), height)
        canvas.save()
        canvas.scale(scale, scale)
        SceneRenderer.draw(
            canvas,
            scene,
            foreground,
            counts = if (Inspection.recordingEnabled) counts else null,
            diagramKey = key,
        )
        canvas.restore()
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
