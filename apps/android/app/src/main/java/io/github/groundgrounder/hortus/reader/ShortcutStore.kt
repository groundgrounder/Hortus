// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import android.content.Intent
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import io.github.groundgrounder.hortus.MainActivity
import io.github.groundgrounder.hortus.R
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest

/**
 * 最近打开的库内笔记：动态快捷方式最多保留 4 篇，只写应用私有配置目录。
 */
internal class ShortcutStore(context: Context) {
    companion object {
        const val ACTION_SHORTCUT = "io.github.groundgrounder.hortus.SHORTCUT"
        private const val MAX_SHORTCUTS = 4
    }

    private val appContext = context.applicationContext
    private val directory = File(context.filesDir, "config").apply { mkdirs() }
    private val file = File(directory, "shortcuts.json")

    @Synchronized
    fun load(): List<RecentNote> {
        return try {
            if (!file.isFile) return emptyList()
            val array = JSONArray(file.readText())
            (0 until array.length()).mapNotNull { index ->
                val item = array.optJSONObject(index) ?: return@mapNotNull null
                val vault = item.optString("vault")
                val path = item.optString("path")
                if (vault.isBlank() || path.isBlank()) return@mapNotNull null
                RecentNote(
                    vault = vault,
                    path = path,
                    title = item.optString("title").ifBlank { path.substringAfterLast('/') },
                    openedAt = item.optLong("openedAt"),
                )
            }.take(MAX_SHORTCUTS)
        } catch (_: Exception) {
            emptyList()
        }
    }

    @Synchronized
    fun remember(vault: String, path: String, title: String, openedAt: Long): List<RecentNote> {
        val notes = load().filterNot { it.vault == vault && it.path == path }
        val updated = (listOf(RecentNote(vault, path, title.ifBlank { path.substringAfterLast('/') }, openedAt)) +
            notes).take(MAX_SHORTCUTS)
        save(updated)
        return updated
    }

    @Synchronized
    fun publish(notes: List<RecentNote>): List<String> {
        if (notes.isEmpty()) {
            ShortcutManagerCompat.removeAllDynamicShortcuts(appContext)
            return emptyList()
        }
        val shortcuts = notes.map { note ->
            ShortcutInfoCompat.Builder(appContext, shortcutId(note.vault, note.path))
                .setShortLabel(note.title.take(40))
                .setLongLabel(note.title.take(80))
                .setIcon(IconCompat.createWithResource(appContext, R.drawable.ic_launcher_foreground))
                .setIntent(intentFor(note))
                .build()
        }
        return if (ShortcutManagerCompat.setDynamicShortcuts(appContext, shortcuts)) {
            notes.map { it.title }
        } else {
            emptyList()
        }
    }

    @Synchronized
    fun requestPin(note: RecentNote): Boolean {
        publish(remember(note.vault, note.path, note.title, System.currentTimeMillis()))
        val shortcut = ShortcutInfoCompat.Builder(appContext, shortcutId(note.vault, note.path))
            .setShortLabel(note.title.take(40))
            .setLongLabel(note.title.take(80))
            .setIcon(IconCompat.createWithResource(appContext, R.drawable.ic_launcher_foreground))
            .setIntent(intentFor(note))
            .build()
        return try {
            ShortcutManagerCompat.requestPinShortcut(appContext, shortcut, null)
        } catch (_: Exception) {
            false
        }
    }

    fun launchIntent(index: Int): Intent? =
        ShortcutManagerCompat.getDynamicShortcuts(appContext).getOrNull(index)?.intent

    private fun intentFor(note: RecentNote): Intent =
        Intent(appContext, MainActivity::class.java)
            .setAction(ACTION_SHORTCUT)
            .putExtra(MainActivity.EXTRA_OPEN_VAULT, note.vault)
            .putExtra(MainActivity.EXTRA_OPEN_NOTE, note.path)

    private fun save(notes: List<RecentNote>) {
        try {
            val array = JSONArray()
            for (note in notes) {
                array.put(JSONObject()
                    .put("vault", note.vault)
                    .put("path", note.path)
                    .put("title", note.title)
                    .put("openedAt", note.openedAt))
            }
            val temporary = File(directory, ".shortcuts.tmp")
            temporary.writeText(array.toString(2) + "\n")
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
            )
        } catch (_: Exception) {
            Unit
        }
    }

    private fun shortcutId(vault: String, path: String): String {
        val digest = MessageDigest.getInstance("SHA-1")
            .digest((vault + "\u0000" + path).encodeToByteArray())
        return "note-" + digest.take(8).joinToString("") { "%02x".format(it.toInt() and 0xff) }
    }
}
