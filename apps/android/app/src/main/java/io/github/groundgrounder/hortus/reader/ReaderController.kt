// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import android.os.SystemClock
import android.util.TypedValue
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.text.input.TextFieldValue
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.DocumentIssue
import io.github.groundgrounder.hortus.core.DocumentUpdate
import io.github.groundgrounder.hortus.core.SaveChoice
import io.github.groundgrounder.hortus.core.SearchCancellation
import io.github.groundgrounder.hortus.core.TreeEntry
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job as CoroutineJob

internal data class ScrollTarget(val itemIndex: Int, val offsetPx: Int)
internal data class ReaderAnchor(val blockId: Long, val offsetPx: Int)
private data class Destination(val path: String, val heading: String?)

internal data class ReaderUiState(
    val loading: Boolean = false,
    val error: String? = null,
    val content: NoteContent? = null,
    val vaultPath: String? = null,
    val tree: List<TreeEntry> = emptyList(),
    val scrollTarget: ScrollTarget? = null,
    val notice: String? = null,
    val openSequence: Long = 0,
)

internal object ReaderTiming {
    @Volatile var activityCreatedAt: Long = 0
    @Volatile var openStartedAt: Long = 0
    @Volatile var contentInstalledAt: Long = 0
    @Volatile var firstFrameAt: Long = 0
    @Volatile var fullyDrawnAt: Long = 0
    @Volatile var openCount: Int = 0
}

internal object HitRegistry {
    private val entries = LinkedHashMap<String, List<ScreenHit>>()

    fun replace(scope: String, hits: List<ScreenHit>) {
        synchronized(entries) {
            if (hits.isEmpty()) entries.remove(scope) else entries[scope] = hits
            while (entries.size > 256) entries.remove(entries.keys.first())
        }
    }

    fun snapshot(): List<ScreenHit> = synchronized(entries) { entries.values.flatten() }

    fun clear() = synchronized(entries) { entries.clear() }
}

internal data class ScreenHit(
    val kind: String,
    val label: String,
    val x: Float,
    val y: Float,
    val width: Float,
    val height: Float,
    val target: String?,
    val heading: String?,
    val canCollapse: Boolean,
    val nodeId: String?,
)

internal class ReaderController(context: Context) : AutoCloseable {
    private val appContext = context.applicationContext
    private val repository = ReaderRepository(appContext)
    private val preferences = ReaderPreferences(appContext)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val loadMutex = Mutex()
    private val stack = ArrayList<Destination>()
    private val anchors = HashMap<String, ReaderAnchor>()
    private var treeJob: Job? = null
    private var externalJob: Job? = null
    private var widthJob: Job? = null
    private var noticeJob: Job? = null
    private var refreshJob: CoroutineJob? = null
    private var foreground = false
    var state by mutableStateOf(ReaderUiState())
        private set
    var searchState by mutableStateOf(SearchState())
        private set
    var knowledgeState by mutableStateOf(KnowledgeState())
        private set
    var editState by mutableStateOf(EditState())
        private set
    var newNoteState by mutableStateOf(NewNoteState())
        private set
    var layoutMode by mutableStateOf("phone")
        private set
    var sidePaneOpen by mutableStateOf(true)
    var sideMode by mutableStateOf(SideMode.FILES)
    var externalMode: String? = null
        private set
    var sidePaneWidthPx: Int = 0
        private set
    var detailPaneWidthPx: Int = 0
        private set
    var querySummary: String = ""
        private set
    var lastSearchMicros: Long = 0
        private set
    var lastSaveIssue: String? = null
        private set
    var lastCreatedNote: String? = null
        private set
    @Volatile var shortcutsPublished: List<String> = emptyList()
        private set
    @Volatile var lastShortcutLaunchNote: String? = null
        private set
    @Volatile var lastShortcutLaunchVault: String? = null
        private set
    private val shortcuts = ShortcutStore(appContext)
    private var searchJob: Job? = null
    private var searchCancellation: SearchCancellation? = null
    private var knowledgeJob: Job? = null
    private var tagJob: Job? = null
    private var previewReturn: NoteContent? = null
    private var pendingScroll: Pair<String, Int>? = null
    private var pendingEditPath: String? = null

