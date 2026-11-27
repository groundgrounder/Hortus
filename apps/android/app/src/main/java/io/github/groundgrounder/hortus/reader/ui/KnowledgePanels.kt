// SPDX-License-Identifier: AGPL-3.0-only
@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package io.github.groundgrounder.hortus.reader.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.DebugProbes.debugPanel
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.reader.model.KnowledgeState
import io.github.groundgrounder.hortus.reader.model.KnowledgeTab
import io.github.groundgrounder.hortus.reader.model.ReaderUiState
import io.github.groundgrounder.hortus.reader.model.SideMode
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel

@Composable
internal fun KnowledgeSheet(
    viewModel: ReaderViewModel,
    knowledge: KnowledgeState,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(
        onDismissRequest = viewModel::closeKnowledge,
        sheetState = sheetState,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 320.dp, max = 580.dp)
                .padding(bottom = 12.dp),
        ) {
            TabRow(selectedTabIndex = knowledge.tab.ordinal) {
                Tab(
                    selected = knowledge.tab == KnowledgeTab.OUTLINE,
                    onClick = { viewModel.selectKnowledgeTab(KnowledgeTab.OUTLINE) },
                    text = {
                        Text(
                            "${stringResource(R.string.knowledge_outline)} (${knowledge.outline.size})",
                        )
                    },
                )
                Tab(
                    selected = knowledge.tab == KnowledgeTab.BACKLINKS,
                    onClick = { viewModel.selectKnowledgeTab(KnowledgeTab.BACKLINKS) },
                    text = {
                        Text(
                            "${stringResource(R.string.knowledge_backlinks)} (${knowledge.backlinks.size})",
                        )
                    },
                )
                Tab(
                    selected = knowledge.tab == KnowledgeTab.TAGS,
                    onClick = { viewModel.selectKnowledgeTab(KnowledgeTab.TAGS) },
                    text = {
                        Text(
                            "${stringResource(R.string.knowledge_tags)} (${knowledge.tags.size})",
                        )
                    },
                )
            }
            KnowledgeTabContent(
                viewModel = viewModel,
                knowledge = knowledge,
                scope = "sheet",
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
internal fun SidePane(
    viewModel: ReaderViewModel,
    state: ReaderUiState,
    knowledge: KnowledgeState,
    sideMode: SideMode,
    modifier: Modifier = Modifier,
) {
    Column(modifier.background(MaterialTheme.colorScheme.surface)) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                SideModeChip(sideMode, SideMode.FILES, stringResource(R.string.side_files), viewModel::selectSideMode)
                SideModeChip(sideMode, SideMode.OUTLINE, stringResource(R.string.knowledge_outline), viewModel::selectSideMode)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                SideModeChip(sideMode, SideMode.BACKLINKS, stringResource(R.string.knowledge_backlinks), viewModel::selectSideMode)
                SideModeChip(sideMode, SideMode.TAGS, stringResource(R.string.knowledge_tags), viewModel::selectSideMode)
            }
        }
        HorizontalDivider()
        if (sideMode == SideMode.FILES) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                OutlinedButton(onClick = { viewModel.openNewNoteDialog() }) {
                    Text(stringResource(R.string.new_note))
                }
                Spacer(Modifier.weight(1f))
                Text(
                    state.vaultPath?.substringAfterLast('/') ?: "",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            FileTreeList(
                entries = state.tree,
                currentPath = state.content?.path,
                onOpen = viewModel::openNote,
                modifier = Modifier.fillMaxSize(),
            )
        } else {
            val tab = when (sideMode) {
                SideMode.OUTLINE -> KnowledgeTab.OUTLINE
                SideMode.BACKLINKS -> KnowledgeTab.BACKLINKS
                else -> KnowledgeTab.TAGS
            }
            KnowledgeTabContent(
                viewModel = viewModel,
                knowledge = knowledge.copy(tab = tab),
                scope = "side",
                modifier = Modifier.fillMaxSize(),
            )
        }
    }
}

@Composable
private fun SideModeChip(
    current: SideMode,
    mode: SideMode,
    label: String,
    onSelect: (SideMode) -> Unit,
) {
    FilterChip(
        selected = current == mode,
        onClick = { onSelect(mode) },
        label = { Text(label) },
    )
}

