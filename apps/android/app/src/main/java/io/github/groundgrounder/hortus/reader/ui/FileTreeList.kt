// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowRight
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.DebugProbes.debugPanel
import io.github.groundgrounder.hortus.core.TreeEntry

@Composable
internal fun FileTreeList(
    entries: List<TreeEntry>,
    currentPath: String?,
    onOpen: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val grouped = remember(entries) {
        entries
            .filter { entry ->
                entry.directory || entry.path.lowercase().endsWith(".md") ||
                    entry.path.lowercase().endsWith(".mmd")
            }
            .groupBy { it.parent }
            .mapValues { (_, children) ->
                children.sortedWith(
                    compareBy<TreeEntry> { !it.directory }.thenBy { it.name.lowercase() },
                )
            }
    }
    val expanded = remember { mutableStateMapOf<String, Boolean>() }
    val expandedSnapshot = expanded.toMap()
    val rows = remember(grouped, expandedSnapshot) {
        buildList {
            fun append(parent: String, depth: Int) {
                val children = grouped[parent] ?: return
                for (child in children) {
                    add(child to depth)
                    if (child.directory && expandedSnapshot[child.path] == true) {
                        append(child.path, depth + 1)
                    }
                }
            }
            append("", 0)
        }
    }
    LazyColumn(modifier) {
        items(rows, key = { it.first.path }) { (entry, depth) ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable {
                        if (entry.directory) {
                            expanded[entry.path] = expanded[entry.path] != true
                        } else {
                            onOpen(entry.path)
                        }
                    }
                    .padding(start = (12 + depth * 16).dp, end = 12.dp, top = 9.dp, bottom = 9.dp)
                    .debugPanel(
                        scope = "files",
                        key = "file:${entry.path}",
                        kind = "file",
                        label = entry.name,
                        target = entry.path,
                        tab = "files",
                    ),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (entry.directory) {
                    Icon(
                        if (expanded[entry.path] == true) Icons.Default.KeyboardArrowDown else Icons.Default.KeyboardArrowRight,
                        contentDescription = null,
                        modifier = Modifier.size(18.dp),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                    Spacer(Modifier.width(18.dp))
                }
                Text(
                    entry.name,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    color = if (entry.path == currentPath) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.onSurface
                    },
                    modifier = Modifier.padding(start = 6.dp),
                )
            }
        }
    }
}