    val bodySizePx: Float = TypedValue.applyDimension(
        TypedValue.COMPLEX_UNIT_SP,
        READER_BODY_SP,
        appContext.resources.displayMetrics,
    )
    var contentWidthPx: Int = 0
        private set
    var lastVisibleBlockId: Long? = null
        private set
    var lastVisibleOffsetPx: Int = 0
        private set
    var lastVisiblePath: String? = null
        private set
    var lastAction: String = ""
        private set
    var lastOpenTarget: ScrollTarget? = null
        private set

    val preferencesState: ReaderPreferenceState get() = preferences.load()

    val canEdit: Boolean
        get() = state.content?.let {
            !it.externalPreview && it.readOnlyReason == null && !state.loading
        } ?: false

    fun updateLayoutMode(value: String) {
        if (layoutMode != value) layoutMode = value
    }

    fun recordPaneWidths(side: Int, detail: Int) {
        sidePaneWidthPx = side
        detailPaneWidthPx = detail
    }

    fun toggleSidePane() {
        sidePaneOpen = !sidePaneOpen
    }

    fun selectSideMode(mode: SideMode) {
        sideMode = mode
        when (mode) {
            SideMode.OUTLINE -> loadKnowledge(KnowledgeTab.OUTLINE, null)
            SideMode.BACKLINKS -> loadKnowledge(KnowledgeTab.BACKLINKS, null)
            SideMode.TAGS -> loadKnowledge(KnowledgeTab.TAGS, null)
            SideMode.FILES -> Unit
        }
    }

    fun openSearch() {
        if (editState.active) return
        searchState = searchState.copy(open = true)
        runSearch(immediate = true)
    }

    fun closeSearch() {
        searchJob?.cancel()
        searchCancellation?.cancel()
        searchCancellation = null
        searchState = searchState.copy(open = false, running = false)
    }

    fun setSearchTab(tab: SearchTab) {
        if (searchState.tab == tab) return
        searchState = searchState.copy(tab = tab)
        runSearch(immediate = true)
    }

    fun setSearchQuery(query: String) {
        searchState = searchState.copy(query = query)
        runSearch(immediate = false)
    }

    private fun runSearch(immediate: Boolean) {
        searchJob?.cancel()
        searchCancellation?.cancel()
        val hadRunning = searchState.running
        val generation = searchState.generation + 1
        val query = searchState.query
        val tab = searchState.tab
        val cancellation = SearchCancellation()
        searchCancellation = cancellation
        searchState = searchState.copy(
            generation = generation,
            running = tab == SearchTab.FULL && query.isNotBlank(),
            cancelled = searchState.cancelled + if (hadRunning) 1 else 0,
        )
        if (tab == SearchTab.FULL && query.isBlank()) {
            searchState = searchState.copy(full = emptyList(), running = false)
            return
        }
        searchJob = scope.launch {
            try {
                if (!immediate) delay(140)
                val started = SystemClock.uptimeMillis()
                val pair = withContext(Dispatchers.IO) {
                    if (tab == SearchTab.QUICK) {
                        MainThreadCounters.record("quickOpen") {
                            repository.quickOpen(query, 50)
                        } to emptyList()
                    } else {
                        emptyList<QuickRow>() to MainThreadCounters.record("search") {
                            repository.search(query, cancellation)
                        }
                    }
                }
                lastSearchMicros = SystemClock.uptimeMillis() - started
                if (generation != searchState.generation || !searchState.open ||
                    searchState.tab != tab
                ) {
                    searchState = searchState.copy(
                        staleDiscarded = searchState.staleDiscarded + 1,
                        running = false,
                    )
                    return@launch
                }
                searchState = searchState.copy(
                    quick = pair.first,
                    full = pair.second,
                    running = false,
                )
                querySummary = when {
                    tab == SearchTab.QUICK -> pair.first.firstOrNull()?.let {
                        "quick:${it.path}:${pair.first.size}"
                    } ?: "quick:none"
                    else -> pair.second.firstOrNull()?.let {
                        "full:${it.path}:${it.count}:${pair.second.size}"
                    } ?: "full:none"
                }
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                if (generation == searchState.generation) {
                    searchState = searchState.copy(running = false)
                }
            }
        }
    }

