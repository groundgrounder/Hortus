// SPDX-License-Identifier: GPL-3.0-only
@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package io.github.groundgrounder.hortus.reader.ui

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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.app.IntentRouter
import io.github.groundgrounder.hortus.reader.model.ReaderUiState
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

@Composable
internal fun HomeScreen(
    viewModel: ReaderViewModel,
    hasAccess: Boolean,
    onRequestPermission: () -> Unit,
    state: ReaderUiState,
) {
    val preferences by viewModel.preferencesFlow.collectAsStateWithLifecycle()
    var path by remember {
        mutableStateOf(preferences.vaultPath?.takeIf(IntentRouter::isValidVaultDirectory) ?: "")
    }
    LaunchedEffect(preferences.vaultPath) {
        val saved = preferences.vaultPath
        if (path.isBlank() && saved != null && IntentRouter.isValidVaultDirectory(saved)) {
            path = saved
        }
    }
    var pickerOpen by remember { mutableStateOf(false) }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.app_name)) },
                actions = {
                    IconButton(onClick = viewModel::openSettings) {
                        Icon(
                            Icons.Default.Settings,
                            contentDescription = stringResource(R.string.settings_title),
                        )
                    }
                },
            )
        },
    ) { insets ->
        LazyColumn(
            Modifier.fillMaxSize().padding(insets).padding(horizontal = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item { Spacer(Modifier.height(16.dp)) }
            if (!hasAccess) {
                item {
                    Text(stringResource(R.string.permission_title), style = MaterialTheme.typography.titleLarge)
                    Text(stringResource(R.string.permission_body), modifier = Modifier.padding(top = 6.dp))
                    Button(onClick = onRequestPermission, modifier = Modifier.padding(top = 10.dp)) {
                        Text(stringResource(R.string.permission_grant_action))
                    }
                }
            } else {
                item {
                    val trimmed = path.trim()
                    val candidate = File(trimmed)
                    val isRoot = trimmed.isNotEmpty() && candidate.isAbsolute && candidate.isDirectory &&
                        IntentRouter.isStorageRoot(candidate)
                    OutlinedTextField(
                        value = path,
                        onValueChange = { path = it },
                        label = { Text(stringResource(R.string.reader_folder_path)) },
                        isError = isRoot,
                        supportingText = if (isRoot) {
                            { Text(stringResource(R.string.vault_root_forbidden)) }
                        } else {
                            null
                        },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                item {
                    val trimmed = path.trim()
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Button(
                            onClick = { viewModel.start(trimmed, null) },
                            enabled = IntentRouter.isValidVaultDirectory(trimmed),
                        ) { Text(stringResource(R.string.reader_open)) }
                        OutlinedButton(onClick = { pickerOpen = true }) {
                            Text(stringResource(R.string.reader_open_folder))
                        }
                    }
                }
                if (state.loading) {
                    item {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                            Text(stringResource(R.string.reader_loading), modifier = Modifier.padding(start = 10.dp))
                        }
                    }
                }
                if (state.error != null) {
                    item {
                        Text(
                            state.error,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                }
                if (state.vaultPath != null) {
                    item {
                        Text(stringResource(R.string.reader_notes), style = MaterialTheme.typography.titleMedium)
                    }
                    item {
                        Box(Modifier.heightIn(max = 560.dp)) {
                            FileTreeList(
                                entries = state.tree,
                                currentPath = null,
                                onOpen = { note -> viewModel.openNote(note) },
                            )
                        }
                    }
                }
            }
        }
    }
    if (pickerOpen) {
        VaultPickerDialog(
            onDismiss = { pickerOpen = false },
            onPick = { chosen ->
                pickerOpen = false
                path = chosen
                viewModel.start(chosen, null)
            },
        )
    }
}

@Composable
private fun VaultPickerDialog(onDismiss: () -> Unit, onPick: (String) -> Unit) {
    var directory by remember { mutableStateOf(File("/storage/emulated/0")) }
    var children by remember { mutableStateOf<List<File>>(emptyList()) }
    LaunchedEffect(directory) {
        children = withContext(Dispatchers.IO) {
            try {
                directory.listFiles()
                    ?.filter { it.isDirectory && !it.name.startsWith(".") }
                    ?.sortedBy { it.name.lowercase() }
                    ?: emptyList()
            } catch (_: Exception) {
                emptyList()
            }
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.reader_open_folder)) },
        text = {
            Column {
                Text(
                    directory.path,
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    TextButton(
                        onClick = { directory.parentFile?.let { directory = it } },
                        enabled = directory.parentFile != null,
                    ) { Text(stringResource(R.string.reader_folder_up)) }
                }
                LazyColumn(Modifier.heightIn(max = 340.dp)) {
                    items(children, key = { it.path }) { child ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .clickable { directory = child }
                                .padding(vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                child.name,
                                style = MaterialTheme.typography.bodyLarge,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            val valid = IntentRouter.isValidVaultDirectory(directory.path)
            TextButton(
                onClick = { onPick(directory.path) },
                enabled = valid,
            ) {
                Text(stringResource(R.string.reader_use_folder))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(stringResource(R.string.reader_cancel))
            }
        },
    )
}
