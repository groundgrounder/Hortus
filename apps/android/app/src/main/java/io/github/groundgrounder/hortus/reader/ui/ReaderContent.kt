// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.ui

import android.app.Activity
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.DebugProbes.debugItemBounds
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.model.DiagramItem
import io.github.groundgrounder.hortus.core.model.NoteContent
import io.github.groundgrounder.hortus.reader.model.ReaderCallbacks
import io.github.groundgrounder.hortus.reader.model.ReaderPalette
import io.github.groundgrounder.hortus.reader.model.ReaderUiState
import io.github.groundgrounder.hortus.reader.model.ScrollTarget
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.first

@Composable
internal fun ReaderDetail(
    viewModel: ReaderViewModel,
    state: ReaderUiState,
    bodySizePx: Float,
    callbacks: ReaderCallbacks,
    palette: ReaderPalette,
    dark: Boolean,
    modifier: Modifier = Modifier,
) {
    Column(modifier) {
        val content = state.content
        if (content?.readOnlyReason != null) {
            ReadOnlyBanner(content)
        }
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
            val density = LocalDensity.current
            val contentWidth = with(density) {
                (maxWidth - 48.dp).roundToPx()
            }.coerceAtLeast(1)
            val bottomPadding = maxHeight + 72.dp
            ReaderContent(
                viewModel = viewModel,
                content = content,
                loading = state.loading,
                scrollTarget = state.scrollTarget,
                openSequence = state.openSequence,
                embedVersion = state.embedVersion,
                bodySizePx = bodySizePx,
                dark = dark,
                palette = palette,
                callbacks = callbacks,
                contentWidthPx = contentWidth,
                bottomPadding = bottomPadding,
            )
        }
    }
}

@Composable
private fun ReadOnlyBanner(content: NoteContent) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .padding(horizontal = 20.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            if (content.externalPreview) {
                stringResource(R.string.reader_external_preview)
            } else {
                stringResource(
                    R.string.reader_read_only_reason,
                    content.readOnlyReason ?: "",
                )
            },
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ReaderContent(
    viewModel: ReaderViewModel,
    content: NoteContent?,
    loading: Boolean,
    scrollTarget: ScrollTarget?,
    openSequence: Long,
    embedVersion: Long,
    bodySizePx: Float,
    dark: Boolean,
    palette: ReaderPalette,
    callbacks: ReaderCallbacks,
    contentWidthPx: Int,
    bottomPadding: Dp,
) {
    if (content == null) return
    LaunchedEffect(contentWidthPx) { viewModel.setContentWidth(contentWidthPx) }
    val initialTarget = remember(content.path) { scrollTarget ?: viewModel.restoreTarget(content) }
    val listState = remember(content.path) {
        LazyListState(
            firstVisibleItemIndex = initialTarget?.itemIndex?.coerceAtLeast(0) ?: 0,
            firstVisibleItemScrollOffset = initialTarget?.offsetPx?.coerceAtLeast(0) ?: 0,
        )
    }
    LaunchedEffect(scrollTarget, content.items.size) {
        val target = scrollTarget ?: return@LaunchedEffect
        val last = content.items.lastIndex.coerceAtLeast(0)
        val index = target.itemIndex.coerceIn(0, last)
        for (attempt in 0 until 8) {
            delay(80)
            if (index > 0) {
                snapshotFlow { listState.layoutInfo.totalItemsCount }
                    .first { it > index }
            }
            listState.scrollToItem(index, target.offsetPx)
            if (listState.firstVisibleItemIndex == index) break
        }
        viewModel.consumeScrollTarget()
    }
    LaunchedEffect(listState, content) {
        snapshotFlow {
            if (listState.layoutInfo.visibleItemsInfo.isEmpty()) null
            else listState.firstVisibleItemIndex to listState.firstVisibleItemScrollOffset
        }.collect { value ->
            if (value != null) {
                content.items.getOrNull(value.first)?.let { item ->
                    viewModel.updateAnchor(item.blockId, value.second)
                }
            }
        }
    }
    if (DebugProbes.enabled) {
        LaunchedEffect(listState, content) {
            snapshotFlow {
                listState.layoutInfo.visibleItemsInfo.count { info ->
                    content.items.getOrNull(info.index) is DiagramItem
                }
            }.collectLatest { count ->
                DebugProbes.setVisibleDiagramCount(count)
            }
        }
    }
    val activity = LocalContext.current as? Activity
    LaunchedEffect(openSequence, content.revision) {
        withFrameNanos { }
        viewModel.markFirstFrame()
        // reportFullyDrawn 在独立 UI 消息里调用，随后再排一帧，Macrobenchmark
        // 才能把这次上报关联到之后的 RenderThread 帧切片。
        withFrameNanos { }
        val decor = activity?.window?.decorView ?: return@LaunchedEffect
        decor.post {
            viewModel.markFullyDrawn()
            activity.reportFullyDrawn()
            decor.invalidate()
        }
    }
    val ancestors = remember(content.path) { listOf(content.path) }
    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            state = listState,
            modifier = Modifier.fillMaxSize().padding(horizontal = 24.dp),
            contentPadding = PaddingValues(bottom = bottomPadding),
        ) {
            itemsIndexed(
                content.items,
                key = { _, item -> item.key },
            ) { _, item ->
                ReaderItemView(
                    item = item,
                    engineText = content.sourceText,
                    contentWidth = contentWidthPx,
                    bodySizePx = bodySizePx,
                    dark = dark,
                    palette = palette,
                    renderBridge = viewModel.renderBridge,
                    vaultPath = content.vaultPath,
                    notePath = content.path,
                    revision = content.revision,
                    embedVersion = embedVersion,
                    ancestors = ancestors,
                    callbacks = callbacks,
                    modifier = Modifier
                        .padding(vertical = 1.dp)
                        .debugItemBounds(item.key, item.blockId),
                )
            }
        }
        if (loading) {
            CircularProgressIndicator(Modifier.align(Alignment.TopCenter).padding(8.dp).size(20.dp))
        }
    }
}