    fun openKnowledge(tab: KnowledgeTab, tag: String? = null, sheet: Boolean = true) {
        if (editState.active) return
        knowledgeState = knowledgeState.copy(open = sheet, tab = tab, selectedTag = tag)
        loadKnowledge(tab, tag)
    }

    fun closeKnowledge() {
        knowledgeJob?.cancel()
        tagJob?.cancel()
        knowledgeState = knowledgeState.copy(open = false)
    }

    fun selectKnowledgeTab(tab: KnowledgeTab) {
        knowledgeState = knowledgeState.copy(tab = tab)
        loadKnowledge(tab, knowledgeState.selectedTag)
    }

    fun selectTag(tag: String) {
        if (tag.isEmpty()) {
            tagJob?.cancel()
            knowledgeState = knowledgeState.copy(selectedTag = null, tagNotes = emptyList())
            return
        }
        knowledgeState = knowledgeState.copy(selectedTag = tag)
        loadTagNotes(tag)
    }

    private fun loadKnowledge(tab: KnowledgeTab, tag: String?) {
        val content = state.content ?: return
        knowledgeJob?.cancel()
        knowledgeState = knowledgeState.copy(loading = true, tab = tab, selectedTag = tag)
        knowledgeJob = scope.launch {
            try {
                val loaded = withContext(Dispatchers.IO) {
                    val outline = MainThreadCounters.record("outline") {
                        repository.outlineRows()
                    }
                    val backlinks = MainThreadCounters.record("backlinks") {
                        repository.backlinks(content.path)
                    }
                    val tags = MainThreadCounters.record("tags") { repository.tags() }
                    Triple(outline, backlinks, tags)
                }
                if (state.content?.path != content.path) return@launch
                knowledgeState = knowledgeState.copy(
                    outline = loaded.first,
                    backlinks = loaded.second,
                    tags = loaded.third,
                    loading = false,
                )
                if (tag != null) loadTagNotes(tag)
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                knowledgeState = knowledgeState.copy(loading = false)
            }
        }
    }

    private fun loadTagNotes(tag: String) {
        tagJob?.cancel()
        tagJob = scope.launch {
            try {
                val notes = withContext(Dispatchers.IO) {
                    MainThreadCounters.record("tags") { repository.notesWithTag(tag) }
                }
                if (knowledgeState.selectedTag == tag) {
                    knowledgeState = knowledgeState.copy(tagNotes = notes)
                }
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                Unit
            }
        }
    }

