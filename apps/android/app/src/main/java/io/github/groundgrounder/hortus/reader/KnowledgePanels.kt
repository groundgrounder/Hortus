// SPDX-License-Identifier: GPL-3.0-only
@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package io.github.groundgrounder.hortus.reader

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
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
import io.github.groundgrounder.hortus.core.TreeEntry

@Composable
internal fun KnowledgeSheet(
    controller: ReaderController,
    state: ReaderUiState,
    knowledge: KnowledgeState,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(
        onDismissRequest = controller::closeKnowledge,
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
                    onClick = { controller.selectKnowledgeTab(KnowledgeTab.OUTLINE) },
                    text = {
                        Text(
                            "${stringResource(R.string.knowledge_outline)} (${knowledge.outline.size})",
                        )
                    },
                )
                Tab(
                    selected = knowledge.tab == KnowledgeTab.BACKLINKS,
                    onClick = { controller.selectKnowledgeTab(KnowledgeTab.BACKLINKS) },
                    text = {
                        Text(
                            "${stringResource(R.string.knowledge_backlinks)} (${knowledge.backlinks.size})",
                        )
                    },
                )
                Tab(
                    selected = knowledge.tab == KnowledgeTab.TAGS,
                    onClick = { controller.selectKnowledgeTab(KnowledgeTab.TAGS) },
                    text = {
                        Text(
                            "${stringResource(R.string.knowledge_tags)} (${knowledge.tags.size})",
                        )
                    },
                )
            }
            KnowledgeTabContent(
                controller = controller,
                state = state,
                knowledge = knowledge,
                scope = "sheet",
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
internal fun SidePane(
    controller: ReaderController,
    state: ReaderUiState,
    knowledge: KnowledgeState,
    modifier: Modifier = Modifier,
) {
    Column(modifier.background(MaterialTheme.colorScheme.surface)) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                SideModeChip(controller, SideMode.FILES, stringResource(R.string.side_files))
                SideModeChip(controller, SideMode.OUTLINE, stringResource(R.string.knowledge_outline))
            }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                SideModeChip(controller, SideMode.BACKLINKS, stringResource(R.string.knowledge_backlinks))
                SideModeChip(controller, SideMode.TAGS, stringResource(R.string.knowledge_tags))
            }
        }
        HorizontalDivider()
        if (controller.sideMode == SideMode.FILES) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                OutlinedButton(onClick = { controller.openNewNoteDialog() }) {
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
                onOpen = controller::openNote,
                modifier = Modifier.fillMaxSize(),
            )
        } else {
            val tab = when (controller.sideMode) {
                SideMode.OUTLINE -> KnowledgeTab.OUTLINE
                SideMode.BACKLINKS -> KnowledgeTab.BACKLINKS
                else -> KnowledgeTab.TAGS
            }
            KnowledgeTabContent(
                controller = controller,
                state = state,
                knowledge = knowledge.copy(tab = tab),
                scope = "side",
                modifier = Modifier.fillMaxSize(),
            )
        }
    }
}

@Composable
private fun SideModeChip(controller: ReaderController, mode: SideMode, label: String) {
    FilterChip(
        selected = controller.sideMode == mode,
        onClick = { controller.selectSideMode(mode) },
        label = { Text(label) },
    )
}

@Composable
internal fun KnowledgeTabContent(
    controller: ReaderController,
    state: ReaderUiState,
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
            KnowledgeTab.OUTLINE -> OutlineList(controller, knowledge, scope)
            KnowledgeTab.BACKLINKS -> BacklinkList(controller, knowledge, scope)
            KnowledgeTab.TAGS -> TagList(controller, knowledge, scope)
        }
    }
}

@Composable
private fun OutlineList(controller: ReaderController, knowledge: KnowledgeState, scope: String) {
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
                    .clickable { controller.openOutline(row.start) }
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
private fun BacklinkList(controller: ReaderController, knowledge: KnowledgeState, scope: String) {
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
                    .clickable { controller.openBacklink(row) }
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
private fun TagList(controller: ReaderController, knowledge: KnowledgeState, scope: String) {
    val selected = knowledge.selectedTag
    if (selected != null) {
        Column(Modifier.fillMaxSize()) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                TextButton(onClick = { controller.selectTag("") }) {
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
                                .clickable { controller.openTagNote(note) }
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
                    .clickable { controller.selectTag(row.tag) }
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

@Composable
internal fun NewNoteDialog(
    controller: ReaderController,
    state: ReaderUiState,
    dialog: NewNoteState,
) {
    val folders = rememberFolders(state.tree)
    AlertDialog(
        onDismissRequest = controller::dismissNewNoteDialog,
        title = { Text(stringResource(R.string.new_note_title)) },
        text = {
            Column {
                Text(
                    stringResource(R.string.new_note_parent),
                    style = MaterialTheme.typography.labelLarge,
                )
                LazyColumn(Modifier.heightIn(max = 180.dp)) {
                    item {
                        FolderRow(
                            label = stringResource(R.string.new_note_root),
                            selected = dialog.parent.isEmpty(),
                            onClick = { controller.selectNewNoteParent("") },
                        )
                    }
                    items(folders, key = { it }) { folder ->
                        FolderRow(
                            label = folder,
                            selected = dialog.parent == folder,
                            onClick = { controller.selectNewNoteParent(folder) },
                        )
                    }
                }
                OutlinedTextField(
                    value = dialog.name,
                    onValueChange = controller::updateNewNoteName,
                    label = { Text(stringResource(R.string.new_note_name)) },
                    singleLine = true,
                    isError = dialog.error != null,
                    modifier = Modifier.fillMaxWidth().padding(top = 10.dp),
                )
                val error = dialog.error
                if (error != null) {
                    Text(
                        newNoteError(error),
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(top = 6.dp),
                    )
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = controller::createNewNote,
                enabled = !dialog.creating,
            ) {
                if (dialog.creating) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                } else {
                    Text(stringResource(R.string.new_note_create))
                }
            }
        },
        dismissButton = {
            TextButton(onClick = controller::dismissNewNoteDialog) {
                Text(stringResource(R.string.reader_cancel))
            }
        },
    )
}

@Composable
private fun FolderRow(label: String, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(vertical = 8.dp, horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            label,
            style = MaterialTheme.typography.bodyMedium,
            color = if (selected) {
                MaterialTheme.colorScheme.primary
            } else {
                MaterialTheme.colorScheme.onSurface
            },
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (selected) {
            Text(
                "✓",
                color = MaterialTheme.colorScheme.primary,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

@Composable
private fun newNoteError(code: String): String = when (code) {
    "vault.already_exists" -> stringResource(R.string.new_note_exists)
    "vault.invalid_name" -> stringResource(R.string.new_note_invalid)
    "vault.not_found" -> stringResource(R.string.new_note_folder_missing)
    else -> stringResource(R.string.new_note_failed)
}

private fun rememberFolders(tree: List<TreeEntry>): List<String> =
    tree.filter { it.directory }.map { it.path }.sorted()
