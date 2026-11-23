// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import org.json.JSONObject
import java.io.File

internal data class ReaderPreferenceState(val vaultPath: String?, val notePath: String?)

/** 只在应用私有配置目录记录上次打开的库与笔记；库目录永远不写应用状态。 */
internal class ReaderPreferences(context: Context) {
    private val directory = File(context.filesDir, "config").apply { mkdirs() }
    private val file = File(directory, "reader-state.json")

    fun load(): ReaderPreferenceState {
        return try {
            if (!file.isFile) return ReaderPreferenceState(null, null)
            val json = JSONObject(file.readText())
            ReaderPreferenceState(
                json.optString("vault").takeIf { it.isNotBlank() },
                json.optString("note").takeIf { it.isNotBlank() },
            )
        } catch (_: Exception) {
            ReaderPreferenceState(null, null)
        }
    }

    fun save(vaultPath: String?, notePath: String?) {
        try {
            val temporary = File(directory, ".reader-state.tmp")
            temporary.writeText(
                JSONObject()
                    .put("vault", vaultPath ?: "")
                    .put("note", notePath ?: "")
                    .toString(2) + "\n",
            )
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
            )
        } catch (_: Exception) {
            Unit
        }
    }
}