    fun openOutline(start: Int) {
        closeKnowledge()
        sidePaneOpen = true
        val content = state.content ?: return
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

    fun openTagInText(tag: String) {
        openKnowledge(KnowledgeTab.TAGS, tag)
    }

    fun openNoteAt(path: String, offset: Int) {
        val current = state.content
        if (current != null && current.path == path && !current.externalPreview) {
            scrollToSourceOffset(current, offset)
            return
        }
        pendingScroll = path to offset
        openNote(path)
    }

    private fun scrollToSourceOffset(content: NoteContent, offset: Int) {
        val target = sourceOffsetTarget(content, offset) ?: return
        lastOpenTarget = target
        state = state.copy(scrollTarget = target)
        lastAction = "scroll_source:$offset"
    }

    private fun sourceOffsetTarget(content: NoteContent, offset: Int): ScrollTarget? {
        content.items.firstOrNull {
            offset >= it.sourceStart && offset < it.sourceStart + it.sourceLength
        }?.let { item ->
            val index = content.items.indexOf(item)
            if (index >= 0) return ScrollTarget(index, 0)
        }
        val heading = content.outline.headings.lastOrNull { it.start.toInt() <= offset }
        if (heading != null) {
            val block = content.model.blocks.firstOrNull {
                heading.start.toInt() >= it.sourceStart &&
                    heading.start.toInt() < it.sourceStart + it.sourceLength
            }
            if (block != null) {
                val index = content.items.indexOfFirst { it.blockId == block.id }
                if (index >= 0) return ScrollTarget(index, 0)
            }
        }
        return null
    }

    fun startEditing() {
        val content = state.content ?: return
        if (!canEdit) {
            notice(
                appContext.getString(
                    R.string.editor_read_only_reason,
                    content.readOnlyReason ?: "external",
                ),
            )
            return
        }
        closeSearch()
        closeKnowledge()
        newNoteState = NewNoteState()
        editState = EditState(
            active = true,
            field = TextFieldValue(content.sourceText),
            savedText = content.sourceText,
            sessionId = SystemClock.uptimeMillis(),
            readOnlyReason = content.readOnlyReason,
        )
        lastAction = "edit_start:${content.path}"
    }

    fun updateEditText(value: TextFieldValue) {
        if (!editState.active) return
        editState = editState.copy(
            field = value,
            dirty = value.text != editState.savedText,
        )
    }

    fun finishEditing() {
        if (!editState.active) return
        scope.launch {
            if (!editState.dirty) {
                editState = editState.copy(active = false)
                return@launch
            }
            if (saveEditSuspending(SaveChoice.NORMAL)) {
                editState = editState.copy(active = false)
                notice(appContext.getString(R.string.editor_saved))
            }
        }
    }

    fun saveEditedNote() {
        if (!editState.active) return
        scope.launch {
            if (saveEditSuspending(SaveChoice.NORMAL)) {
                notice(appContext.getString(R.string.editor_saved))
            }
        }
    }

    fun keepMine() {
        if (!editState.active) return
        val choice = if (editState.issue == "missing") SaveChoice.RECREATE else SaveChoice.KEEP_MINE
        scope.launch {
            if (saveEditSuspending(choice)) {
                editState = editState.copy(active = false)
                notice(appContext.getString(R.string.editor_saved))
            }
        }
    }

    fun useDiskVersion() {
        if (!editState.active) return
        scope.launch {
            editState = editState.copy(saving = true)
            val update = try {
                loadMutex.withLock {
                    withContext(Dispatchers.IO) { repository.useDiskVersion() }
                }
            } catch (_: Exception) {
                null
            }
            val text = update?.text
            if (text != null) {
                editState = editState.copy(
                    active = false,
                    field = TextFieldValue(text),
                    savedText = text,
                    dirty = false,
                    saving = false,
                    issue = null,
                    lastSaveOk = false,
                )
                refreshInternal()
                notice(appContext.getString(R.string.editor_disk_loaded))
            } else {
                editState = editState.copy(saving = false, issue = "save_failed")
            }
        }
    }

    fun saveConflictCopy() {
        if (!editState.active) return
        scope.launch {
            if (saveEditSuspending(SaveChoice.CONFLICT_COPY)) {
                editState = editState.copy(active = false)
            }
        }
    }

    fun autoSaveEdit() {
        if (!editState.active || !editState.dirty || editState.saving) return
        scope.launch { saveEditSuspending(SaveChoice.NORMAL) }
    }

    private suspend fun saveEditSuspending(choice: SaveChoice): Boolean {
        if (!editState.active) return true
        val text = editState.field.text
        editState = editState.copy(saving = true, issue = null, choice = choice.name)
        val temporary = (appContext.externalCacheDir ?: appContext.cacheDir).path
        val update: DocumentUpdate? = try {
            loadMutex.withLock {
                withContext(Dispatchers.IO) {
                    MainThreadCounters.record("save") {
                        repository.replaceAndSave(text, temporary, choice)
                    }
                }
            }
        } catch (error: CancellationException) {
            throw error
        } catch (_: Exception) {
            null
        }
        if (update == null) {
            lastSaveIssue = "io.failure"
            editState = editState.copy(saving = false, issue = "save_failed", lastSaveOk = false)
            return false
        }
        lastSaveIssue = update.issue.name
        if (update.copyPath != null) {
            val copy = update.copyPath
            try {
                loadMutex.withLock {
                    withContext(Dispatchers.IO) { repository.useDiskVersion() }
                }
            } catch (_: Exception) {
                Unit
            }
            editState = editState.copy(
                saving = false,
                dirty = false,
                copyPath = copy,
                lastSaveOk = true,
                issue = null,
            )
            refreshInternal()
            notice(appContext.getString(R.string.editor_copy_saved, copy))
            return true
        }
        return when (update.issue) {
            DocumentIssue.CONFLICT -> {
                editState = editState.copy(saving = false, issue = "conflict", lastSaveOk = false)
                false
            }
            DocumentIssue.MISSING -> {
                editState = editState.copy(saving = false, issue = "missing", lastSaveOk = false)
                false
            }
            DocumentIssue.SAVE_FAILED, DocumentIssue.RECOVERY -> {
                editState = editState.copy(saving = false, issue = "save_failed", lastSaveOk = false)
                false
            }
            DocumentIssue.NONE -> {
                editState = editState.copy(
                    savedText = text,
                    dirty = false,
                    saving = false,
                    issue = null,
                    lastSaveOk = true,
                    lastSaveMicros = update.saveMicros.toLong(),
                )
                refreshInternal()
                true
            }
        }
    }

    fun openNewNoteDialog(parent: String? = null) {
        val content = state.content
        val defaultParent = parent
            ?: content?.path?.substringBeforeLast('/', "")
            ?: ""
        newNoteState = NewNoteState(open = true, parent = defaultParent)
    }

    fun dismissNewNoteDialog() {
        newNoteState = NewNoteState()
    }

    fun updateNewNoteName(name: String) {
        newNoteState = newNoteState.copy(name = name, error = null)
    }

    fun selectNewNoteParent(parent: String) {
        newNoteState = newNoteState.copy(parent = parent, error = null)
    }

    fun createNewNote() {
        if (newNoteState.creating) return
        val name = newNoteState.name.trim()
        if (name.isEmpty()) {
            newNoteState = newNoteState.copy(error = "vault.invalid_name")
            return
        }
        val parent = newNoteState.parent
        newNoteState = newNoteState.copy(creating = true, error = null)
        scope.launch {
            val entry = try {
                withContext(Dispatchers.IO) {
                    MainThreadCounters.record("createNote") {
                        repository.createNote(parent, name)
                    }
                }
            } catch (error: Exception) {
                val raw = error.message ?: "vault.create_failed"
                val code = raw.removePrefix("kind=").substringBefore(',')
                newNoteState = newNoteState.copy(creating = false, error = code)
                return@launch
            }
            if (entry == null) {
                newNoteState = newNoteState.copy(creating = false, error = "vault.create_failed")
                return@launch
            }
            lastCreatedNote = entry.path
            newNoteState = NewNoteState()
            pendingEditPath = entry.path
            openNote(entry.path)
        }
    }

    fun pinCurrentNote() {
        val content = state.content ?: return
        if (content.externalPreview) {
            notice(appContext.getString(R.string.shortcut_unsupported))
            return
        }
        val pinned = shortcuts.requestPin(
            RecentNote(content.vaultPath, content.path, content.title, System.currentTimeMillis()),
        )
        notice(
            appContext.getString(
                if (pinned) R.string.shortcut_pinned else R.string.shortcut_unsupported,
            ),
        )
    }

    fun launchShortcut(index: Int): Boolean {
        val intent = shortcuts.launchIntent(index) ?: return false
        lastShortcutLaunchNote = intent.getStringExtra(
            io.github.groundgrounder.hortus.MainActivity.EXTRA_OPEN_NOTE,
        )
        lastShortcutLaunchVault = intent.getStringExtra(
            io.github.groundgrounder.hortus.MainActivity.EXTRA_OPEN_VAULT,
        )
        appContext.startActivity(
            intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        return true
    }

    fun recordExternalMode(mode: String) {
        externalMode = mode
    }

    fun shortcutNotes(): List<RecentNote> = shortcuts.load()

    fun openExternalPreview(fileName: String, text: String) {
        scope.launch {
            loadMutex.withLock {
                val previous = state.content
                if (previous != null && !previous.externalPreview) previewReturn = previous
                val width = if (contentWidthPx > 0) contentWidthPx else defaultWidth()
                val content = try {
                    withContext(Dispatchers.IO) {
                        repository.openExternalPreview(fileName, text, width, bodySizePx)
                    }
                } catch (error: Exception) {
                    notice(error.message ?: "preview.failed")
                    return@withLock
                }
                editState = EditState()
                externalMode = "readonly_stream"
                state = state.copy(
                    loading = false,
                    error = null,
                    content = content,
                    scrollTarget = null,
                    openSequence = state.openSequence + 1,
                )
                ReaderTiming.contentInstalledAt = SystemClock.uptimeMillis()
                lastAction = "external_preview:$fileName"
            }
        }
    }

    fun setForeground(value: Boolean) {
        foreground = value
        refreshJob?.cancel()
        refreshJob = null
        if (value) {
            refreshJob = scope.launch {
                while (true) {
                    delay(900)
                    if (!foreground || state.content == null || state.loading) continue
                    loadMutex.withLock {
                        try {
                            refreshInternal()
                        } catch (_: Exception) {
                            Unit
                        }
                    }
                }
            }
        } else {
            autoSaveEdit()
        }
    }

    fun start(vaultPath: String?, notePath: String?) {
        if (vaultPath == null) return
        scope.launch {
            loadMutex.withLock {
                ReaderTiming.openCount += 1
                editState = EditState()
                searchState = SearchState()
                knowledgeState = KnowledgeState()
                sideMode = SideMode.FILES
                previewReturn = null
                state = state.copy(loading = true, error = null)
                try {
                    withContext(Dispatchers.IO) {
                        repository.openVault(vaultPath)
                        repository.awaitScan()
                    }
                    state = state.copy(vaultPath = vaultPath, tree = repository.tree.value)
                    preferences.save(vaultPath, notePath ?: state.content?.path)
                    observeRepository()
                    if (notePath != null) {
                        openInternal(notePath, null, push = true, restoreAnchor = true)
                    } else {
                        state = state.copy(loading = false)
                    }
                } catch (error: Exception) {
                    state = state.copy(loading = false, error = error.message ?: "vault.open_failed")
                }
            }
        }
    }

    private fun observeRepository() {
        treeJob?.cancel()
        treeJob = scope.launch {
            repository.tree.collectLatest { tree ->
                state = state.copy(tree = tree)
            }
        }
        externalJob?.cancel()
        externalJob = scope.launch {
            repository.externalChanges.collectLatest { path ->
                delay(250)
                if (path == state.content?.path) refresh()
            }
        }
    }

    fun openNote(path: String, heading: String? = null) {
        scope.launch {
            if (editState.active) {
                if (editState.dirty && !saveEditSuspending(SaveChoice.NORMAL)) return@launch
                editState = EditState()
            }
            loadMutex.withLock {
                val current = state.content
                if (current?.path == path && !current.externalPreview) {
                    if (heading != null) scrollToHeading(current, heading)
                    return@withLock
                }
                openInternal(path, heading, push = true, restoreAnchor = false)
            }
        }
    }

    fun openPath(path: String, heading: String?) {
        val content = state.content
        if (content == null) {
            scope.launch {
                loadMutex.withLock {
                    val resolved = withContext(Dispatchers.IO) {
                        repository.resolveLink("", path)
                    } ?: path
                    if (resolved.endsWith(".md", ignoreCase = true) ||
                        resolved.endsWith(".mmd", ignoreCase = true)
                    ) {
                        openInternal(resolved, heading, push = true, restoreAnchor = false)
                    } else {
                        notice(appContext.getString(io.github.groundgrounder.hortus.R.string.link_not_found))
                    }
                }
            }
            return
        }
        if (path == content.path) {
            if (heading != null) scrollToHeading(content, heading) else Unit
            return
        }
        scope.launch {
            val resolved = withContext(Dispatchers.IO) {
                repository.resolveLink(content.path, path)
            } ?: path
            if (resolved.endsWith(".md", ignoreCase = true) ||
                resolved.endsWith(".mmd", ignoreCase = true)
            ) {
                openNote(resolved, heading)
            } else {
                notice(appContext.getString(io.github.groundgrounder.hortus.R.string.link_not_found))
            }
        }
    }

    fun followLink(link: TextLink) {
        if (link.resolution == "note" && link.resolvedPath != null) {
            lastAction = "link_open:${link.resolvedPath}"
            openNote(link.resolvedPath, link.heading)
        } else if (link.resolution == "attachment") {
            lastAction = "link_attachment:${link.target}"
            notice(appContext.getString(io.github.groundgrounder.hortus.R.string.link_not_found))
        } else {
            lastAction = "link_missing:${link.target}"
            notice(appContext.getString(io.github.groundgrounder.hortus.R.string.link_not_found))
        }
    }

    private suspend fun openInternal(path: String, heading: String?, push: Boolean, restoreAnchor: Boolean) {
        val start = SystemClock.uptimeMillis()
        ReaderTiming.openStartedAt = start
        ReaderTiming.firstFrameAt = 0
        state = state.copy(loading = true, error = null, notice = null)
        val width = if (contentWidthPx > 0) contentWidthPx else defaultWidth()
        try {
            val content = withContext(Dispatchers.IO) {
                repository.openNote(path, width, bodySizePx)
            }
            val destination = Destination(path, heading)
            if (push) {
                stack.add(destination)
            } else if (stack.isEmpty()) {
                stack.add(destination)
            } else {
                stack[stack.lastIndex] = destination
            }
            previewReturn = null
            val pending = pendingScroll
            val target = if (pending != null && pending.first == content.path) {
                pendingScroll = null
                sourceOffsetTarget(content, pending.second)
                    ?: scrollTargetFor(content, heading, restoreAnchor)
            } else {
                scrollTargetFor(content, heading, restoreAnchor)
            }
            lastOpenTarget = target
            state = state.copy(
                loading = false,
                error = null,
                content = content,
                vaultPath = content.vaultPath,
                scrollTarget = target,
                openSequence = state.openSequence + 1,
            )
            ReaderTiming.contentInstalledAt = SystemClock.uptimeMillis()
            preferences.save(content.vaultPath, content.path)
            HitRegistry.clear()
            rememberShortcut(content)
            if (pendingEditPath == content.path) {
                pendingEditPath = null
                startEditing()
            }
        } catch (error: Exception) {
            state = state.copy(
                loading = false,
                error = error.message ?: "document.open_failed",
            )
        }
    }

    private fun scrollTargetFor(content: NoteContent, heading: String?, restoreAnchor: Boolean): ScrollTarget? {
        val anchor = if (restoreAnchor) anchors[content.path] else null
        if (anchor != null) {
            val index = itemIndexForBlock(content, anchor.blockId)
            if (index != null) return ScrollTarget(index, anchor.offsetPx)
        }
        if (heading != null) {
            return headingTarget(content, heading)
        }
        return null
    }

    private fun scrollToHeading(content: NoteContent, heading: String) {
        headingTarget(content, heading)?.let { state = state.copy(scrollTarget = it) }
    }

    private fun headingTarget(content: NoteContent, heading: String): ScrollTarget? {
        val normalized = heading.trim()
        if (normalized.isEmpty()) return null
        val match = content.outline.headings.firstOrNull {
            it.title.trim().equals(normalized, ignoreCase = true) ||
                slug(it.title) == slug(normalized)
        } ?: return null
        val start = match.start.toInt()
        val block = content.model.blocks.firstOrNull {
            start in it.sourceStart until (it.sourceStart + it.sourceLength)
        } ?: return null
        val index = itemIndexForBlock(content, block.id) ?: return null
        return ScrollTarget(index, 0)
    }

    private fun slug(value: String): String = value.trim().lowercase().replace(' ', '-')

    private fun itemIndexForBlock(content: NoteContent, blockId: Long): Int? {
        val index = content.items.indexOfFirst { it.blockId == blockId }
        return index.takeIf { it >= 0 }
    }

    fun consumeScrollTarget() {
        if (state.scrollTarget != null) state = state.copy(scrollTarget = null)
    }

    fun updateAnchor(blockId: Long, offsetPx: Int) {
        lastVisibleBlockId = blockId
        lastVisibleOffsetPx = offsetPx
        val path = state.content?.path ?: return
        lastVisiblePath = path
        anchors[path] = ReaderAnchor(blockId, offsetPx)
    }

    fun restoreTarget(content: NoteContent): ScrollTarget? {
        if (lastVisiblePath != content.path) return null
        val anchor = anchors[content.path] ?: return null
        val index = itemIndexForBlock(content, anchor.blockId) ?: return null
        return ScrollTarget(index, anchor.offsetPx)
    }

    fun setContentWidth(width: Int) {
        if (width <= 0 || width == contentWidthPx) return
        contentWidthPx = width
        val content = state.content ?: return
        widthJob?.cancel()
        widthJob = scope.launch {
            delay(120)
            loadMutex.withLock {
                if (state.content?.path != content.path) return@withLock
                val updated = withContext(Dispatchers.IO) {
                    repository.rebuildForWidth(content, width, bodySizePx)
                }
                if (state.content?.path == updated.path) {
                    state = state.copy(content = updated)
                }
            }
        }
    }

    fun refresh() {
        scope.launch {
            loadMutex.withLock { refreshInternal() }
        }
    }

    suspend fun refreshInternal() {
        val content = state.content ?: return
        val width = if (contentWidthPx > 0) contentWidthPx else defaultWidth()
        if (editState.active && !content.externalPreview) {
            val update = try {
                withContext(Dispatchers.IO) { repository.refreshSession() }
            } catch (_: Exception) {
                null
            } ?: return
            if (editState.dirty) {
                when (update.issue) {
                    DocumentIssue.CONFLICT -> editState = editState.copy(issue = "conflict")
                    DocumentIssue.MISSING -> editState = editState.copy(issue = "missing")
                    else -> Unit
                }
                return
            }
            val text = update.text
            if (text != null) {
                withContext(Dispatchers.IO) { repository.acceptExternal(update.revision.toLong()) }
                editState = editState.copy(
                    field = TextFieldValue(text),
                    savedText = text,
                    dirty = false,
                    issue = null,
                )
            }
            val updated = try {
                withContext(Dispatchers.IO) { repository.refresh(content, width, bodySizePx) }
            } catch (_: Exception) {
                null
            } ?: return
            if (state.content?.path == updated.path) state = state.copy(content = updated)
            return
        }
        val updated = try {
            withContext(Dispatchers.IO) { repository.refresh(content, width, bodySizePx) }
        } catch (_: Exception) {
            null
        } ?: return
        if (state.content?.path != updated.path) return
        state = state.copy(content = updated)
    }

    private fun rememberShortcut(content: NoteContent) {
        if (content.externalPreview) return
        scope.launch(Dispatchers.IO) {
            try {
                val notes = shortcuts.remember(
                    content.vaultPath,
                    content.path,
                    content.title,
                    System.currentTimeMillis(),
                )
                shortcutsPublished = shortcuts.publish(notes)
            } catch (_: Exception) {
                Unit
            }
        }
    }

    fun back(): Boolean {
        if (editState.active) {
            finishEditing()
            return true
        }
        val current = state.content
        if (current?.externalPreview == true) {
            val previous = previewReturn ?: return false
            previewReturn = null
            scope.launch {
                loadMutex.withLock {
                    openInternal(previous.path, null, push = false, restoreAnchor = true)
                }
            }
            return true
        }
        if (stack.size <= 1) return false
        if (current != null) {
            lastVisibleBlockId?.let { anchors[current.path] = ReaderAnchor(it, lastVisibleOffsetPx) }
        }
        stack.removeAt(stack.lastIndex)
        val previous = stack.last()
        scope.launch {
            loadMutex.withLock {
                openInternal(previous.path, null, push = false, restoreAnchor = true)
            }
        }
        return true
    }

    fun canGoBack(): Boolean =
        editState.active ||
            (state.content?.externalPreview == true && previewReturn != null) ||
            stack.size > 1

    fun notice(message: String) {
        state = state.copy(notice = message)
        noticeJob?.cancel()
        noticeJob = scope.launch {
            delay(2600)
            if (state.notice == message) state = state.copy(notice = null)
        }
    }

    fun clearNotice() {
        noticeJob?.cancel()
        state = state.copy(notice = null)
    }

    fun markFirstFrame() {
        if (ReaderTiming.firstFrameAt == 0L && state.content != null) {
            ReaderTiming.firstFrameAt = SystemClock.uptimeMillis()
        }
    }

    fun markFullyDrawn() {
        ReaderTiming.fullyDrawnAt = SystemClock.uptimeMillis()
    }

    fun markActivityCreated() {
        ReaderTiming.activityCreatedAt = SystemClock.uptimeMillis()
    }

    fun recordAction(action: String) {
        lastAction = action
    }

    fun openVaultFromPicker(path: String) {
        start(path, null)
    }

    fun onActivityStop() {
        autoSaveEdit()
    }

    private fun defaultWidth(): Int {
        val metrics = appContext.resources.displayMetrics
        return (metrics.widthPixels - 48 * metrics.density).toInt().coerceAtLeast(1)
    }

    fun repository(): ReaderRepository = repository

    override fun close() {
        refreshJob?.cancel()
        searchJob?.cancel()
        knowledgeJob?.cancel()
        tagJob?.cancel()
        scope.cancel()
        repository.close()
    }
}
