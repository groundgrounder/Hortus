// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.state

import android.content.Context
import android.util.TypedValue
import io.github.groundgrounder.hortus.core.settings.AppSettings
import io.github.groundgrounder.hortus.core.settings.AppSettingsRepository
import io.github.groundgrounder.hortus.core.settings.MAX_READER_BODY_SP
import io.github.groundgrounder.hortus.core.settings.MIN_READER_BODY_SP
import io.github.groundgrounder.hortus.core.settings.ThemeMode
import io.github.groundgrounder.hortus.data.DiagramRenderService
import io.github.groundgrounder.hortus.data.DocumentRepository
import io.github.groundgrounder.hortus.data.VaultRepository
import io.github.groundgrounder.hortus.reader.model.ReaderUiState
import io.github.groundgrounder.hortus.reader.render.DiagramPreviewCache
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * 协调阅读器偏好（字号/主题/缓存清理）、仓库目录树与外部变更监听、宽度防抖重排及短提示计时。
 */
internal class ReaderSyncController(
    private val context: Context,
    private val scope: CoroutineScope,
    private val settingsRepository: AppSettingsRepository,
    private val diagramService: DiagramRenderService,
    private val vaultRepository: VaultRepository,
    private val documentRepository: DocumentRepository,
    private val embedVersion: StateFlow<Long>,
    private val loadMutex: Mutex,
    private val readerState: MutableStateFlow<ReaderUiState>,
    private val onClearCaches: suspend () -> Unit,
    private val isEditorActive: () -> Boolean,
    private val onHandleExternalRefreshWhileEditing: suspend () -> Boolean,
) {
    private var treeJob: Job? = null
    private var embedJob: Job? = null
    private var externalJob: Job? = null
    private var widthJob: Job? = null
    private var fontSizeJob: Job? = null
    private var noticeJob: Job? = null
    private var refreshJob: Job? = null

    private val _cacheClearedCount = MutableStateFlow(0L)
    val cacheClearedCount: StateFlow<Long> = _cacheClearedCount

    var contentWidthPx: Int = 0
        private set

    val appSettings: StateFlow<AppSettings> = settingsRepository.settings

    val bodySizePx: StateFlow<Float> = settingsRepository.settings
        .map { spToPx(it.fontSizeSp) }
        .stateIn(
            scope = scope,
            started = SharingStarted.Eagerly,
            initialValue = spToPx(settingsRepository.settings.value.fontSizeSp),
        )

    private fun spToPx(sp: Float): Float = TypedValue.applyDimension(
        TypedValue.COMPLEX_UNIT_SP,
        sp,
        context.resources.displayMetrics,
    )

    fun resolvedWidthPx(): Int {
        if (contentWidthPx > 0) return contentWidthPx
        val metrics = context.resources.displayMetrics
        return (metrics.widthPixels - 48 * metrics.density).toInt().coerceAtLeast(1)
    }

    fun setThemeMode(mode: ThemeMode): Job = scope.launch {
        settingsRepository.setThemeMode(mode)
    }

    fun setFontSizeSp(fontSizeSp: Float): Job {
        val clamped = fontSizeSp.coerceIn(MIN_READER_BODY_SP, MAX_READER_BODY_SP)
        fontSizeJob?.cancel()
        val job = scope.launch {
            settingsRepository.setFontSizeSp(clamped)
            DiagramPreviewCache.reset()
            diagramService.clearCache()
            val sizePx = spToPx(clamped)
            val width = resolvedWidthPx()
            loadMutex.withLock {
                val content = readerState.value.content ?: return@withLock
                val updated = withContext(Dispatchers.IO) {
                    documentRepository.rebuildForWidth(content, width, sizePx)
                }
                if (readerState.value.content?.path == updated.path) {
                    readerState.value = readerState.value.copy(content = updated)
                }
            }
        }
        fontSizeJob = job
        return job
    }

    fun clearCaches(): Job = scope.launch {
        onClearCaches()
        _cacheClearedCount.value += 1L
    }

    fun observeRepository() {
        treeJob?.cancel()
        treeJob = scope.launch {
            vaultRepository.tree.collect { tree ->
                readerState.value = readerState.value.copy(tree = tree)
            }
        }
        embedJob?.cancel()
        embedJob = scope.launch {
            embedVersion.collect { version ->
                readerState.value = readerState.value.copy(embedVersion = version)
            }
        }
        externalJob?.cancel()
        externalJob = scope.launch {
            vaultRepository.externalChanges
                .filter { path -> path == readerState.value.content?.path }
                .collect { path ->
                    refreshJob?.cancel()
                    refreshJob = scope.launch {
                        delay(250)
                        if (path == readerState.value.content?.path) {
                            loadMutex.withLock { refreshInternal() }
                        }
                    }
                }
        }
    }

    fun setContentWidth(width: Int) {
        if (width <= 0 || width == contentWidthPx) return
        contentWidthPx = width
        val initial = readerState.value.content ?: return
        widthJob?.cancel()
        widthJob = scope.launch {
            delay(120)
            loadMutex.withLock {
                val content = readerState.value.content ?: return@withLock
                if (content.path != initial.path) return@withLock
                val sizePx = spToPx(settingsRepository.settings.value.fontSizeSp)
                val updated = documentRepository.rebuildForWidth(content, width, sizePx)
                if (readerState.value.content?.path == updated.path) {
                    readerState.value = readerState.value.copy(content = updated)
                }
            }
        }
    }

    fun refresh() {
        scope.launch { loadMutex.withLock { refreshInternal() } }
    }

    suspend fun refreshInternal() {
        val content = readerState.value.content ?: return
        val width = resolvedWidthPx()
        val sizePx = spToPx(settingsRepository.settings.value.fontSizeSp)
        if (isEditorActive() && !content.externalPreview) {
            if (!onHandleExternalRefreshWhileEditing()) return
        }
        val updated = try {
            documentRepository.refresh(content, width, sizePx)
        } catch (_: Exception) {
            null
        } ?: return
        if (readerState.value.content?.path == updated.path) {
            readerState.value = readerState.value.copy(content = updated)
        }
    }

    fun notice(message: String) {
        readerState.value = readerState.value.copy(notice = message)
        noticeJob?.cancel()
        noticeJob = scope.launch {
            delay(2600)
            if (readerState.value.notice == message) {
                readerState.value = readerState.value.copy(notice = null)
            }
        }
    }

    fun clearNotice() {
        noticeJob?.cancel()
        readerState.value = readerState.value.copy(notice = null)
    }

    fun cancelAll() {
        treeJob?.cancel()
        embedJob?.cancel()
        externalJob?.cancel()
        refreshJob?.cancel()
        widthJob?.cancel()
        fontSizeJob?.cancel()
        noticeJob?.cancel()
    }
}
