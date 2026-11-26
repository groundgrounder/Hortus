// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.state

import android.os.SystemClock
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.core.SearchCancellation
import io.github.groundgrounder.hortus.core.model.NoteContent
import io.github.groundgrounder.hortus.core.model.QuickRow
import io.github.groundgrounder.hortus.data.DocumentRepository
import io.github.groundgrounder.hortus.data.VaultRepository
import io.github.groundgrounder.hortus.reader.model.KnowledgeState
import io.github.groundgrounder.hortus.reader.model.KnowledgeTab
import io.github.groundgrounder.hortus.reader.model.SearchState
import io.github.groundgrounder.hortus.reader.model.SearchTab
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 独立管理搜索防抖取消（SearchCancellation）与大纲/反链/标签异步加载。
 */
internal class SearchKnowledgeController(
    private val scope: CoroutineScope,
    private val vaultRepository: VaultRepository,
    private val documentRepository: DocumentRepository,
    private val currentContent: () -> NoteContent?,
) {
    private val _searchState = MutableStateFlow(SearchState())
    val searchState: StateFlow<SearchState> = _searchState.asStateFlow()

    private val _knowledgeState = MutableStateFlow(KnowledgeState())
    val knowledgeState: StateFlow<KnowledgeState> = _knowledgeState.asStateFlow()

    private var searchJob: Job? = null
    private var searchCancellation: SearchCancellation? = null
    private var knowledgeJob: Job? = null
    private var tagJob: Job? = null

    fun reset() {
        cancelAll()
        _searchState.value = SearchState()
        _knowledgeState.value = KnowledgeState()
    }

    fun cancelAll() {
        searchJob?.cancel()
        searchCancellation?.cancel()
        searchCancellation = null
        knowledgeJob?.cancel()
        tagJob?.cancel()
    }

    fun openSearch(isEditing: Boolean) {
        if (isEditing) return
        _searchState.value = _searchState.value.copy(open = true)
        runSearch(immediate = true)
    }

    fun closeSearch() {
        searchJob?.cancel()
        searchCancellation?.cancel()
        searchCancellation = null
        _searchState.value = _searchState.value.copy(open = false, running = false)
    }

    fun setSearchTab(tab: SearchTab) {
        if (_searchState.value.tab == tab) return
        _searchState.value = _searchState.value.copy(tab = tab)
        runSearch(immediate = true)
    }

    fun setSearchQuery(query: String) {
        _searchState.value = _searchState.value.copy(query = query)
        runSearch(immediate = false)
    }

    private fun runSearch(immediate: Boolean) {
        searchJob?.cancel()
        searchCancellation?.cancel()
        val prev = _searchState.value
        val hadRunning = prev.running
        val generation = prev.generation + 1
        val query = prev.query
        val tab = prev.tab
        val cancellation = SearchCancellation()
        searchCancellation = cancellation
        _searchState.value = prev.copy(
            generation = generation,
            running = tab == SearchTab.FULL && query.isNotBlank(),
            cancelled = prev.cancelled + if (hadRunning) 1 else 0,
        )
        if (tab == SearchTab.FULL && query.isBlank()) {
            _searchState.value = _searchState.value.copy(full = emptyList(), running = false)
            return
        }
        searchJob = scope.launch {
            try {
                if (!immediate) delay(140)
                val started = SystemClock.uptimeMillis()
                val pair = withContext(Dispatchers.IO) {
                    if (tab == SearchTab.QUICK) {
                        DebugProbes.traceMainThread("quickOpen") {
                            vaultRepository.quickOpen(query, 50)
                        } to emptyList()
                    } else {
                        emptyList<QuickRow>() to DebugProbes.traceMainThread("search") {
                            vaultRepository.search(query, cancellation)
                        }
                    }
                }
                val elapsedMs = SystemClock.uptimeMillis() - started
                DebugProbes.recordSearchMicros(elapsedMs)
                val latest = _searchState.value
                if (generation != latest.generation || !latest.open || latest.tab != tab) {
                    _searchState.value = latest.copy(
                        staleDiscarded = latest.staleDiscarded + 1,
                        running = false,
                    )
                    return@launch
                }
                _searchState.value = latest.copy(
                    quick = pair.first,
                    full = pair.second,
                    running = false,
                )
                val summary = when (tab) {
                    SearchTab.QUICK -> pair.first.firstOrNull()?.let {
                        "quick:${it.path}:${pair.first.size}"
                    } ?: "quick:none"
                    SearchTab.FULL -> pair.second.firstOrNull()?.let {
                        "full:${it.path}:${it.count}:${pair.second.size}"
                    } ?: "full:none"
                }
                DebugProbes.recordQuerySummary(summary, elapsedMs)
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                if (generation == _searchState.value.generation) {
                    _searchState.value = _searchState.value.copy(running = false)
                }
            }
        }
    }

    fun openKnowledge(tab: KnowledgeTab, tag: String? = null, sheet: Boolean = true, isEditing: Boolean = false) {
        if (isEditing) return
        _knowledgeState.value = _knowledgeState.value.copy(open = sheet, tab = tab, selectedTag = tag)
        loadKnowledge(tab, tag)
    }

    fun closeKnowledge() {
        knowledgeJob?.cancel()
        tagJob?.cancel()
        _knowledgeState.value = _knowledgeState.value.copy(open = false)
    }

    fun selectKnowledgeTab(tab: KnowledgeTab) {
        _knowledgeState.value = _knowledgeState.value.copy(tab = tab)
        loadKnowledge(tab, _knowledgeState.value.selectedTag)
    }

    fun selectTag(tag: String) {
        if (tag.isEmpty()) {
            tagJob?.cancel()
            _knowledgeState.value = _knowledgeState.value.copy(selectedTag = null, tagNotes = emptyList())
            return
        }
        _knowledgeState.value = _knowledgeState.value.copy(selectedTag = tag)
        loadTagNotes(tag)
    }

    fun loadKnowledge(tab: KnowledgeTab, tag: String?) {
        val content = currentContent() ?: return
        knowledgeJob?.cancel()
        _knowledgeState.value = _knowledgeState.value.copy(loading = true, tab = tab, selectedTag = tag)
        knowledgeJob = scope.launch {
            try {
                val loaded = withContext(Dispatchers.IO) {
                    val outline = DebugProbes.traceMainThread("outline") {
                        documentRepository.outlineRows()
                    }
                    val backlinks = DebugProbes.traceMainThread("backlinks") {
                        vaultRepository.backlinks(content.path)
                    }
                    val tags = DebugProbes.traceMainThread("tags") { vaultRepository.tags() }
                    Triple(outline, backlinks, tags)
                }
                if (currentContent()?.path != content.path) return@launch
                _knowledgeState.value = _knowledgeState.value.copy(
                    outline = loaded.first,
                    backlinks = loaded.second,
                    tags = loaded.third,
                    loading = false,
                )
                if (tag != null) loadTagNotes(tag)
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                _knowledgeState.value = _knowledgeState.value.copy(loading = false)
            }
        }
    }

    private fun loadTagNotes(tag: String) {
        tagJob?.cancel()
        tagJob = scope.launch {
            try {
                val notes = withContext(Dispatchers.IO) {
                    DebugProbes.traceMainThread("tags") { vaultRepository.notesWithTag(tag) }
                }
                if (_knowledgeState.value.selectedTag == tag) {
                    _knowledgeState.value = _knowledgeState.value.copy(tagNotes = notes)
                }
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                Unit
            }
        }
    }

    suspend fun awaitSearchIdle() {
        searchJob?.join()
    }

    suspend fun awaitKnowledgeIdle() {
        knowledgeJob?.join()
        tagJob?.join()
    }
}
