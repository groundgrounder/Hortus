// SPDX-License-Identifier: GPL-3.0-only
@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package io.github.groundgrounder.hortus.reader.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.BottomAppBar
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.VerticalDivider
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.movableContentOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.DebugProbes.debugPaneWidth
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.settings.ThemeMode
import io.github.groundgrounder.hortus.focus.DiagramFocusActivity
import io.github.groundgrounder.hortus.focus.FocusPayload
import io.github.groundgrounder.hortus.focus.FocusPayloads
import io.github.groundgrounder.hortus.reader.model.KnowledgeState
import io.github.groundgrounder.hortus.reader.model.KnowledgeTab
import io.github.groundgrounder.hortus.reader.model.ReaderCallbacks
import io.github.groundgrounder.hortus.reader.model.ReaderPalette
import io.github.groundgrounder.hortus.reader.model.ReaderUiState
import io.github.groundgrounder.hortus.reader.model.SideMode
import io.github.groundgrounder.hortus.reader.model.readerPalette
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel
import kotlinx.coroutines.launch

@Composable
internal fun ReaderApp(
    viewModel: ReaderViewModel,
    hasAccess: Boolean,
    onRequestPermission: () -> Unit,
) {
    val state by viewModel.readerState.collectAsStateWithLifecycle()
    val settings by viewModel.appSettings.collectAsStateWithLifecycle()
    val palette = readerPalette()
    val dark = when (settings.themeMode) {
        ThemeMode.SYSTEM -> isSystemInDarkTheme()
        ThemeMode.LIGHT -> false
        ThemeMode.DARK -> true
    }
    if (state.content == null) {
        HomeScreen(viewModel, hasAccess, onRequestPermission, state)
        return
    }
    ReaderScaffold(viewModel, state, palette, dark)
}

@Composable
private fun ReaderScaffold(
    viewModel: ReaderViewModel,
    state: ReaderUiState,
    palette: ReaderPalette,
    dark: Boolean,
) {
    val context = LocalContext.current
    val snackbarHost = remember { SnackbarHostState() }
    val configuration = LocalConfiguration.current
    val tablet = configuration.screenWidthDp >= 600
    val searchState by viewModel.searchState.collectAsStateWithLifecycle()
    val editState by viewModel.editState.collectAsStateWithLifecycle()
    val knowledgeState by viewModel.knowledgeState.collectAsStateWithLifecycle()
    val newNoteState by viewModel.newNoteState.collectAsStateWithLifecycle()
    val sidePaneOpen by viewModel.sidePaneOpen.collectAsStateWithLifecycle()
    val sideMode by viewModel.sideMode.collectAsStateWithLifecycle()
    val bodySizePx by viewModel.bodySizePx.collectAsStateWithLifecycle()

    if (DebugProbes.enabled) {
        LaunchedEffect(tablet) {
            DebugProbes.recordLayoutMode(if (tablet) "tablet" else "phone")
        }
    }

    val callbacks = remember(viewModel, dark, context) {
        ReaderCallbacks(
            onLink = viewModel::followLink,
            onTag = viewModel::openTagInText,
            onOpenPath = viewModel::openPath,
            onFocus = { item, scene, collapsed ->
                val currentContent = viewModel.readerState.value.content
                val token = FocusPayloads.put(
                    FocusPayload(
                        vaultPath = currentContent?.vaultPath ?: "",
                        notePath = item.documentKey,
                        documentKey = item.documentKey,
                        engineText = item.engineText,
                        mmd = item.mmd,
                        ordinal = item.ordinal,
                        revision = currentContent?.revision ?: 0,
                        dark = dark,
                        collapsed = collapsed,
                        title = currentContent?.title ?: "",
                        renderWidth = scene.width,
                    ),
                )
                DiagramFocusActivity.start(context, token)
            },
            onNotice = viewModel::notice,
        )
    }

    if (viewModel.canGoBack()) {
        BackHandler { viewModel.back() }
    }
    LaunchedEffect(state.notice) {
        state.notice?.let {
            snackbarHost.showSnackbar(it)
            viewModel.clearNotice()
        }
    }

    // 跨 600dp 断点与旋转共享单一 ReaderDetail 组合槽位，保持 LazyListState 与滚动锚点零丢失。
    val sharedDetail = remember(viewModel) {
        movableContentOf<DetailSlotArgs> { args ->
            ReaderDetail(
                viewModel = viewModel,
                state = args.state,
                bodySizePx = args.bodySizePx,
                callbacks = args.callbacks,
                palette = args.palette,
                dark = args.dark,
                modifier = args.modifier,
            )
        }
    }

    Box(Modifier.fillMaxSize()) {
        when {
            searchState.open -> SearchOverlay(viewModel, searchState, palette)
            editState.active -> EditorScreen(viewModel, state, editState)
            tablet -> TabletScaffold(
                viewModel = viewModel,
                state = state,
                knowledgeState = knowledgeState,
                sidePaneOpen = sidePaneOpen,
                sideMode = sideMode,
                snackbarHost = snackbarHost,
                detailContent = { detailModifier ->
                    sharedDetail(DetailSlotArgs(detailModifier, state, bodySizePx, callbacks, palette, dark))
                },
            )
            else -> PhoneScaffold(
                viewModel = viewModel,
                state = state,
                snackbarHost = snackbarHost,
                detailContent = { detailModifier ->
                    sharedDetail(DetailSlotArgs(detailModifier, state, bodySizePx, callbacks, palette, dark))
                },
            )
        }
    }
    if (knowledgeState.open) {
        KnowledgeSheet(viewModel, knowledgeState)
    }
    if (newNoteState.open) {
        NewNoteDialog(viewModel, state, newNoteState)
    }
    ConflictDialog(viewModel, editState)
}