@Composable
internal fun KnowledgeTabContent(
    viewModel: ReaderViewModel,
    knowledge: KnowledgeState,
    scope: String,
    modifier: Modifier = Modifier,
) {
    Box(modifier) {
        if (knowledge.loading && knowledge.tab == KnowledgeTab.OUTLINE &&
            knowledge.outline.isEmpty()
        ) {
            Row(Modifier.padding(20.dp), verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                Text(
                    stringResource(R.string.knowledge_loading),
                    modifier = Modifier.padding(start = 10.dp),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        when (knowledge.tab) {
            KnowledgeTab.OUTLINE -> OutlineList(viewModel, knowledge, scope)
            KnowledgeTab.BACKLINKS -> BacklinkList(viewModel, knowledge, scope)
            KnowledgeTab.TAGS -> TagList(viewModel, knowledge, scope)
        }
    }
}

@Composable
private fun OutlineList(viewModel: ReaderViewModel, knowledge: KnowledgeState, scope: String) {
    val rows = knowledge.outline
    if (DebugProbes.enabled) {
        LaunchedEffect(rows) { DebugProbes.retainPanels(scope, rows.map { "outline:${it.start}" }) }
        DisposableEffect(scope) { onDispose { DebugProbes.clearPanels(scope) } }
    }
    if (rows.isEmpty()) {
        EmptyRow(stringResource(R.string.knowledge_empty_outline))
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(rows, key = { it.start }) { row ->
            Text(
                row.title,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { viewModel.openOutline(row.start) }
                    .padding(
                        start = (16 + (row.level - 1).coerceAtLeast(0) * 16).dp,
                        end = 16.dp,
                        top = 9.dp,
                        bottom = 9.dp,
                    )
                    .debugPanel(
                        scope = scope,
                        key = "outline:${row.start}",
                        kind = "outline",
                        label = row.title,
                        target = row.start.toString(),
                        tab = "outline",
                    ),
            )
        }
    }
}

@Composable
private fun BacklinkList(viewModel: ReaderViewModel, knowledge: KnowledgeState, scope: String) {
    val rows = knowledge.backlinks
    if (DebugProbes.enabled) {
        LaunchedEffect(rows) { DebugProbes.retainPanels(scope, rows.map { "backlink:${it.source}:${it.startUtf16}" }) }
        DisposableEffect(scope) { onDispose { DebugProbes.clearPanels(scope) } }
    }
    if (rows.isEmpty()) {
        EmptyRow(stringResource(R.string.knowledge_empty_backlinks))
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(rows, key = { "${it.source}:${it.startUtf16}" }) { row ->
            Column(
                Modifier
                    .fillMaxWidth()
                    .clickable { viewModel.openBacklink(row) }
                    .padding(horizontal = 16.dp, vertical = 9.dp)
                    .debugPanel(
                        scope = scope,
                        key = "backlink:${row.source}:${row.startUtf16}",
                        kind = "backlink",
                        label = row.title,
                        target = row.source,
                        tab = "backlinks",
                    ),
            ) {
                Text(
                    row.title,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    "${row.source} · ${stringResource(R.string.knowledge_line, row.line)}",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (row.context.isNotBlank()) {
                    Text(
                        row.context,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                }
                val badges = ArrayList<String>(2)
                if (row.inDiagram) badges.add(stringResource(R.string.knowledge_in_diagram))
                if (row.embed) badges.add(stringResource(R.string.knowledge_embed))
                if (badges.isNotEmpty()) {
                    Text(
                        badges.joinToString(" · "),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun TagList(viewModel: ReaderViewModel, knowledge: KnowledgeState, scope: String) {
    val selected = knowledge.selectedTag
    if (selected != null) {
        Column(Modifier.fillMaxSize()) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                TextButton(onClick = { viewModel.selectTag("") }) {
                    Text(stringResource(R.string.knowledge_back_to_tags))
                }
                Text(
                    "#$selected",
                    style = MaterialTheme.typography.titleSmall,
                    color = MaterialTheme.colorScheme.primary,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (knowledge.tagNotes.isEmpty()) {
                EmptyRow(stringResource(R.string.knowledge_empty_tags))
            } else {
                LazyColumn(Modifier.fillMaxSize()) {
                    items(knowledge.tagNotes, key = { it.path }) { note ->
                        Column(
                            Modifier
                                .fillMaxWidth()
                                .clickable { viewModel.openTagNote(note) }
                                .padding(horizontal = 16.dp, vertical = 9.dp)
                                .debugPanel(
                                    scope = scope,
                                    key = "tag-note:${note.path}",
                                    kind = "tag-note",
                                    label = note.title,
                                    target = note.path,
                                    tab = "tags",
                                ),
                        ) {
                            Text(note.title, style = MaterialTheme.typography.bodyMedium)
                            Text(
                                note.path,
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                }
            }
        }
        return
    }
    val rows = knowledge.tags
    if (DebugProbes.enabled) {
        LaunchedEffect(rows) { DebugProbes.retainPanels(scope, rows.map { "tag:${it.tag}" }) }
        DisposableEffect(scope) { onDispose { DebugProbes.clearPanels(scope) } }
    }
    if (rows.isEmpty()) {
        EmptyRow(stringResource(R.string.knowledge_empty_tags))
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(rows, key = { it.tag }) { row ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable { viewModel.selectTag(row.tag) }
                    .padding(horizontal = 16.dp, vertical = 10.dp)
                    .debugPanel(
                        scope = scope,
                        key = "tag:${row.tag}",
                        kind = "tag",
                        label = row.tag,
                        target = row.tag,
                        tab = "tags",
                    ),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    "#${row.tag}",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.primary,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    stringResource(R.string.knowledge_tag_count, row.count),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun EmptyRow(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(20.dp),
    )
}
