// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.core.model

import android.graphics.Bitmap
import io.github.groundgrounder.hortus.core.DiagramBlock
import io.github.groundgrounder.hortus.core.DiagramIdentity
import io.github.groundgrounder.hortus.core.DocumentOutline
import io.github.groundgrounder.hortus.core.KnowledgeMarkupInfo
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MediaBlock
import io.github.groundgrounder.hortus.core.SceneData

internal data class NoteContent(
    val vaultPath: String,
    val path: String,
    val revision: Long,
    val model: DecodedReadingModel,
    val items: List<ReaderItem>,
    val media: List<MediaBlock>,
    val math: List<MathBlock>,
    val knowledge: KnowledgeMarkupInfo,
    val sourceText: String,
    val outline: DocumentOutline,
    val readOnlyReason: String?,
    val diagramBlocks: List<DiagramBlock>,
    val width: Int,
    val bodySize: Float,
    val isMmd: Boolean,
    val externalPreview: Boolean = false,
) {
    val title: String = run {
        val heading = outline.headings.firstOrNull { it.level.toInt() == 1 }?.title
            ?: outline.headings.firstOrNull()?.title
        heading?.trim()?.takeIf { it.isNotEmpty() } ?: path.substringAfterLast('/').substringBeforeLast('.')
    }
    val diagramItems: List<DiagramItem> = items.filterIsInstance<DiagramItem>()
}

internal data class DiagramRenderResult(
    val scene: SceneData?,
    val failure: String?,
    val identity: DiagramIdentity?,
    val width: Double,
    val height: Double,
)

internal data class LoadedImage(
    val bitmap: Bitmap?,
    val widthPx: Int,
    val heightPx: Int,
    val intrinsicWidth: Int,
    val intrinsicHeight: Int,
    val reason: String,
)

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

internal data class RecentNote(
    val vault: String,
    val path: String,
    val title: String,
    val openedAt: Long,
)
