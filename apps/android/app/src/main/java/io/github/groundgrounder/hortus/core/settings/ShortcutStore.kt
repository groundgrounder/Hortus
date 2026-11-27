// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.core.settings

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.model.RecentNote
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

internal object LaunchExtras {
    const val EXTRA_OPEN_NOTE = "io.github.groundgrounder.hortus.OPEN_NOTE"
    const val EXTRA_OPEN_HEADING = "io.github.groundgrounder.hortus.OPEN_HEADING"
    const val EXTRA_OPEN_VAULT = "io.github.groundgrounder.hortus.OPEN_VAULT"
}

/**
 * 最近打开的库内笔记：动态快捷方式最多保留 4 篇，全部文件与系统快捷方式调用均在 IO 线程执行。
 */
internal class ShortcutStore(context: Context) {
    companion object {
        const val ACTION_SHORTCUT = "io.github.groundgrounder.hortus.SHORTCUT"
        private const val MAX_SHORTCUTS = 4
    }

    private val appContext = context.applicationContext
    private val directory = File(appContext.filesDir, "config")
    private val file = File(directory, "shortcuts.json")
    private val lock = Any()

    @Volatile
    private var cachedNotes: List<RecentNote> = emptyList()

    fun currentNotes(): List<RecentNote> = cachedNotes

    suspend fun load(): List<RecentNote> = withContext(Dispatchers.IO) {
        synchronized(lock) { loadLocked() }
    }

    fun loadSync(): List<RecentNote> = synchronized(lock) { loadLocked() }

    suspend fun remember(vault: String, path: String, title: String, openedAt: Long): List<RecentNote> =
        withContext(Dispatchers.IO) {
            synchronized(lock) {
                val notes = loadLocked().filterNot { it.vault == vault && it.path == path }
                val updated = (listOf(RecentNote(vault, path, title.ifBlank { path.substringAfterLast('/') }, openedAt)) +
                    notes).take(MAX_SHORTCUTS)
                saveLocked(updated)
                cachedNotes = updated
                updated
            }
        }

    suspend fun publish(notes: List<RecentNote>): List<String> = withContext(Dispatchers.IO) {
        synchronized(lock) { publishLocked(notes) }
    }

    suspend fun requestPin(note: RecentNote): Boolean = withContext(Dispatchers.IO) {
        synchronized(lock) {
            val existing = loadLocked().filterNot { it.vault == note.vault && it.path == note.path }
            val safeTitle = note.title.trim().ifBlank {
                note.path.substringAfterLast('/').ifBlank { "Hortus" }
            }
            val pinnedNote = RecentNote(
                note.vault,
                note.path,
                safeTitle,
                System.currentTimeMillis(),
            )
            val updated = (listOf(pinnedNote) + existing).take(MAX_SHORTCUTS)
            saveLocked(updated)
            cachedNotes = updated
            publishLocked(updated)
            val label = updated.first().title.ifBlank {
                note.path.substringAfterLast('/').ifBlank { "Hortus" }
            }
            val shortcut = ShortcutInfoCompat.Builder(appContext, shortcutId(note.vault, note.path))
                .setShortLabel(label.take(40))
                .setLongLabel(label.take(80))
                .setIcon(IconCompat.createWithResource(appContext, R.drawable.ic_launcher_foreground))
                .setIntent(intentFor(pinnedNote))
                .build()
            try {
                ShortcutManagerCompat.requestPinShortcut(appContext, shortcut, null)
            } catch (_: Exception) {
                false
            }
        }
    }

    suspend fun launchIntent(index: Int): Intent? = withContext(Dispatchers.IO) {
        synchronized(lock) {
            ShortcutManagerCompat.getDynamicShortcuts(appContext).getOrNull(index)?.intent
        }
    }

    private fun loadLocked(): List<RecentNote> {
        val loaded = try {
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
        cachedNotes = loaded
        return loaded
    }

    private fun publishLocked(notes: List<RecentNote>): List<String> {
        if (notes.isEmpty()) {
            ShortcutManagerCompat.removeAllDynamicShortcuts(appContext)
            return emptyList()
        }
        val shortcuts = notes.map { note ->
            val label = note.title.trim().ifBlank {
                note.path.substringAfterLast('/').ifBlank { "Hortus" }
            }
            ShortcutInfoCompat.Builder(appContext, shortcutId(note.vault, note.path))
                .setShortLabel(label.take(40))
                .setLongLabel(label.take(80))
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

    private fun intentFor(note: RecentNote): Intent =
        Intent().apply {
            component = ComponentName(appContext.packageName, "${appContext.packageName}.MainActivity")
            action = ACTION_SHORTCUT
            putExtra(LaunchExtras.EXTRA_OPEN_VAULT, note.vault)
            putExtra(LaunchExtras.EXTRA_OPEN_NOTE, note.path)
        }

    private fun saveLocked(notes: List<RecentNote>) {
        try {
            directory.mkdirs()
            val array = JSONArray()
            for (note in notes) {
                array.put(
                    JSONObject()
                        .put("vault", note.vault)
                        .put("path", note.path)
                        .put("title", note.title)
                        .put("openedAt", note.openedAt),
                )
            }
            val temporary = File(directory, ".shortcuts.tmp")
            temporary.writeText(array.toString(2) + "\n")
            Files.move(
                temporary.toPath(),
                file.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
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
