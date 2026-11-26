// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.TreeEntry
import io.github.groundgrounder.hortus.reader.model.NewNoteState
import io.github.groundgrounder.hortus.reader.model.ReaderUiState
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel

@Composable
internal fun NewNoteDialog(
    viewModel: ReaderViewModel,
    state: ReaderUiState,
    dialog: NewNoteState,
) {
    val folders = remember(state.tree) { rememberFolders(state.tree) }
    AlertDialog(
        onDismissRequest = viewModel::dismissNewNoteDialog,
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
                            onClick = { viewModel.selectNewNoteParent("") },
                        )
                    }
                    items(folders, key = { it }) { folder ->
                        FolderRow(
                            label = folder,
                            selected = dialog.parent == folder,
                            onClick = { viewModel.selectNewNoteParent(folder) },
                        )
                    }
                }
                OutlinedTextField(
                    value = dialog.name,
                    onValueChange = viewModel::updateNewNoteName,
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
                onClick = viewModel::createNewNote,
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
            TextButton(onClick = viewModel::dismissNewNoteDialog) {
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
