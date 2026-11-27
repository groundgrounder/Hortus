// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.state

import android.app.Application
import androidx.compose.ui.text.input.TextFieldValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.app.HortusServices
import io.github.groundgrounder.hortus.app.IntentRouter
import io.github.groundgrounder.hortus.core.model.BacklinkRow
import io.github.groundgrounder.hortus.core.model.NoteContent
import io.github.groundgrounder.hortus.core.model.RecentNote
import io.github.groundgrounder.hortus.core.model.TagNoteRow
import io.github.groundgrounder.hortus.core.model.TextLink
import io.github.groundgrounder.hortus.core.settings.AppSettings
import io.github.groundgrounder.hortus.core.settings.ReaderPreferenceState
import io.github.groundgrounder.hortus.core.settings.ThemeMode
import io.github.groundgrounder.hortus.reader.model.EditState
import io.github.groundgrounder.hortus.reader.model.ItemRenderBridge
import io.github.groundgrounder.hortus.reader.model.KnowledgeState
import io.github.groundgrounder.hortus.reader.model.KnowledgeTab
import io.github.groundgrounder.hortus.reader.model.NewNoteState
import io.github.groundgrounder.hortus.reader.model.ReaderUiState
import io.github.groundgrounder.hortus.reader.model.ScrollTarget
import io.github.groundgrounder.hortus.reader.model.SearchState
import io.github.groundgrounder.hortus.reader.model.SearchTab
import io.github.groundgrounder.hortus.reader.model.SideMode
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.concurrent.atomic.AtomicLong

/**
 * 阅读与轻编辑主 ViewModel：跨配置变更存活，协调 VaultRepository、DocumentRepository、
 * EditorSessionController、SearchKnowledgeController 与 NewNoteShortcutController，
 * 并在 onCleared() 确定性关闭全部原生句柄。
 */
internal class ReaderViewModel(application: Application) : AndroidViewModel(application) {
    private val appContext = application.applicationContext
    private val services = HortusServices.get(appContext)
    private val settingsRepository = services.settingsRepository
    private val shortcuts = services.shortcutStore
    private val diagramService = services.diagramService
    private val mathService = services.mathService

    private lateinit var embedRepository: io.github.groundgrounder.hortus.data.EmbedRepository
    private val vaultRepository = services.createVaultRepository {
        if (::embedRepository.isInitialized) embedRepository.invalidate()
    }
    private val documentRepository = services.createDocumentRepository(vaultRepository) {
        if (::embedRepository.isInitialized) embedRepository.invalidate()
    }

    init {
        embedRepository = services.createEmbedRepository(vaultRepository)
        viewModelScope.launch {
            settingsRepository.preload()
            shortcuts.load()
        }
    }

    private val loadMutex = Mutex()
    private val openRequestSeq = AtomicLong(0)
    private val navigation = ReaderNavigation()
    private var persistSessionPreference = true
    private var previewReturn: NoteContent? = null

    private val _readerState = MutableStateFlow(ReaderUiState())
    val readerState: StateFlow<ReaderUiState> = _readerState.asStateFlow()

    private val _sidePaneOpen = MutableStateFlow(true)
    val sidePaneOpen: StateFlow<Boolean> = _sidePaneOpen.asStateFlow()

    private val _sideMode = MutableStateFlow(SideMode.FILES)
    val sideMode: StateFlow<SideMode> = _sideMode.asStateFlow()

    private val _settingsOpen = MutableStateFlow(false)
    val settingsOpen: StateFlow<Boolean> = _settingsOpen.asStateFlow()

    private lateinit var editorController: EditorSessionController

    private val syncController = ReaderSyncController(
        context = appContext,
        scope = viewModelScope,
        settingsRepository = settingsRepository,
        diagramService = diagramService,
        vaultRepository = vaultRepository,
        documentRepository = documentRepository,
        embedVersion = embedRepository.mutationVersion,
        loadMutex = loadMutex,
        readerState = _readerState,
        onClearCaches = { services.clearCaches() },
        isEditorActive = { ::editorController.isInitialized && editorController.isActive },
        onHandleExternalRefreshWhileEditing = { editorController.handleExternalRefreshWhileEditing() },
    )

