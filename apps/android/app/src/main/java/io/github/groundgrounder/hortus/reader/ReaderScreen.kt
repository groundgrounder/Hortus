// SPDX-License-Identifier: GPL-3.0-only
@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package io.github.groundgrounder.hortus.reader

import android.content.Context
import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.BottomAppBar
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.DebugProbes.debugItemBounds
import io.github.groundgrounder.hortus.DebugProbes.debugPaneWidth
import io.github.groundgrounder.hortus.DebugProbes.debugPanel
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.TreeEntry
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

@Composable
internal fun ReaderApp(
    controller: ReaderController,
    hasAccess: Boolean,
    onRequestPermission: () -> Unit,
) {
    val state = controller.state
    val palette = readerPalette()
    val dark = isSystemInDarkTheme()
    if (state.content == null) {
        HomeScreen(controller, hasAccess, onRequestPermission, state)
        return
    }
    ReaderScaffold(controller, state, palette, dark)
}

@Composable
private fun ReaderScaffold(
    controller: ReaderController,
    state: ReaderUiState,
    palette: ReaderPalette,
    dark: Boolean,
) {
    val context = LocalContext.current
    val snackbarHost = remember { SnackbarHostState() }
    val configuration = LocalConfiguration.current
    val tablet = configuration.screenWidthDp >= 600
    if (DebugProbes.enabled) {
        LaunchedEffect(tablet) {
            DebugProbes.recordLayoutMode(if (tablet) "tablet" else "phone")
        }
    }
    fun focusDiagram(
        item: DiagramItem,
        scene: io.github.groundgrounder.hortus.core.SceneData,
        collapsed: List<String>,
    ) {
        val token = FocusPayloads.put(
            FocusPayload(
                vaultPath = state.content?.vaultPath ?: "",
                notePath = item.documentKey,
                documentKey = item.documentKey,
                engineText = item.engineText,
                mmd = item.mmd,
                ordinal = item.ordinal,
                revision = state.content?.revision ?: 0,
                dark = dark,
                collapsed = collapsed,
                title = state.content?.title ?: "",
                renderWidth = scene.width,
            ),
        )
        DiagramFocusActivity.start(context, token)
    }

    val callbacks = ReaderCallbacks(
        onLink = controller::followLink,
        onTag = controller::openTagInText,
        onOpenPath = controller::openPath,
        onFocus = ::focusDiagram,
        onNotice = controller::notice,
    )
    if (controller.canGoBack()) {
        BackHandler { controller.back() }
    }
    LaunchedEffect(state.notice) {
        state.notice?.let {
            snackbarHost.showSnackbar(it)
            controller.clearNotice()
        }
    }
    Box(Modifier.fillMaxSize()) {
        when {
            controller.searchState.open -> SearchOverlay(controller, controller.searchState, palette)
            controller.editState.active -> EditorScreen(controller, state, controller.editState)
            tablet -> TabletScaffold(controller, state, snackbarHost, callbacks, palette, dark)
            else -> PhoneScaffold(controller, state, snackbarHost, callbacks, palette, dark)
        }
    }
    if (controller.knowledgeState.open) {
        KnowledgeSheet(controller, state, controller.knowledgeState)
    }
    if (controller.newNoteState.open) {
        NewNoteDialog(controller, state, controller.newNoteState)
    }
    ConflictDialog(controller, controller.editState)
}

@Composable
private fun PhoneScaffold(
    controller: ReaderController,
    state: ReaderUiState,
    snackbarHost: SnackbarHostState,
    callbacks: ReaderCallbacks,
    palette: ReaderPalette,
    dark: Boolean,
) {
    val scope = rememberCoroutineScope()
    val drawerState = androidx.compose.material3.rememberDrawerState(DrawerValue.Closed)
    ModalNavigationDrawer(
        drawerState = drawerState,
        gesturesEnabled = true,
        drawerContent = {
            ModalDrawerSheet {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        stringResource(R.string.reader_notes),
                        style = MaterialTheme.typography.titleMedium,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(onClick = { controller.openNewNoteDialog() }) {
                        Text(stringResource(R.string.new_note))
                    }
                }
                HorizontalDivider()
                FileTreeList(
                    entries = state.tree,
                    currentPath = state.content?.path,
                    onOpen = { path ->
                        controller.openNote(path)
                        scope.launch { drawerState.close() }
                    },
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(vertical = 8.dp),
                )
            }
        },
    ) {
        Scaffold(
            topBar = {
                ReaderTopBar(
                    controller = controller,
                    state = state,
                    tablet = false,
                    onOpenDrawer = { scope.launch { drawerState.open() } },
                )
            },
            bottomBar = { PhoneKnowledgeBar(controller) },
            snackbarHost = { SnackbarHost(snackbarHost) },
        ) { insets ->
            ReaderDetail(
                controller = controller,
                state = state,
                callbacks = callbacks,
                palette = palette,
                dark = dark,
                modifier = Modifier.fillMaxSize().padding(insets),
            )
        }
    }
}

