// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import androidx.compose.ui.text.input.TextFieldValue

internal enum class KnowledgeTab { OUTLINE, BACKLINKS, TAGS }

internal enum class SearchTab { QUICK, FULL }

internal enum class SideMode { FILES, OUTLINE, BACKLINKS, TAGS }

internal data class OutlineRow(
    val level: Int,
    val title: String,
    val start: Int,
)

internal data class BacklinkRow(
    val source: String,
    val title: String,
    val line: Int,
    val context: String,
    val inDiagram: Boolean,
    val embed: Boolean,
    val startUtf16: Int,
    val lengthUtf16: Int,
)

internal data class TagRow(
    val tag: String,
    val raw: String,
    val count: Int,
    val notes: List<String>,
)

internal data class TagNoteRow(val path: String, val title: String)

internal data class QuickRow(
    val path: String,
    val title: String,
    val titleMatches: List<IntRange>,
    val pathMatches: List<IntRange>,
)

internal data class SearchSnippet(
    val start: Int,
    val length: Int,
    val snippet: String,
    val snippetStart: Int,
)

internal data class SearchRow(
    val path: String,
    val title: String,
    val count: Int,
    val matches: List<SearchSnippet>,
)

internal data class SearchState(
    val open: Boolean = false,
    val tab: SearchTab = SearchTab.QUICK,
    val query: String = "",
    val quick: List<QuickRow> = emptyList(),
    val full: List<SearchRow> = emptyList(),
    val running: Boolean = false,
    val generation: Long = 0,
    val cancelled: Int = 0,
    val staleDiscarded: Int = 0,
)

internal data class KnowledgeState(
    val open: Boolean = false,
    val tab: KnowledgeTab = KnowledgeTab.OUTLINE,
    val outline: List<OutlineRow> = emptyList(),
    val backlinks: List<BacklinkRow> = emptyList(),
    val tags: List<TagRow> = emptyList(),
    val selectedTag: String? = null,
    val tagNotes: List<TagNoteRow> = emptyList(),
    val loading: Boolean = false,
)

internal data class EditState(
    val active: Boolean = false,
    val field: TextFieldValue = TextFieldValue(""),
    val savedText: String = "",
    val dirty: Boolean = false,
    val saving: Boolean = false,
    val issue: String? = null,
    val copyPath: String? = null,
    val lastSaveOk: Boolean? = null,
    val lastSaveMicros: Long = 0,
    val sessionId: Long = 0,
    val readOnlyReason: String? = null,
    val choice: String? = null,
)

internal data class NewNoteState(
    val open: Boolean = false,
    val name: String = "",
    val parent: String = "",
    val error: String? = null,
    val creating: Boolean = false,
)

internal data class RecentNote(
    val vault: String,
    val path: String,
    val title: String,
    val openedAt: Long,
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
}