    val appSettings: StateFlow<AppSettings> = syncController.appSettings
    val bodySizePx: StateFlow<Float> = syncController.bodySizePx
    val cacheClearedCount: StateFlow<Long> = syncController.cacheClearedCount

    init {
        editorController = EditorSessionController(
            context = appContext,
            scope = viewModelScope,
            documentRepository = documentRepository,
            loadMutex = loadMutex,
            onNotice = ::notice,
            onRefreshLocked = { syncController.refreshInternal() },
            onOpenConflictCopyLocked = { copyPath ->
                openInternal(copyPath, null, push = true, restoreAnchor = false)
            },
        )
    }

    val editState: StateFlow<EditState> = editorController.editState

    private val searchKnowledgeController = SearchKnowledgeController(
        scope = viewModelScope,
        vaultRepository = vaultRepository,
        documentRepository = documentRepository,
        currentContent = { _readerState.value.content },
    )
    val searchState: StateFlow<SearchState> = searchKnowledgeController.searchState
    val knowledgeState: StateFlow<KnowledgeState> = searchKnowledgeController.knowledgeState

    private val newNoteShortcuts = NewNoteShortcutController(
        context = appContext,
        scope = viewModelScope,
        vaultRepository = vaultRepository,
        shortcuts = shortcuts,
        currentContent = { _readerState.value.content },
        onNotice = ::notice,
        onCreatedAndOpen = { path ->
            val token = openRequestSeq.incrementAndGet()
            openNoteInternal(path, null, token)
        },
    )
    val newNoteState: StateFlow<NewNoteState> = newNoteShortcuts.newNoteState

    val renderBridge: ItemRenderBridge = ReaderRenderBridge(
        mathService = mathService,
        diagramService = diagramService,
        embedRepository = embedRepository,
        documentRepository = documentRepository,
    )

    val contentWidthPx: Int get() = syncController.contentWidthPx
    val lastVisibleBlockId: Long? get() = navigation.lastVisibleBlockId
    val lastVisibleOffsetPx: Int get() = navigation.lastVisibleOffsetPx

    val preferencesState: ReaderPreferenceState get() = settingsRepository.currentReaderState()
    val preferencesFlow: StateFlow<ReaderPreferenceState> = settingsRepository.readerState
    val isPreferencesLoaded: Boolean get() = settingsRepository.isPreloaded

    suspend fun awaitPreferences(): ReaderPreferenceState {
        settingsRepository.preload()
        return settingsRepository.readerState.value
    }

    val canEdit: Boolean
        get() = _readerState.value.content?.let {
            !it.externalPreview && it.readOnlyReason == null && !_readerState.value.loading
        } ?: false

    fun setThemeMode(mode: ThemeMode): Job = syncController.setThemeMode(mode)
    fun setFontSizeSp(fontSizeSp: Float): Job = syncController.setFontSizeSp(fontSizeSp)
    fun clearCaches(): Job = syncController.clearCaches()
    fun openSettings() { _settingsOpen.value = true }
    fun closeSettings() { _settingsOpen.value = false }

    fun toggleSidePane() {
        _sidePaneOpen.value = !_sidePaneOpen.value
    }

    fun selectSideMode(mode: SideMode) {
        _sideMode.value = mode
        when (mode) {
            SideMode.OUTLINE -> searchKnowledgeController.loadKnowledge(KnowledgeTab.OUTLINE, null)
            SideMode.BACKLINKS -> searchKnowledgeController.loadKnowledge(KnowledgeTab.BACKLINKS, null)
            SideMode.TAGS -> searchKnowledgeController.loadKnowledge(KnowledgeTab.TAGS, null)
            SideMode.FILES -> Unit
        }
    }

    fun openSearch() = searchKnowledgeController.openSearch(editorController.isActive)
    fun closeSearch() = searchKnowledgeController.closeSearch()
    fun setSearchTab(tab: SearchTab) = searchKnowledgeController.setSearchTab(tab)
    fun setSearchQuery(query: String) = searchKnowledgeController.setSearchQuery(query)