@Composable
private fun TabletScaffold(
    controller: ReaderController,
    state: ReaderUiState,
    snackbarHost: SnackbarHostState,
    callbacks: ReaderCallbacks,
    palette: ReaderPalette,
    dark: Boolean,
) {
    Scaffold(
        topBar = {
            ReaderTopBar(
                controller = controller,
                state = state,
                tablet = true,
                onOpenDrawer = {},
            )
        },
        snackbarHost = { SnackbarHost(snackbarHost) },
    ) { insets ->
        Row(Modifier.fillMaxSize().padding(insets)) {
            if (DebugProbes.enabled) {
                LaunchedEffect(controller.sidePaneOpen) {
                    if (!controller.sidePaneOpen) DebugProbes.clearSidePaneWidth()
                }
            }
            if (controller.sidePaneOpen) {
                SidePane(
                    controller = controller,
                    state = state,
                    knowledge = controller.knowledgeState,
                    modifier = Modifier
                        .width(320.dp)
                        .fillMaxHeight()
                        .debugPaneWidth(isSide = true),
                )
                VerticalDivider()
            }
            ReaderDetail(
                controller = controller,
                state = state,
                callbacks = callbacks,
                palette = palette,
                dark = dark,
                modifier = Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .debugPaneWidth(isSide = false),
            )
        }
    }
}

@Composable
private fun ReaderTopBar(
    controller: ReaderController,
    state: ReaderUiState,
    tablet: Boolean,
    onOpenDrawer: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    TopAppBar(
        title = {
            Text(
                state.content?.title ?: stringResource(R.string.app_name),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        },
        navigationIcon = {
            when {
                controller.canGoBack() -> {
                    IconButton(onClick = controller::back) {
                        Icon(
                            Icons.Default.ArrowBack,
                            contentDescription = stringResource(R.string.reader_back),
                        )
                    }
                }
                tablet -> {
                    IconButton(onClick = controller::toggleSidePane) {
                        Icon(
                            Icons.Default.Menu,
                            contentDescription = stringResource(R.string.reader_menu),
                        )
                    }
                }
                else -> {
                    IconButton(onClick = onOpenDrawer) {
                        Icon(
                            Icons.Default.Menu,
                            contentDescription = stringResource(R.string.reader_menu),
                        )
                    }
                }
            }
        },
        actions = {
            IconButton(onClick = controller::openSearch) {
                Icon(
                    Icons.Default.Search,
                    contentDescription = stringResource(R.string.reader_search),
                )
            }
            if (state.content?.readOnlyReason != null) {
                Text(
                    stringResource(R.string.reader_read_only_label),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(end = 8.dp),
                )
            } else {
                IconButton(onClick = controller::startEditing, enabled = controller.canEdit) {
                    Icon(
                        Icons.Default.Edit,
                        contentDescription = stringResource(R.string.reader_edit),
                    )
                }
            }
            Box {
                IconButton(onClick = { menu = true }) {
                    Icon(
                        Icons.Default.MoreVert,
                        contentDescription = stringResource(R.string.reader_more),
                    )
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.shortcut_pin)) },
                        onClick = {
                            menu = false
                            controller.pinCurrentNote()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.new_note)) },
                        enabled = state.content?.externalPreview != true,
                        onClick = {
                            menu = false
                            controller.openNewNoteDialog()
                        },
                    )
                }
            }
        },
    )
}

@Composable
private fun PhoneKnowledgeBar(controller: ReaderController) {
    BottomAppBar {
        TextButton(
            onClick = { controller.openKnowledge(KnowledgeTab.OUTLINE) },
            modifier = Modifier.weight(1f),
        ) {
            Text(stringResource(R.string.knowledge_outline))
        }
        TextButton(
            onClick = { controller.openKnowledge(KnowledgeTab.BACKLINKS) },
            modifier = Modifier.weight(1f),
        ) {
            Text(stringResource(R.string.knowledge_backlinks))
        }
        TextButton(
            onClick = { controller.openKnowledge(KnowledgeTab.TAGS) },
            modifier = Modifier.weight(1f),
        ) {
            Text(stringResource(R.string.knowledge_tags))
        }
    }
}

