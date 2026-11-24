// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.SceneHit
import io.github.groundgrounder.hortus.reader.render.SceneRenderer
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

internal object FocusCounts {
    @Volatile var counts = SceneCounts()
}

/** 顶栏与系统安全区的实测像素值，随 focus-state.json 输出，供验收脚本断言。 */
internal object FocusChromeMetrics {
    @Volatile var topInsetPx: Int = 0
    @Volatile var topBarContentTopPx: Int = 0
    @Volatile var topBarBottomPx: Int = 0
}

internal object FocusStateRegistry {
    @Volatile var lastScale: Float = 0f
    @Volatile var lastOffsetX: Float = 0f
    @Volatile var lastOffsetY: Float = 0f
    @Volatile var sceneWidth: Float = 0f
    @Volatile var sceneHeight: Float = 0f
    @Volatile var largestFontPx: Float = 0f
    @Volatile var baseLargestFontPx: Float = 0f
    @Volatile var pointerEvents: Int = 0
    @Volatile var gestureDistance: Float = 0f
    @Volatile var sceneFontSize: Float = 0f
    @Volatile var unitScale: Float = 0f
    @Volatile var drawnFontSize: Float = 0f
    @Volatile var sceneKey: String = ""
    @Volatile var notePath: String = ""
    @Volatile var ordinal: Int = 0
    @Volatile var collapsed: List<String> = emptyList()
    @Volatile var visibleNodes: Int = 0
    @Volatile var plusLabels: Int = 0
    @Volatile var fitScale: Float = 0f
    @Volatile var lastTapSceneX: Float = 0f
    @Volatile var lastTapSceneY: Float = 0f
    @Volatile var lastHitLabel: String = ""
    @Volatile var lastHitNodeId: String = ""
    @Volatile var lastHitCanCollapse: Boolean = false
    @Volatile var hits: List<SceneHit> = emptyList()
}

/** Debug 构建把全屏页当前状态写到应用外部目录，供验收脚本读取；Release 里为空实现。 */
internal object FocusDebug {
    fun write(context: Context, state: JSONObject) {
        runCatching {
            val directory = context.getExternalFilesDir("focus") ?: return
            directory.mkdirs()
            val file = File(directory, "focus-state.json")
            val temporary = File(directory, ".focus-state.json.tmp")
            temporary.writeText(state.toString(2) + "\n")
            temporary.setReadable(true, false)
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
            )
        }
    }

    fun stateJson(): JSONObject = JSONObject()
        .put("scale", FocusStateRegistry.lastScale)
        .put("offsetX", FocusStateRegistry.lastOffsetX)
        .put("offsetY", FocusStateRegistry.lastOffsetY)
        .put("fitScale", FocusStateRegistry.fitScale)
        .put("sceneWidth", FocusStateRegistry.sceneWidth)
        .put("sceneHeight", FocusStateRegistry.sceneHeight)
        .put("sceneFontSize", FocusStateRegistry.sceneFontSize)
        .put("unitScale", FocusStateRegistry.unitScale)
        .put("drawnFontSize", FocusStateRegistry.drawnFontSize)
        .put("sceneKey", FocusStateRegistry.sceneKey)
        .put("notePath", FocusStateRegistry.notePath)
        .put("ordinal", FocusStateRegistry.ordinal)
        .put("visibleNodes", FocusStateRegistry.visibleNodes)
        .put("plusLabels", FocusStateRegistry.plusLabels)
        .put("collapsed", JSONArray(FocusStateRegistry.collapsed))
        .put("hits", JSONArray().also { array ->
            FocusStateRegistry.hits.forEach { hit ->
                array.put(
                    JSONObject()
                        .put("label", hit.label)
                        .put("nodeId", hit.nodeId)
                        .put("canCollapse", hit.canCollapse)
                        .put("target", hit.linkTarget)
                        .put("heading", hit.linkHeading)
                        .put("x", hit.rect.x)
                        .put("y", hit.rect.y)
                        .put("width", hit.rect.width)
                        .put("height", hit.rect.height),
                )
            }
        })
        .put("topInsetPx", FocusChromeMetrics.topInsetPx)
        .put("topBarContentTopPx", FocusChromeMetrics.topBarContentTopPx)
        .put("topBarBottomPx", FocusChromeMetrics.topBarBottomPx)

    fun measureFocusScene(scene: SceneData, documentKey: String, ordinal: Int) {
        val counts = SceneCounts()
        val width = scene.width.toInt().coerceAtLeast(1)
        val height = scene.height.toInt().coerceAtLeast(1)
        val bitmap = android.graphics.Bitmap.createBitmap(
            width,
            height,
            android.graphics.Bitmap.Config.ARGB_8888,
        )
        try {
            SceneRenderer.draw(
                android.graphics.Canvas(bitmap),
                scene,
                android.graphics.Color.BLACK,
            )
            counts.merge(DiagramInspection.measureScene(scene, "$documentKey#$ordinal"))
        } finally {
            bitmap.recycle()
        }
        FocusCounts.counts = counts
    }
}