    fun openKnowledge(tab: KnowledgeTab, tag: String? = null, sheet: Boolean = true) =
        searchKnowledgeController.openKnowledge(tab, tag, sheet, editorController.isActive)

    fun closeKnowledge() = searchKnowledgeController.closeKnowledge()
    fun selectKnowledgeTab(tab: KnowledgeTab) = searchKnowledgeController.selectKnowledgeTab(tab)
    fun selectTag(tag: String) = searchKnowledgeController.selectTag(tag)

    fun openOutline(start: Int) {
        closeKnowledge()
        _sidePaneOpen.value = true
        val content = _readerState.value.content ?: return
        scrollToSourceOffset(content, start)
    }

    fun openBacklink(row: BacklinkRow) {
        closeKnowledge()
        openNoteAt(row.source, row.startUtf16)
    }

    fun openTagNote(row: TagNoteRow) {
        closeKnowledge()
        openNote(row.path)
    }

    fun openTagInText(tag: String) = openKnowledge(KnowledgeTab.TAGS, tag)

    fun openNoteAt(path: String, offset: Int) {
        val current = _readerState.value.content
        if (current != null && current.path == path && !current.externalPreview) {
            scrollToSourceOffset(current, offset)
            return
        }
        navigation.pendingScroll = path to offset
        openNote(path)
    }

    private fun scrollToSourceOffset(content: NoteContent, offset: Int) {
        val target = navigation.sourceOffsetTarget(content, offset) ?: return
        DebugProbes.recordOpenTarget(target)
        _readerState.value = _readerState.value.copy(scrollTarget = target)
        DebugProbes.recordAction("scroll_source:$offset")
    }

    fun startEditing() {
        val content = _readerState.value.content ?: return
        if (!canEdit) {
            editorController.startEditing(content, canEdit = false)
            return
        }
        closeSearch()
        closeKnowledge()
        newNoteShortcuts.dismissNewNoteDialog()
        editorController.startEditing(content, canEdit = true)
    }

    fun updateEditText(value: TextFieldValue) = editorController.updateEditText(value)
    fun finishEditing(): Job? = editorController.finishEditing()
    fun saveEditedNote(): Job? = editorController.saveEditedNote()
    fun keepMine(): Job? = editorController.keepMine()
    fun useDiskVersion(): Job? = editorController.useDiskVersion()
    fun saveConflictCopy(): Job? = editorController.saveConflictCopy()
    fun autoSaveEdit(): Job? = editorController.autoSaveEdit()

    fun openNewNoteDialog(parent: String? = null) = newNoteShortcuts.openNewNoteDialog(parent)
    fun dismissNewNoteDialog() = newNoteShortcuts.dismissNewNoteDialog()
    fun updateNewNoteName(name: String) = newNoteShortcuts.updateNewNoteName(name)
    fun selectNewNoteParent(parent: String) = newNoteShortcuts.selectNewNoteParent(parent)
    fun createNewNote(): Job? = newNoteShortcuts.createNewNote()
    fun pinCurrentNote(): Job? = newNoteShortcuts.pinCurrentNote()
    fun launchShortcutAsync(index: Int): Job = newNoteShortcuts.launchShortcutAsync(index)
    fun knownShortcutVaults(): Set<String> = newNoteShortcuts.knownShortcutVaults()
    fun shortcutNotesSync(): List<RecentNote> = newNoteShortcuts.shortcutNotesSync()

