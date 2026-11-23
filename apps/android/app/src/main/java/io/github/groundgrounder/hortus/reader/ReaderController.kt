// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import android.os.SystemClock
import android.util.TypedValue
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
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

    val bodySizePx: Float = TypedValue.applyDimension(
        TypedValue.COMPLEX_UNIT_SP,
        17f,
        appContext.resources.displayMetrics,
    )
    var contentWidthPx: Int = 0
        private set
    var lastVisibleBlockId: Long? = null
        private set
    var lastVisibleOffsetPx: Int = 0
        private set
    var lastAction: String = ""
        private set
    var lastOpenTarget: ScrollTarget? = null
        private set

    val preferencesState: ReaderPreferenceState get() = preferences.load()

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
        }
    }

    fun start(vaultPath: String?, notePath: String?) {
        if (vaultPath == null) return
        scope.launch {
            loadMutex.withLock {
                ReaderTiming.openCount += 1
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
            loadMutex.withLock {
                val current = state.content
                if (current?.path == path) {
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
            openNote(path, heading)
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
        OpenFrameMonitor.begin()
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
            val target = scrollTargetFor(content, heading, restoreAnchor)
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
        state.content?.path?.let { anchors[it] = ReaderAnchor(blockId, offsetPx) }
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
        val updated = try {
            withContext(Dispatchers.IO) { repository.refresh(content, width, bodySizePx) }
        } catch (_: Exception) {
            null
        } ?: return
        if (state.content?.path != updated.path) return
        state = state.copy(content = updated)
    }

    fun back(): Boolean {
        if (stack.size <= 1) return false
        val current = state.content
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

    fun canGoBack(): Boolean = stack.size > 1

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

    fun markActivityCreated() {
        ReaderTiming.activityCreatedAt = SystemClock.uptimeMillis()
    }

    fun recordAction(action: String) {
        lastAction = action
    }

    fun openVaultFromPicker(path: String) {
        start(path, null)
    }

    private fun defaultWidth(): Int {
        val metrics = appContext.resources.displayMetrics
        return (metrics.widthPixels - 48 * metrics.density).toInt().coerceAtLeast(1)
    }

    fun repository(): ReaderRepository = repository

    override fun close() {
        refreshJob?.cancel()
        scope.cancel()
        repository.close()
    }
}