private data class DetailSlotArgs(
    val modifier: Modifier,
    val state: ReaderUiState,
    val bodySizePx: Float,
    val callbacks: ReaderCallbacks,
    val palette: ReaderPalette,
    val dark: Boolean,
)

@Composable
private fun PhoneScaffold(
    viewModel: ReaderViewModel,
    state: ReaderUiState,
    snackbarHost: SnackbarHostState,
    detailContent: @Composable (Modifier) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val drawerState = rememberDrawerState(DrawerValue.Closed)
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
                    TextButton(onClick = { viewModel.openNewNoteDialog() }) {
                        Text(stringResource(R.string.new_note))
                    }
                }
                HorizontalDivider()
                FileTreeList(
                    entries = state.tree,
                    currentPath = state.content?.path,
                    onOpen = { path ->
                        viewModel.openNote(path)
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
                    viewModel = viewModel,
                    state = state,
                    tablet = false,
                    onOpenDrawer = { scope.launch { drawerState.open() } },
                )
            },
            bottomBar = { PhoneKnowledgeBar(viewModel) },
            snackbarHost = { SnackbarHost(snackbarHost) },
        ) { insets ->
            detailContent(Modifier.fillMaxSize().padding(insets))
        }
    }
}

@Composable
private fun TabletScaffold(
    viewModel: ReaderViewModel,
    state: ReaderUiState,
    knowledgeState: KnowledgeState,
    sidePaneOpen: Boolean,
    sideMode: SideMode,
    snackbarHost: SnackbarHostState,
    detailContent: @Composable (Modifier) -> Unit,
) {
    Scaffold(
        topBar = {
            ReaderTopBar(
                viewModel = viewModel,
                state = state,
                tablet = true,
                onOpenDrawer = {},
            )
        },
        snackbarHost = { SnackbarHost(snackbarHost) },
    ) { insets ->
        Row(Modifier.fillMaxSize().padding(insets)) {
            if (DebugProbes.enabled) {
                LaunchedEffect(sidePaneOpen) {
                    if (!sidePaneOpen) DebugProbes.clearSidePaneWidth()
                }
            }
            if (sidePaneOpen) {
                SidePane(
                    viewModel = viewModel,
                    state = state,
                    knowledge = knowledgeState,
                    sideMode = sideMode,
                    modifier = Modifier
                        .width(320.dp)
                        .fillMaxHeight()
                        .debugPaneWidth(isSide = true),
                )
                VerticalDivider()
            }
            detailContent(
                Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .debugPaneWidth(isSide = false),
            )
        }
    }
}

@Composable
private fun ReaderTopBar(
    viewModel: ReaderViewModel,
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
                viewModel.canGoBack() -> {
                    IconButton(onClick = viewModel::back) {
                        Icon(
                            Icons.Default.ArrowBack,
                            contentDescription = stringResource(R.string.reader_back),
                        )
                    }
                }
                tablet -> {
                    IconButton(onClick = viewModel::toggleSidePane) {
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
            IconButton(onClick = viewModel::openSearch) {
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
                IconButton(onClick = viewModel::startEditing, enabled = viewModel.canEdit) {
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
                            viewModel.pinCurrentNote()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.new_note)) },
                        enabled = state.content?.externalPreview != true,
                        onClick = {
                            menu = false
                            viewModel.openNewNoteDialog()
                        },
                    )
                }
            }
        },
    )
}

@Composable
private fun PhoneKnowledgeBar(viewModel: ReaderViewModel) {
    BottomAppBar {
        TextButton(
            onClick = { viewModel.openKnowledge(KnowledgeTab.OUTLINE) },
            modifier = Modifier.weight(1f),
        ) {
            Text(stringResource(R.string.knowledge_outline))
        }
        TextButton(
            onClick = { viewModel.openKnowledge(KnowledgeTab.BACKLINKS) },
            modifier = Modifier.weight(1f),
        ) {
            Text(stringResource(R.string.knowledge_backlinks))
        }
        TextButton(
            onClick = { viewModel.openKnowledge(KnowledgeTab.TAGS) },
            modifier = Modifier.weight(1f),
        ) {
            Text(stringResource(R.string.knowledge_tags))
        }
    }
}