    fun openExternalPreview(fileName: String, text: String): Job {
        val requestToken = openRequestSeq.incrementAndGet()
        return viewModelScope.launch {
            settingsRepository.preload()
            if (openRequestSeq.get() != requestToken) return@launch
            if (!editorController.flushBeforeNavigation()) {
                clearPendingNavigation()
                return@launch
            }
            loadMutex.withLock {
                if (openRequestSeq.get() != requestToken) return@withLock
                val previous = _readerState.value.content
                if (previous != null && !previous.externalPreview) previewReturn = previous
                val width = syncController.resolvedWidthPx()
                val content = try {
                    documentRepository.openExternalPreview(
                        fileName = fileName,
                        text = text,
                        width = width,
                        bodySize = bodySizePx.value,
                        activeVaultPath = vaultRepository.vaultRoot,
                    )
                } catch (error: Exception) {
                    notice(error.message ?: "preview.failed")
                    return@withLock
                }
                DebugProbes.recordExternalMode("readonly_stream")
                val current = _readerState.value
                _readerState.value = current.copy(
                    loading = false,
                    error = null,
                    content = content,
                    scrollTarget = null,
                    openSequence = current.openSequence + 1,
                )
                DebugProbes.markContentInstalled()
                DebugProbes.recordAction("external_preview:$fileName")
            }
        }
    }

    fun setForeground(value: Boolean) {
        if (!value) autoSaveEdit()
    }

    fun start(
        vaultPath: String?,
        notePath: String?,
        persistPreference: Boolean = true,
    ): Job? {
        if (vaultPath == null) return null
        if (!IntentRouter.isValidVaultDirectory(vaultPath)) {
            notice(appContext.getString(R.string.vault_root_forbidden))
            return null
        }
        val requestToken = openRequestSeq.incrementAndGet()
        return viewModelScope.launch {
            if (openRequestSeq.get() != requestToken) return@launch
            if (!editorController.flushBeforeNavigation()) {
                clearPendingNavigation()
                return@launch
            }
            loadMutex.withLock {
                if (openRequestSeq.get() != requestToken) return@withLock
                startLocked(vaultPath, notePath, persistPreference = persistPreference)
            }
        }
    }

    fun openVaultAndPath(
        vaultPath: String,
        notePath: String,
        heading: String? = null,
        persistPreference: Boolean = true,
    ): Job {
        if (!IntentRouter.isValidVaultDirectory(vaultPath)) {
            notice(appContext.getString(R.string.vault_root_forbidden))
            return viewModelScope.launch { }
        }
        val requestToken = openRequestSeq.incrementAndGet()
        return viewModelScope.launch {
            if (openRequestSeq.get() != requestToken) return@launch
            if (!editorController.flushBeforeNavigation()) {
                clearPendingNavigation()
                return@launch
            }
            loadMutex.withLock {
                if (openRequestSeq.get() != requestToken) return@withLock
                if (_readerState.value.vaultPath != vaultPath) {
                    startLocked(vaultPath, notePath = null, persistPreference = persistPreference)
                }
                if (openRequestSeq.get() != requestToken) return@withLock
                val resolved = vaultRepository.resolveLink("", notePath) ?: notePath
                if (isReadablePath(resolved)) {
                    openInternal(resolved, heading, push = true, restoreAnchor = false)
                } else {
                    clearPendingNavigation()
                    notice(appContext.getString(R.string.link_not_found))
                }
            }
        }
    }

    private suspend fun startLocked(
        vaultPath: String,
        notePath: String?,
        persistPreference: Boolean,
    ) {
        if (!IntentRouter.isValidVaultDirectory(vaultPath)) {
            _readerState.value = _readerState.value.copy(
                loading = false,
                error = appContext.getString(R.string.vault_root_forbidden),
            )
            return
        }
        settingsRepository.preload()
        DebugProbes.incrementOpenCount()
        persistSessionPreference = persistPreference
        searchKnowledgeController.reset()
        newNoteShortcuts.reset()
        _sideMode.value = SideMode.FILES
        previewReturn = null
        navigation.resetForVault()
        documentRepository.cleanupExternalPreview()
        _readerState.value = _readerState.value.copy(loading = true, error = null)
        try {
            vaultRepository.openVault(vaultPath)
            vaultRepository.awaitScan()
            _readerState.value = _readerState.value.copy(
                vaultPath = vaultPath,
                tree = vaultRepository.tree.value,
            )
            if (persistPreference) {
                settingsRepository.saveReaderState(vaultPath, notePath ?: _readerState.value.content?.path)
            }
            syncController.observeRepository()
            if (notePath != null) {
                openInternal(notePath, null, push = true, restoreAnchor = true)
            } else {
                _readerState.value = _readerState.value.copy(loading = false)
            }
        } catch (error: Exception) {
            _readerState.value = _readerState.value.copy(
                loading = false,
                error = error.message ?: "vault.open_failed",
            )
        }
    }

