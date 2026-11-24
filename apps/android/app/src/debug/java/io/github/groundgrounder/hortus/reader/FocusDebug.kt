// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

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

    fun stateJson(): JSONObject {
        val state = JSONObject()
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
        return state
    }
}
