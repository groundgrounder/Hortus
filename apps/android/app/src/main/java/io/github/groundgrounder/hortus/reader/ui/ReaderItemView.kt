// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.ui

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
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.model.DiagramItem
import io.github.groundgrounder.hortus.core.model.EmbedItem
import io.github.groundgrounder.hortus.core.model.EmbeddedContent
import io.github.groundgrounder.hortus.core.model.ImageItem
import io.github.groundgrounder.hortus.core.model.MathItem
import io.github.groundgrounder.hortus.core.model.ReaderItem
import io.github.groundgrounder.hortus.core.model.TableItem
import io.github.groundgrounder.hortus.core.model.TextItem
import io.github.groundgrounder.hortus.core.model.embedDiagramType
import io.github.groundgrounder.hortus.reader.model.ItemRenderBridge
import io.github.groundgrounder.hortus.reader.model.ReaderCallbacks
import io.github.groundgrounder.hortus.reader.model.ReaderPalette

@Composable
internal fun ReaderItemView(
    item: ReaderItem,
    engineText: String,
    contentWidth: Int,
    bodySizePx: Float,
    dark: Boolean,
    palette: ReaderPalette,
    renderBridge: ItemRenderBridge,
    vaultPath: String,
    notePath: String,
    revision: Long,
    embedVersion: Long,
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
            renderBridge = renderBridge,
            notePath = notePath,
            onLink = callbacks.onLink,
            onTag = callbacks.onTag,
            modifier = modifier,
        )
        is TableItem -> TableBlockView(
            item.media,
            contentWidth = contentWidth,
            bodySizePx = bodySizePx,
            palette = palette,
            renderBridge = renderBridge,
            notePath = notePath,
            modifier = modifier,
        )
        is ImageItem -> ImageBlockView(
            item.media,
            contentWidth = contentWidth,
            palette = palette,
            renderBridge = renderBridge,
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
            renderBridge = renderBridge,
            modifier = modifier,
        )
        is DiagramItem -> DiagramBlock(
            item = item,
            engineText = engineText,
            contentWidth = contentWidth,
            bodySizePx = bodySizePx,
            dark = dark,
            palette = palette,
            renderBridge = renderBridge,
            vaultPath = vaultPath,
            notePath = notePath,
            revision = revision,
            onFocus = callbacks.onFocus,
            modifier = modifier,
        )
        is EmbedItem -> EmbedBlockView(
            item = item,
            vaultPath = vaultPath,
            notePath = notePath,
            revision = revision,
            embedVersion = embedVersion,
            ancestors = ancestors,
            contentWidth = contentWidth,
            bodySizePx = bodySizePx,
            dark = dark,
            palette = palette,
            renderBridge = renderBridge,
            callbacks = callbacks,
            modifier = modifier,
        )
    }
}

@Composable
internal fun EmbedBlockView(
    item: EmbedItem,
    vaultPath: String,
    notePath: String,
    revision: Long,
    embedVersion: Long,
    ancestors: List<String>,
    contentWidth: Int,
    bodySizePx: Float,
    dark: Boolean,
    palette: ReaderPalette,
    renderBridge: ItemRenderBridge,
    callbacks: ReaderCallbacks,
    modifier: Modifier = Modifier,
) {
    var embedded by remember(vaultPath, item.key, notePath, contentWidth, revision) {
        mutableStateOf<EmbeddedContent?>(null)
    }
    LaunchedEffect(vaultPath, item.key, notePath, ancestors, contentWidth, bodySizePx, revision, embedVersion) {
        embedded = renderBridge.resolveEmbed(
            sourcePath = notePath,
            target = item.target,
            heading = item.heading,
            alias = item.alias,
            ancestors = ancestors,
            width = contentWidth.toDouble(),
            size = bodySizePx.toDouble(),
        )
    }
    val content = embedded
    val info = content?.info
    if (info != null && DebugProbes.enabled) {
        DebugProbes.recordEmbed("$notePath|${item.target}|${item.heading}", info)
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
                            renderBridge = renderBridge,
                            vaultPath = vaultPath,
                            notePath = info.resolvedPath ?: notePath,
                            revision = revision + embedVersion,
                            embedVersion = embedVersion,
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
                    bodySizePx = bodySizePx * 0.94f,
                    dark = dark,
                    palette = palette,
                    renderBridge = renderBridge,
                    vaultPath = vaultPath,
                    notePath = info.resolvedPath ?: notePath,
                    revision = revision + embedVersion,
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