@Composable
private fun ReaderDetail(
    controller: ReaderController,
    state: ReaderUiState,
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
                controller = controller,
                content = content,
                scrollTarget = state.scrollTarget,
                openSequence = state.openSequence,
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
    controller: ReaderController,
    content: NoteContent?,
    scrollTarget: ScrollTarget?,
    openSequence: Long,
    dark: Boolean,
    palette: ReaderPalette,
    callbacks: ReaderCallbacks,
    contentWidthPx: Int,
    bottomPadding: androidx.compose.ui.unit.Dp,
) {
    if (content == null) return
    LaunchedEffect(contentWidthPx) { controller.setContentWidth(contentWidthPx) }
    val initialTarget = remember(content.path) { scrollTarget ?: controller.restoreTarget(content) }
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
        repeat(8) {
            delay(80)
            if (index > 0) {
                snapshotFlow { listState.layoutInfo.totalItemsCount }
                    .first { it > index }
            }
            listState.scrollToItem(index, target.offsetPx)
            if (listState.firstVisibleItemIndex == index) return@repeat
        }
        controller.consumeScrollTarget()
    }
    LaunchedEffect(listState, content) {
        snapshotFlow {
            if (listState.layoutInfo.visibleItemsInfo.isEmpty()) null
            else listState.firstVisibleItemIndex to listState.firstVisibleItemScrollOffset
        }.collectLatest { value ->
            if (value != null) {
                delay(80)
                content.items.getOrNull(value.first)?.let { item ->
                    controller.updateAnchor(item.blockId, value.second)
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
    val activity = LocalContext.current as? android.app.Activity
    LaunchedEffect(openSequence, content.revision) {
        withFrameNanos { }
        controller.markFirstFrame()
        // reportFullyDrawn 在独立 UI 消息里调用，随后再排一帧，Macrobenchmark
        // 才能把这次上报关联到之后的 RenderThread 帧切片。
        withFrameNanos { }
        val decor = activity?.window?.decorView ?: return@LaunchedEffect
        decor.post {
            controller.markFullyDrawn()
            activity.reportFullyDrawn()
            decor.invalidate()
        }
    }
    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            state = listState,
            modifier = Modifier.fillMaxSize().padding(horizontal = 24.dp),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = bottomPadding),
        ) {
            itemsIndexed(
                content.items,
                key = { _, item -> item.key },
            ) { _, item ->
                ReaderItemView(
                    item = item,
                    engineText = content.sourceText,
                    contentWidth = contentWidthPx,
                    bodySizePx = controller.bodySizePx,
                    dark = dark,
                    palette = palette,
                    repository = controller.repository(),
                    vaultPath = content.vaultPath,
                    notePath = content.path,
                    revision = content.revision,
                    ancestors = listOf(content.path),
                    callbacks = callbacks,
                    modifier = Modifier
                        .padding(vertical = 1.dp)
                        .debugItemBounds(item.key, item.blockId),
                )
            }
        }
        if (controller.state.loading) {
            CircularProgressIndicator(Modifier.align(Alignment.TopCenter).padding(8.dp).size(20.dp))
        }
    }
}

@Composable
internal fun FileTreeList(
    entries: List<TreeEntry>,
    currentPath: String?,
    onOpen: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val filtered = remember(entries) {
        entries.filter { entry ->
            entry.directory || entry.path.lowercase().endsWith(".md") ||
                entry.path.lowercase().endsWith(".mmd")
        }
    }
    val grouped = remember(filtered) { filtered.groupBy { it.parent } }
    val expanded = remember { mutableStateMapOf<String, Boolean>() }
    val rows = ArrayList<Pair<TreeEntry, Int>>()
    fun append(parent: String, depth: Int) {
        val children = grouped[parent]?.sortedWith(
            compareBy<TreeEntry> { !it.directory }.thenBy { it.name.lowercase() },
        ) ?: return
        for (child in children) {
            rows += child to depth
            if (child.directory && expanded[child.path] == true) {
                append(child.path, depth + 1)
            }
        }
    }
    append("", 0)
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

@Composable
private fun HomeScreen(
    controller: ReaderController,
    hasAccess: Boolean,
    onRequestPermission: () -> Unit,
    state: ReaderUiState,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var path by remember { mutableStateOf(controller.preferencesState.vaultPath ?: "/storage/emulated/0") }
    var pickerOpen by remember { mutableStateOf(false) }
    Scaffold(
        topBar = { TopAppBar(title = { Text(stringResource(R.string.app_name)) }) },
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
                    OutlinedTextField(
                        value = path,
                        onValueChange = { path = it },
                        label = { Text(stringResource(R.string.reader_folder_path)) },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                item {
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Button(
                            onClick = {
                                controller.start(path.trim(), null)
                            },
                            enabled = File(path.trim()).isAbsolute && File(path.trim()).isDirectory,
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
                                onOpen = { note -> controller.openNote(note) },
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
                controller.start(chosen, null)
            },
        )
    }
}

@Composable
private fun VaultPickerDialog(onDismiss: () -> Unit, onPick: (String) -> Unit) {
    var directory by remember { mutableStateOf(File("/storage/emulated/0")) }
    val children = remember(directory) {
        try {
            directory.listFiles()
                ?.filter { it.isDirectory && !it.name.startsWith(".") }
                ?.sortedBy { it.name.lowercase() }
                ?: emptyList()
        } catch (_: Exception) {
            emptyList()
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
                            Icon(
                                Icons.Default.KeyboardArrowRight,
                                contentDescription = null,
                                modifier = Modifier.size(18.dp),
                                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Text(child.name, modifier = Modifier.padding(start = 6.dp))
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = { onPick(directory.path) }) {
                Text(stringResource(R.string.reader_use_folder))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.reader_cancel)) }
        },
    )
}