    fun openNote(path: String, heading: String? = null): Job {
        val requestToken = openRequestSeq.incrementAndGet()
        return viewModelScope.launch {
            openNoteInternal(path, heading, requestToken)
        }
    }

    private suspend fun openNoteInternal(path: String, heading: String?, requestToken: Long) {
        if (openRequestSeq.get() != requestToken) return
        if (!editorController.flushBeforeNavigation()) {
            clearPendingNavigation()
            return
        }
        loadMutex.withLock {
            if (openRequestSeq.get() != requestToken) return@withLock
            val current = _readerState.value.content
            if (current?.path == path && !current.externalPreview) {
                if (heading != null) {
                    navigation.headingTarget(current, heading)?.let {
                        _readerState.value = _readerState.value.copy(scrollTarget = it)
                    }
                }
                return@withLock
            }
            openInternal(path, heading, push = true, restoreAnchor = false)
        }
    }

    fun openPath(path: String, heading: String?): Job? {
        val content = _readerState.value.content
        val requestToken = openRequestSeq.incrementAndGet()
        if (content == null) {
            return viewModelScope.launch {
                if (openRequestSeq.get() != requestToken) return@launch
                if (!editorController.flushBeforeNavigation()) {
                    clearPendingNavigation()
                    return@launch
                }
                loadMutex.withLock {
                    if (openRequestSeq.get() != requestToken) return@withLock
                    val resolved = vaultRepository.resolveLink("", path) ?: path
                    if (openRequestSeq.get() != requestToken) return@withLock
                    if (isReadablePath(resolved)) {
                        openInternal(resolved, heading, push = true, restoreAnchor = false)
                    } else {
                        clearPendingNavigation()
                        notice(appContext.getString(R.string.link_not_found))
                    }
                }
            }
        }
        if (path == content.path) {
            if (heading != null) {
                navigation.headingTarget(content, heading)?.let {
                    _readerState.value = _readerState.value.copy(scrollTarget = it)
                }
            }
            return null
        }
        val sourcePath = content.path
        return viewModelScope.launch {
            val resolved = vaultRepository.resolveLink(sourcePath, path) ?: path
            if (openRequestSeq.get() != requestToken || _readerState.value.content?.path != sourcePath) {
                return@launch
            }
            if (isReadablePath(resolved)) {
                openNoteInternal(resolved, heading, requestToken)
            } else {
                clearPendingNavigation()
                notice(appContext.getString(R.string.link_not_found))
            }
        }
    }

    fun followLink(link: TextLink) {
        if (link.resolution == "note" && link.resolvedPath != null) {
            DebugProbes.recordAction("link_open:${link.resolvedPath}")
            openNote(link.resolvedPath, link.heading)
        } else if (link.resolution == "attachment") {
            DebugProbes.recordAction("link_attachment:${link.target}")
            notice(appContext.getString(R.string.link_not_found))
        } else {
            DebugProbes.recordAction("link_missing:${link.target}")
            notice(appContext.getString(R.string.link_not_found))
        }
    }

