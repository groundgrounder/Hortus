// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.SceneData
import androidx.compose.ui.res.stringResource
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import androidx.compose.ui.platform.LocalDensity

internal data class ReaderCallbacks(
    val onLink: (TextLink) -> Unit,
    val onOpenPath: (String, String?) -> Unit,
    val onFocus: (DiagramItem, SceneData, List<String>) -> Unit,
    val onNotice: (String) -> Unit,
)

@Composable
internal fun ReaderItemView(
    item: ReaderItem,
    engineText: String,
    contentWidth: Int,
    bodySizePx: Float,
    dark: Boolean,
    palette: ReaderPalette,
    repository: ReaderRepository,
    notePath: String,
    ancestors: List<String>,
    callbacks: ReaderCallbacks,
    modifier: Modifier = Modifier,
) {
    when (item) {
        is TextItem -> TextBlockView(
            item,
            contentWidth = contentWidth,
            bodySizePx = bodySizePx,
            palette = palette,
            repository = repository,
            notePath = notePath,
            onLink = callbacks.onLink,
            modifier = modifier,
        )
        is TableItem -> TableBlockView(
            item.media,
            contentWidth = contentWidth,
            bodySizePx = bodySizePx,
            palette = palette,
            repository = repository,
            notePath = notePath,
            modifier = modifier,
        )
        is ImageItem -> ImageBlockView(
            item.media,
            contentWidth = contentWidth,
            palette = palette,
            modifier = modifier,
        )
        is MathItem -> MathBlockView(
            item.formulas,
            itemKey = item.key,
            blockId = item.blockId,
            notePath = notePath,
            contentWidth = contentWidth,
            bodySizePx = bodySizePx,
            palette = palette,
            repository = repository,
            modifier = modifier,
        )
        is DiagramItem -> DiagramBlock(
            item = item,
            engineText = engineText,
            contentWidth = contentWidth,
            dark = dark,
            palette = palette,
            repository = repository,
            notePath = notePath,
            onFocus = callbacks.onFocus,
            modifier = modifier,
        )
        is EmbedItem -> EmbedBlockView(
            item = item,
            notePath = notePath,
            ancestors = ancestors,
            contentWidth = contentWidth,
            bodySizePx = bodySizePx,
            dark = dark,
            palette = palette,
            repository = repository,
            callbacks = callbacks,
            modifier = modifier,
        )
    }
}

@Composable
internal fun EmbedBlockView(
    item: EmbedItem,
    notePath: String,
    ancestors: List<String>,
    contentWidth: Int,
    bodySizePx: Float,
    dark: Boolean,
    palette: ReaderPalette,
    repository: ReaderRepository,
    callbacks: ReaderCallbacks,
    modifier: Modifier = Modifier,
) {
    var embedded by remember(item.key, notePath, contentWidth) { mutableStateOf<EmbeddedContent?>(null) }
    LaunchedEffect(item.key, notePath, ancestors, contentWidth, bodySizePx) {
        embedded = withContext(Dispatchers.IO) {
            repository.resolveEmbed(
                sourcePath = notePath,
                target = item.target,
                heading = item.heading,
                alias = item.alias,
                ancestors = ancestors,
                width = contentWidth.toDouble(),
                size = bodySizePx.toDouble(),
            )
        }
    }
    val content = embedded
    val info = content?.info
    if (info != null) {
        Inspection.recordEmbed("$notePath|${item.target}|${item.heading}", info)
    }
    val density = LocalDensity.current.density
    val cardShape = RoundedCornerShape(10.dp)
    Column(
        modifier
            .fillMaxWidth()
            .padding(vertical = 6.dp)
            .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.72f), cardShape)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, cardShape)
            .padding(12.dp),
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .clickable(enabled = info?.resolvedPath != null) {
                    info?.resolvedPath?.let { callbacks.onOpenPath(it, info.heading) }
                },
        ) {
            Text(
                info?.displayTitle?.takeIf { it.isNotBlank() }
                    ?: item.alias
                    ?: item.target,
                style = MaterialTheme.typography.titleSmall,
                color = palette.accent,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            if (info != null && info.kind != "note" && info.kind != "diagram") {
                Text(
                    info.kind,
                    style = MaterialTheme.typography.labelSmall,
                    color = palette.muted,
                )
            }
        }
        if (info == null) {
            Text(
                stringResource(R.string.embed_loading),
                style = MaterialTheme.typography.bodySmall,
                color = palette.muted,
                modifier = Modifier.padding(top = 6.dp),
            )
            return@Column
        }
        when (info.kind) {
            "note" -> {
                val inner = content
                if (inner != null) {
                    val innerWidth = (contentWidth - (24 * density).toInt()).coerceAtLeast(1)
                    val nextAncestors = ancestors + notePath + (info.resolvedPath ?: item.target)
                    for (child in inner.items) {
                        ReaderItemView(
                            item = child,
                            engineText = info.content,
                            contentWidth = innerWidth,
                            bodySizePx = bodySizePx * 0.94f,
                            dark = dark,
                            palette = palette,
                            repository = repository,
                            notePath = info.resolvedPath ?: notePath,
                            ancestors = nextAncestors,
                            callbacks = callbacks,
                        )
                    }
                }
            }
            "diagram" -> {
                val engine = info.content
                val ordinal = 1
                val diagram = DiagramItem(
                    key = "${item.key}-diagram",
                    blockId = item.blockId,
                    sourceStart = 0,
                    sourceLength = engine.length,
                    ordinal = ordinal,
                    displayOrdinal = 1,
                    source = engine,
                    engineText = engine,
                    mmd = info.isMmd,
                    diagramType = embedDiagramType(info, info.diagrams.firstOrNull()),
                    documentKey = info.resolvedPath ?: item.target,
                    isEmbedded = true,
                )
                DiagramBlock(
                    item = diagram,
                    engineText = engine,
                    contentWidth = (contentWidth - (24 * density).toInt()).coerceAtLeast(1),
                    dark = dark,
                    palette = palette,
                    repository = repository,
                    notePath = info.resolvedPath ?: notePath,
                    onFocus = callbacks.onFocus,
                )
            }
            "cycle" -> EmbedMessage(R.string.embed_cycle, info.displayTitle, palette)
            "depth_exceeded" -> EmbedMessage(R.string.embed_depth, info.displayTitle, palette)
            "heading_missing" -> EmbedMessage(R.string.embed_heading_missing, info.displayTitle, palette)
            "ambiguous" -> EmbedMessage(
                R.string.embed_ambiguous,
                info.candidates.joinToString(", ").ifBlank { info.displayTitle },
                palette,
            )
            "broken" -> EmbedMessage(R.string.embed_broken, info.displayTitle, palette)
            "attachment" -> EmbedMessage(R.string.embed_attachment, info.displayTitle, palette)
            else -> EmbedMessage(R.string.embed_error, info.displayTitle, palette)
        }
    }
}

@Composable
private fun EmbedMessage(resource: Int, detail: String, palette: ReaderPalette) {
    Text(
        stringResource(resource, detail),
        style = MaterialTheme.typography.bodySmall,
        color = palette.muted,
        modifier = Modifier.padding(top = 6.dp),
    )
}
