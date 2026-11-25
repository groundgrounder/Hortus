// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.model

import androidx.compose.runtime.Immutable
import androidx.compose.ui.text.input.TextFieldValue
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.TreeEntry
import io.github.groundgrounder.hortus.core.model.DiagramItem
import io.github.groundgrounder.hortus.core.model.DiagramRenderResult
import io.github.groundgrounder.hortus.core.model.EmbeddedContent
import io.github.groundgrounder.hortus.core.model.LoadedImage
import io.github.groundgrounder.hortus.core.model.NoteContent
import io.github.groundgrounder.hortus.core.model.OutlineRow
import io.github.groundgrounder.hortus.core.model.BacklinkRow
import io.github.groundgrounder.hortus.core.model.QuickRow
import io.github.groundgrounder.hortus.core.model.SearchRow
import io.github.groundgrounder.hortus.core.model.TagNoteRow
import io.github.groundgrounder.hortus.core.model.TagRow
import io.github.groundgrounder.hortus.core.model.TextLink
import io.github.groundgrounder.hortus.reader.render.MathRenderResult

internal data class ScrollTarget(val itemIndex: Int, val offsetPx: Int)
internal data class ReaderAnchor(val blockId: Long, val offsetPx: Int)

internal data class ReaderUiState(
    val loading: Boolean = false,
    val error: String? = null,
    val content: NoteContent? = null,
    val vaultPath: String? = null,
    val tree: List<TreeEntry> = emptyList(),
    val scrollTarget: ScrollTarget? = null,
    val notice: String? = null,
    val openSequence: Long = 0,
    val embedVersion: Long = 1L,
)

internal enum class KnowledgeTab { OUTLINE, BACKLINKS, TAGS }

internal enum class SearchTab { QUICK, FULL }

internal enum class SideMode { FILES, OUTLINE, BACKLINKS, TAGS }

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

@Immutable
internal data class ReaderCallbacks(
    val onLink: (TextLink) -> Unit,
    val onTag: (String) -> Unit,
    val onOpenPath: (String, String?) -> Unit,
    val onFocus: (DiagramItem, SceneData, List<String>) -> Unit,
    val onNotice: (String) -> Unit,
)

/**
 * UI 层请求公式/图片/图表/内嵌的窄接口，彻底阻断 ui/ 直接持有或锁住 data/ 仓储。
 */
internal interface ItemRenderBridge {
    suspend fun prepareMath(formulas: List<MathBlock>, bodySize: Float): List<MathRenderResult>
    suspend fun loadImage(path: String, targetWidthPx: Int, requestedWidthPx: Int?): LoadedImage
    suspend fun renderDiagram(
        vaultPath: String,
        documentKey: String,
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        revision: Long,
        previewContentWidthPx: Int,
        previewUnitScale: Float,
    ): DiagramRenderResult
    suspend fun collapsedNodes(vaultPath: String, note: String, block: Int): List<String>
    suspend fun resolveEmbed(
        sourcePath: String,
        target: String,
        heading: String?,
        alias: String?,
        ancestors: List<String>,
        width: Double,
        size: Double,
    ): EmbeddedContent
}