    private suspend fun openInternal(path: String, heading: String?, push: Boolean, restoreAnchor: Boolean) {
        DebugProbes.markOpenStarted()
        val vaultRoot = vaultRepository.vaultRoot ?: run {
            _readerState.value = _readerState.value.copy(loading = false, error = "vault not opened")
            return
        }
        if (_readerState.value.content?.externalPreview == true) {
            documentRepository.cleanupExternalPreview()
        }
        _readerState.value = _readerState.value.copy(loading = true, error = null, notice = null)
        val width = syncController.resolvedWidthPx()
        try {
            val content = documentRepository.openNote(vaultRoot, path, width, bodySizePx.value)
            navigation.recordVisit(path, heading, push)
            previewReturn = null
            val target = navigation.resolveOpenTarget(content, heading, restoreAnchor)
            DebugProbes.recordOpenTarget(target)
            val prev = _readerState.value
            _readerState.value = prev.copy(
                loading = false,
                error = null,
                content = content,
                vaultPath = content.vaultPath,
                scrollTarget = target,
                openSequence = prev.openSequence + 1,
            )
            DebugProbes.markContentInstalled()
            if (persistSessionPreference) {
                settingsRepository.saveReaderState(content.vaultPath, content.path)
            }
            DebugProbes.clearAllHits()
            newNoteShortcuts.rememberShortcut(content)
            if (newNoteShortcuts.pendingEditPath == content.path) {
                newNoteShortcuts.pendingEditPath = null
                startEditing()
            }
        } catch (error: Exception) {
            clearPendingNavigation()
            _readerState.value = _readerState.value.copy(
                loading = false,
                error = error.message ?: "document.open_failed",
            )
        }
    }

    fun consumeScrollTarget() {
        if (_readerState.value.scrollTarget != null) {
            _readerState.value = _readerState.value.copy(scrollTarget = null)
        }
    }

    fun updateAnchor(blockId: Long, offsetPx: Int) =
        navigation.updateAnchor(_readerState.value.content?.path, blockId, offsetPx)

    fun restoreTarget(content: NoteContent): ScrollTarget? = navigation.restoreTarget(content)

    fun setContentWidth(width: Int) = syncController.setContentWidth(width)
    fun refresh() = syncController.refresh()

    fun back(): Boolean {
        if (_settingsOpen.value) {
            _settingsOpen.value = false
            return true
        }
        if (editorController.isActive) {
            finishEditing()
            return true
        }
        val current = _readerState.value.content
        if (current?.externalPreview == true) {
            if (previewReturn == null) return false
            val requestToken = openRequestSeq.incrementAndGet()
            viewModelScope.launch {
                loadMutex.withLock {
                    if (openRequestSeq.get() != requestToken) return@withLock
                    val previous = previewReturn ?: return@withLock
                    previewReturn = null
                    documentRepository.cleanupExternalPreview()
                    openInternal(previous.path, null, push = false, restoreAnchor = true)
                }
            }
            return true
        }
        if (!navigation.canPop()) return false
        val requestToken = openRequestSeq.incrementAndGet()
        viewModelScope.launch {
            loadMutex.withLock {
                if (openRequestSeq.get() != requestToken) return@withLock
                val previous = navigation.popPrevious(_readerState.value.content) ?: return@withLock
                openInternal(previous.path, null, push = false, restoreAnchor = true)
            }
        }
        return true
    }

    fun canGoBack(): Boolean =
        _settingsOpen.value ||
            editorController.isActive ||
            (_readerState.value.content?.externalPreview == true && previewReturn != null) ||
            navigation.canPop()

    fun notice(message: String) = syncController.notice(message)
    fun clearNotice() = syncController.clearNotice()

    fun markFirstFrame() {
        if (_readerState.value.content != null) DebugProbes.markFirstFrame()
    }

    fun markFullyDrawn() = DebugProbes.markFullyDrawn()
    fun markActivityCreated() = DebugProbes.markActivityCreated()
    fun recordAction(action: String) = DebugProbes.recordAction(action)
    fun openVaultFromPicker(path: String) { start(path, null) }
    fun onActivityStop() { autoSaveEdit() }

    private fun clearPendingNavigation() {
        navigation.clearPending()
        newNoteShortcuts.pendingEditPath = null
    }

    private fun isReadablePath(path: String): Boolean =
        path.endsWith(".md", ignoreCase = true) || path.endsWith(".mmd", ignoreCase = true)

    internal suspend fun awaitSearchIdle() = searchKnowledgeController.awaitSearchIdle()
    internal suspend fun awaitKnowledgeIdle() = searchKnowledgeController.awaitKnowledgeIdle()
    internal suspend fun awaitSaveIdle() = editorController.awaitSaveIdle()

    override fun onCleared() {
        syncController.cancelAll()
        searchKnowledgeController.cancelAll()
        documentRepository.close()
        vaultRepository.close()
        super.onCleared()
    }
}
