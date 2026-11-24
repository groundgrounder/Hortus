// SPDX-License-Identifier: GPL-3.0-only
@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package io.github.groundgrounder.hortus.reader

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SearchBar
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.DebugProbes.debugPanel
import io.github.groundgrounder.hortus.R

@Composable
internal fun SearchOverlay(
    controller: ReaderController,
    search: SearchState,
    palette: ReaderPalette,
) {
    BackHandler { controller.closeSearch() }
    val keyboard = LocalSoftwareKeyboardController.current
    fun closeAnd(action: () -> Unit) {
        keyboard?.hide()
        controller.closeSearch()
        action()
    }
    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background),
    ) {
        SearchBar(
            query = search.query,
            onQueryChange = controller::setSearchQuery,
            onSearch = {
                controller.setSearchQuery(it)
                controller.setSearchTab(SearchTab.FULL)
            },
            active = false,
            onActiveChange = {},
            placeholder = { Text(stringResource(R.string.search_hint)) },
            leadingIcon = { Icon(Icons.Default.Search, contentDescription = null) },
            trailingIcon = {
                if (search.query.isNotEmpty()) {
                    IconButton(onClick = { controller.setSearchQuery("") }) {
                        Icon(
                            Icons.Default.Clear,
                            contentDescription = stringResource(R.string.search_clear),
                        )
                    }
                }
            },
            modifier = Modifier.fillMaxWidth(),
        ) {}
        TabRow(selectedTabIndex = search.tab.ordinal) {
            Tab(
                selected = search.tab == SearchTab.QUICK,
                onClick = { controller.setSearchTab(SearchTab.QUICK) },
                text = { Text(stringResource(R.string.search_tab_quick)) },
            )
            Tab(
                selected = search.tab == SearchTab.FULL,
                onClick = { controller.setSearchTab(SearchTab.FULL) },
                text = { Text(stringResource(R.string.search_tab_full)) },
            )
        }
        if (search.running) {
            LinearProgressIndicator(Modifier.fillMaxWidth())
        }
        Box(Modifier.fillMaxWidth().weight(1f)) {
            when (search.tab) {
                SearchTab.QUICK -> QuickResults(
                    rows = search.quick,
                    palette = palette,
                    onOpen = { row ->
                        closeAnd { controller.openNote(row.path) }
                    },
                )
                SearchTab.FULL -> FullResults(
                    rows = search.full,
                    palette = palette,
                    onOpen = { row, snippet ->
                        closeAnd { controller.openNoteAt(row.path, snippet.start) }
                    },
                )
            }
        }
    }
}

@Composable
private fun QuickResults(
    rows: List<QuickRow>,
    palette: ReaderPalette,
    onOpen: (QuickRow) -> Unit,
) {
    if (rows.isEmpty()) {
        EmptySearch()
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(rows, key = { it.path }) { row ->
            Column(
                Modifier
                    .fillMaxWidth()
                    .clickable { onOpen(row) }
                    .padding(horizontal = 20.dp, vertical = 11.dp)
                    .debugPanel(
                        scope = "search",
                        key = "quick:${row.path}",
                        kind = "quick",
                        label = row.title,
                        target = row.path,
                        tab = "quick",
                    ),
            ) {
                Highlighted(
                    text = row.title,
                    ranges = row.titleMatches,
                    palette = palette,
                    style = MaterialTheme.typography.bodyLarge,
                )
                Highlighted(
                    text = row.path,
                    ranges = row.pathMatches,
                    palette = palette,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            HorizontalDivider(Modifier.padding(horizontal = 20.dp))
        }
    }
}

internal data class SnippetDisplay(
    val text: String,
    val start: Int,
    val end: Int,
)

/**
 * 正文片段从匹配所在行开始显示，避免匹配行上方残留半截标题。
 * 行内换行折叠为空格，高亮区间随截掉的同一前缀平移，仍指向真实匹配词。
 */
internal fun snippetDisplay(snippet: String, localStart: Int, localEnd: Int): SnippetDisplay {
    val matchStart = localStart.coerceIn(0, snippet.length)
    val matchEnd = localEnd.coerceIn(matchStart, snippet.length)
    var from = 0
    var index = matchStart - 1
    while (index >= 0 && snippet[index] != '\n' && snippet[index] != '\r') index -= 1
    if (index >= 0) {
        from = index + 1
        while (from < matchStart && (snippet[from] == '\n' || snippet[from] == '\r')) from += 1
    }
    val visible = snippet.substring(from)
    val text = buildString(visible.length) {
        for (character in visible) {
            append(if (character == '\n' || character == '\r') ' ' else character)
        }
    }
    return SnippetDisplay(
        text = text,
        start = (matchStart - from).coerceIn(0, text.length),
        end = (matchEnd - from).coerceIn(0, text.length),
    )
}

@Composable
private fun FullResults(
    rows: List<SearchRow>,
    palette: ReaderPalette,
    onOpen: (SearchRow, SearchSnippet) -> Unit,
) {
    if (rows.isEmpty()) {
        EmptySearch()
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(rows, key = { it.path }) { row ->
            Column(Modifier.fillMaxWidth().padding(top = 12.dp)) {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 20.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        row.title,
                        style = MaterialTheme.typography.titleSmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    Text(
                        stringResource(R.string.search_match_count, row.count),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Text(
                    row.path,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(horizontal = 20.dp),
                )
                for (match in row.matches) {
                    val localStart = (match.start - match.snippetStart).coerceAtLeast(0)
                    val localEnd = (localStart + match.length).coerceAtMost(match.snippet.length)
                    val display = snippetDisplay(match.snippet, localStart, localEnd)
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .clickable { onOpen(row, match) }
                            .padding(horizontal = 24.dp, vertical = 8.dp)
                            .debugPanel(
                                scope = "search",
                                key = "full:${row.path}:${match.start}",
                                kind = "search-snippet",
                                label = row.title,
                                target = "${row.path}#${match.start}",
                                tab = "full",
                            ),
                    ) {
                        Highlighted(
                            text = display.text,
                            ranges = if (display.end > display.start) {
                                listOf(display.start until display.end)
                            } else {
                                emptyList()
                            },
                            palette = palette,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                }
                HorizontalDivider(Modifier.padding(top = 8.dp, start = 20.dp, end = 20.dp))
            }
        }
    }
}

@Composable
private fun EmptySearch() {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text(
            stringResource(R.string.search_empty),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun Highlighted(
    text: String,
    ranges: List<IntRange>,
    palette: ReaderPalette,
    style: androidx.compose.ui.text.TextStyle,
    color: Color = Color.Unspecified,
) {
    Text(
        annotatedHighlight(text, ranges, palette),
        style = style,
        color = color,
        maxLines = 3,
        overflow = TextOverflow.Ellipsis,
    )
}

internal fun annotatedHighlight(
    text: String,
    ranges: List<IntRange>,
    palette: ReaderPalette,
): AnnotatedString = buildAnnotatedString {
    append(text)
    val highlight = palette.highlight.copy(alpha = 0.55f)
    for (range in ranges) {
        val start = range.first.coerceIn(0, text.length)
        val end = (range.last + 1).coerceIn(start, text.length)
        if (end > start) {
            addStyle(SpanStyle(background = highlight, fontWeight = FontWeight.SemiBold), start, end)
        }
    }
}
