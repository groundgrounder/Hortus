// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.FoundationResult
import io.github.groundgrounder.hortus.NotePreview
import io.github.groundgrounder.hortus.R
import org.json.JSONObject
import java.io.File

@Composable
internal fun FoundationScreen(
    hasAccess: Boolean,
    grantAccess: () -> Unit,
    path: String,
    setPath: (String) -> Unit,
    running: Boolean,
    failed: Boolean,
    result: FoundationResult?,
    preview: NotePreview?,
    open: () -> Unit,
    testSave: () -> Unit,
    openNote: (String) -> Unit,
    closePreview: () -> Unit,
) {
    Scaffold { insets ->
        LazyColumn(
            modifier = Modifier.padding(insets).fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            item {
                Text(
                    stringResource(R.string.app_name),
                    style = MaterialTheme.typography.headlineLarge,
                    modifier = Modifier.padding(start = 24.dp, top = 22.dp),
                )
            }
            if (!hasAccess) {
                item {
                    Column(
                        Modifier.padding(horizontal = 24.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Text(stringResource(R.string.permission_title), style = MaterialTheme.typography.titleLarge)
                        Text(stringResource(R.string.permission_body))
                        Button(onClick = grantAccess) { Text(stringResource(R.string.permission_grant_action)) }
                    }
                }
            } else {
                item {
                    Column(
                        Modifier.padding(horizontal = 24.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        OutlinedTextField(
                            value = path,
                            onValueChange = setPath,
                            label = { Text(stringResource(R.string.vault_path)) },
                            singleLine = true,
                            enabled = !running,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        Button(onClick = open, enabled = !running && File(path.trim()).isAbsolute) {
                            Text(stringResource(R.string.open_vault))
                        }
                        if (running) {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(12.dp),
                            ) {
                                CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                                Text(stringResource(R.string.status_running))
                            }
                        }
                        if (failed) {
                            Text(stringResource(R.string.check_error), color = MaterialTheme.colorScheme.error)
                        }
                    }
                }
                if (result != null) {
                    item {
                        Column(
                            Modifier.padding(horizontal = 24.dp),
                            verticalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            Text(
                                File(result.vaultPath).name,
                                style = MaterialTheme.typography.headlineSmall,
                            )
                            FoundationStatus(result.report, running, testSave)
                            Text(stringResource(R.string.section_notes_list), style = MaterialTheme.typography.titleMedium)
                            if (result.notes.isEmpty()) Text(stringResource(R.string.empty_notes))
                        }
                    }
                    items(result.notes, key = { it.path }) { note ->
                        ListItem(
                            headlineContent = { Text(note.name) },
                            supportingContent = if (note.parent.isEmpty()) null else {
                                { Text(note.parent) }
                            },
                            modifier = Modifier.padding(horizontal = 8.dp).clickable { openNote(note.path) },
                        )
                    }
                }
            }
        }
    }

    if (preview != null) {
        AlertDialog(
            onDismissRequest = closePreview,
            title = { Text(File(preview.path).name) },
            text = {
                Text(
                    preview.excerpt ?: stringResource(R.string.preview_error),
                    modifier = Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState()),
                )
            },
            confirmButton = {
                TextButton(onClick = closePreview) { Text(stringResource(R.string.note_preview_close)) }
            },
        )
    }
}

@Composable
private fun FoundationStatus(report: JSONObject, running: Boolean, testSave: () -> Unit) {
    val scan = report.getJSONObject("vaultScan")
    val diagrams = report.getJSONObject("diagrams")
    val fonts = report.getJSONObject("fonts")
    val save = report.getJSONObject("atomicSave")
    Card(
        Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
    ) {
        Column(
            Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(stringResource(R.string.section_foundation_status), style = MaterialTheme.typography.titleMedium)
            StatusRow(stringResource(R.string.check_vault_scan), scan) {
                stringResource(R.string.note_count, scan.optInt("noteCount"))
            }
            StatusRow(stringResource(R.string.check_mermaid), diagrams) {
                stringResource(R.string.diagram_count, diagrams.getJSONArray("items").length())
            }
            StatusRow(stringResource(R.string.check_math), report.getJSONObject("math"))
            StatusRow(stringResource(R.string.check_fonts), fonts) {
                stringResource(R.string.font_difference, fonts.optDouble("maxAbsDiffPx"))
            }
            StatusRow(stringResource(R.string.check_atomic_save), save)
            if (scan.optInt("noteCount") > 0 && !save.optBoolean("passed")) {
                OutlinedButton(onClick = testSave, enabled = !running) {
                    Text(stringResource(R.string.test_save_copy))
                }
            }
        }
    }
}

@Composable
private fun StatusRow(
    label: String,
    result: JSONObject,
    success: @Composable () -> String = { stringResource(R.string.status_pass) },
) {
    val passed = result.optBoolean("passed")
    val status = when {
        !result.has("passed") -> stringResource(R.string.status_pending)
        passed -> success()
        else -> stringResource(R.string.status_fail)
    }
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
        Text(
            status,
            style = MaterialTheme.typography.labelLarge,
            color = when {
                !result.has("passed") -> MaterialTheme.colorScheme.onSurfaceVariant
                passed -> MaterialTheme.colorScheme.primary
                else -> MaterialTheme.colorScheme.error
            },
        )
    }
}
