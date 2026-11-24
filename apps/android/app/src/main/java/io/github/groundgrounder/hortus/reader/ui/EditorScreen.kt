// SPDX-License-Identifier: GPL-3.0-only
@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package io.github.groundgrounder.hortus.reader.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.reader.model.EditState
import io.github.groundgrounder.hortus.reader.model.ReaderUiState
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel

@Composable
internal fun EditorScreen(
    viewModel: ReaderViewModel,
    state: ReaderUiState,
    edit: EditState,
) {
    BackHandler { viewModel.finishEditing() }
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        state.content?.title ?: stringResource(R.string.app_name),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                navigationIcon = {
                    IconButton(onClick = viewModel::finishEditing) {
                        Icon(
                            Icons.Default.Check,
                            contentDescription = stringResource(R.string.editor_done),
                        )
                    }
                },
                actions = {
                    TextButton(
                        onClick = viewModel::saveEditedNote,
                        enabled = !edit.saving && edit.dirty,
                    ) {
                        Text(stringResource(R.string.editor_save))
                    }
                },
            )
        },
    ) { insets ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(insets)
                .imePadding(),
        ) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.surfaceVariant)
                    .padding(horizontal = 16.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (edit.saving) {
                    CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 2.dp)
                }
                Text(
                    when {
                        edit.saving -> stringResource(R.string.editor_saving)
                        edit.dirty -> stringResource(R.string.editor_unsaved)
                        else -> stringResource(R.string.editor_saved)
                    },
                    style = MaterialTheme.typography.labelMedium,
                    color = if (edit.dirty) {
                        MaterialTheme.colorScheme.error
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                )
                Spacer(Modifier.weight(1f))
                Text(
                    "${edit.field.text.length}",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            BasicTextField(
                value = edit.field,
                onValueChange = viewModel::updateEditText,
                textStyle = TextStyle(
                    fontFamily = FontFamily.Monospace,
                    fontSize = 16.sp,
                    lineHeight = 24.sp,
                    color = MaterialTheme.colorScheme.onSurface,
                ),
                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                modifier = Modifier
                    .fillMaxSize()
                    .padding(horizontal = 16.dp, vertical = 12.dp),
            )
        }
    }
}

@Composable
internal fun ConflictDialog(
    viewModel: ReaderViewModel,
    edit: EditState,
) {
    if (!edit.active) return
    val issue = edit.issue
    if (issue != "conflict" && issue != "missing") return
    val missing = issue == "missing"
    AlertDialog(
        onDismissRequest = {},
        title = {
            Text(
                stringResource(
                    if (missing) R.string.editor_missing_title else R.string.editor_conflict_title,
                ),
            )
        },
        text = {
            Column {
                Text(
                    stringResource(
                        if (missing) R.string.editor_missing_body else R.string.editor_conflict_body,
                    ),
                )
                if (!missing) {
                    TextButton(
                        onClick = viewModel::useDiskVersion,
                        enabled = !edit.saving,
                    ) {
                        Text(stringResource(R.string.editor_use_disk))
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = viewModel::keepMine, enabled = !edit.saving) {
                Text(stringResource(R.string.editor_keep_mine))
            }
        },
        dismissButton = {
            TextButton(onClick = viewModel::saveConflictCopy, enabled = !edit.saving) {
                Text(stringResource(R.string.editor_copy_action))
            }
        },
    )
}
