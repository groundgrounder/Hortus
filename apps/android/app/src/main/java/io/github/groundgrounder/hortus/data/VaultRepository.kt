// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.data

import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.core.EmbedContentInfo
import io.github.groundgrounder.hortus.core.KnowledgeMarkupInfo
import io.github.groundgrounder.hortus.core.SearchCancellation
import io.github.groundgrounder.hortus.core.TreeBatch
import io.github.groundgrounder.hortus.core.TreeEntry
import io.github.groundgrounder.hortus.core.VaultHandle
import io.github.groundgrounder.hortus.core.WorkspaceCore
import io.github.groundgrounder.hortus.core.model.BacklinkRow
import io.github.groundgrounder.hortus.core.model.QuickRow
import io.github.groundgrounder.hortus.core.model.SearchRow
import io.github.groundgrounder.hortus.core.model.SearchSnippet
import io.github.groundgrounder.hortus.core.model.TagNoteRow
import io.github.groundgrounder.hortus.core.model.TagRow
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/**
 * 统一串行持有 VaultHandle，负责目录扫描、全文检索与反链标签索引。
 * 使用自身独立锁，与 DocumentSession / 图表渲染 / 公式渲染彻底解耦。
 */
internal class VaultRepository(
    private val openVaultHandle: suspend (String) -> VaultHandle,
    private val registerActiveVault: (String, suspend (String, String) -> String?) -> Unit,
    private val unregisterActiveVault: (String) -> Unit,
    private val onVaultReset: () -> Unit,
    private val onVaultMutated: () -> Unit,
) : AutoCloseable {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val lifecycleMutex = Mutex()
    private val queryMutex = Mutex()

    @Volatile
    private var vault: VaultHandle? = null

    @Volatile
    private var vaultPath: String? = null

    private var scanJob: Job? = null
    private var scanReady = CompletableDeferred<Unit>()
    private val vaultGeneration = AtomicLong(0L)
    private val entries = ConcurrentHashMap<String, TreeEntry>()

    private val _tree = MutableStateFlow<List<TreeEntry>>(emptyList())
    val tree: StateFlow<List<TreeEntry>> = _tree.asStateFlow()

    private val _externalChanges = MutableSharedFlow<String>(extraBufferCapacity = 64)
    val externalChanges: SharedFlow<String> = _externalChanges.asSharedFlow()

    val vaultRoot: String? get() = vaultPath

    suspend fun openVault(path: String) = withContext(Dispatchers.IO) {
        lifecycleMutex.withLock {
            if (vaultPath == path && vault != null) return@withLock
            queryMutex.withLock {
                vaultPath?.let(unregisterActiveVault)
                vaultGeneration.incrementAndGet()
                stopScan()
                vault?.stopWatching()
                vault?.close()
                vault = null
                vaultPath = null
                entries.clear()
                _tree.value = emptyList()
                onVaultReset()
                val handle = openVaultHandle(path)
                val generation = vaultGeneration.incrementAndGet()
                vault = handle
                vaultPath = path
                registerActiveVault(path) { source, target -> resolveLink(source, target) }
                scanReady = CompletableDeferred()
                scanJob = scope.launch { scan(handle, generation) }
            }
        }
    }

    private fun scan(handle: VaultHandle, generation: Long) {
        try {
            while (vaultGeneration.get() == generation && vault === handle) {
                val batches: List<TreeBatch> = DebugProbes.traceMainThread("vaultScan") {
                    handle.waitForBatches()
                }
                if (batches.isEmpty()) break
                if (vaultGeneration.get() != generation || vault !== handle) break
                var changed = false
                for (batch in batches) {
                    if (vaultGeneration.get() != generation || vault !== handle) break
                    if (batch.upserted.isNotEmpty() || batch.removed.isNotEmpty()) {
                        handle.updateSearchIndex(
                            batch.upserted.map { it.path },
                            batch.removed,
                        )
                        onVaultMutated()
                    }
                    for (entry in batch.upserted) {
                        entries[entry.path] = entry
                        changed = true
                        _externalChanges.tryEmit(entry.path)
                    }
                    for (removed in batch.removed) {
                        entries.remove(removed)
                        changed = true
                        _externalChanges.tryEmit(removed)
                    }
                    if (batch.scanComplete) scanReady.complete(Unit)
                }
                if (changed && vaultGeneration.get() == generation && vault === handle) {
                    _tree.value = entries.values.sortedWith(
                        compareBy<TreeEntry> { !it.directory }
                            .thenBy { it.path.lowercase() },
                    )
                }
            }
        } catch (_: Exception) {
            // 关闭库或进程退出时阻塞调用会失败；扫描线程不再有用。
        }
    }

    suspend fun awaitScan() {
        withTimeoutOrNull(15_000) { scanReady.await() }
    }

    private fun stopScan() {
        scanJob?.cancel()
        scanJob = null
    }

    suspend fun knowledgeMarkup(path: String, text: String): KnowledgeMarkupInfo =
        withContext(Dispatchers.IO) {
            queryMutex.withLock {
                vault?.knowledgeMarkup(path, text)
                    ?: KnowledgeMarkupInfo(emptyList(), emptyList())
            }
        }

    suspend fun upsertNoteContent(path: String, text: String) = withContext(Dispatchers.IO) {
        queryMutex.withLock {
            vault?.upsertNoteContent(path, text)
        }
    }

    suspend fun resolveLink(sourcePath: String, rawTarget: String): String? =
        withContext(Dispatchers.IO) {
            queryMutex.withLock {
                try {
                    vault?.resolveLink(sourcePath, rawTarget)?.resolvedPath
                } catch (_: Exception) {
                    null
                }
            }
        }

    suspend fun readNote(path: String): String? = withContext(Dispatchers.IO) {
        queryMutex.withLock {
            try {
                vault?.readNote(path)
            } catch (_: Exception) {
                null
            }
        }
    }

    suspend fun quickOpen(query: String, limit: Int = 50): List<QuickRow> =
        withContext(Dispatchers.IO) {
            queryMutex.withLock {
                vault?.quickOpen(query, limit.toUInt())?.map { hit ->
                    QuickRow(
                        path = hit.path,
                        title = hit.title,
                        titleMatches = hit.titleMatches.mapNotNull { range ->
                            range.length.toInt().takeIf { it > 0 }?.let {
                                range.start.toInt() until range.start.toInt() + it
                            }
                        },
                        pathMatches = hit.pathMatches.mapNotNull { range ->
                            range.length.toInt().takeIf { it > 0 }?.let {
                                range.start.toInt() until range.start.toInt() + it
                            }
                        },
                    )
                } ?: emptyList()
            }
        }

    suspend fun search(query: String, cancellation: SearchCancellation): List<SearchRow> =
        withContext(Dispatchers.IO) {
            val handle = vault ?: return@withContext emptyList()
            handle.search(query, 8u, cancellation).map { result ->
                SearchRow(
                    path = result.path,
                    title = result.title,
                    count = result.count.toInt(),
                    matches = result.matches.map { matched ->
                        SearchSnippet(
                            start = matched.start.toInt(),
                            length = matched.length.toInt(),
                            snippet = matched.snippet,
                            snippetStart = matched.snippetStart.toInt(),
                        )
                    },
                )
            }
        }

    suspend fun backlinks(path: String): List<BacklinkRow> = withContext(Dispatchers.IO) {
        queryMutex.withLock {
            vault?.backlinks(path)?.map { link ->
                BacklinkRow(
                    source = link.source,
                    title = titleFor(link.source),
                    line = link.line.toInt(),
                    context = link.context,
                    inDiagram = link.inDiagram,
                    embed = link.embed,
                    startUtf16 = link.startUtf16.toInt(),
                    lengthUtf16 = link.lengthUtf16.toInt(),
                )
            } ?: emptyList()
        }
    }

    suspend fun tags(): List<TagRow> = withContext(Dispatchers.IO) {
        queryMutex.withLock {
            vault?.tags()?.map { summary ->
                TagRow(
                    tag = summary.tag,
                    raw = summary.raw,
                    count = summary.count.toInt(),
                    notes = summary.notes,
                )
            } ?: emptyList()
        }
    }

    suspend fun notesWithTag(tag: String): List<TagNoteRow> = withContext(Dispatchers.IO) {
        queryMutex.withLock {
            vault?.notesWithTag(tag)?.map { path -> TagNoteRow(path, titleFor(path)) } ?: emptyList()
        }
    }

    suspend fun resolveEmbedRaw(
        sourcePath: String,
        target: String,
        heading: String?,
        alias: String?,
        ancestors: List<String>,
        width: Double,
        size: Double,
    ): EmbedContentInfo? = withContext(Dispatchers.IO) {
        queryMutex.withLock {
            vault?.resolveEmbed(
                sourcePath = sourcePath,
                rawTarget = target,
                heading = heading,
                alias = alias,
                ancestors = ancestors,
                maxDepth = 8u,
                width = width,
                size = size,
            )
        }
    }

    suspend fun createNote(parent: String, name: String): TreeEntry? = withContext(Dispatchers.IO) {
        lifecycleMutex.withLock {
            queryMutex.withLock {
                vault?.createNote(parent, name)?.also { entry ->
                    entries[entry.path] = entry
                    _tree.value = entries.values.sortedWith(
                        compareBy<TreeEntry> { !it.directory }.thenBy { it.path.lowercase() },
                    )
                    vault?.upsertNoteContent(entry.path, "")
                    onVaultMutated()
                }
            }
        }
    }

    private fun titleFor(path: String): String =
        entries[path]?.name?.substringBeforeLast('.')
            ?: path.substringAfterLast('/').substringBeforeLast('.')

    override fun close() {
        vaultPath?.let(unregisterActiveVault)
        vaultGeneration.incrementAndGet()
        stopScan()
        try {
            vault?.stopWatching()
            vault?.close()
        } catch (_: Exception) {
            Unit
        }
        vault = null
        vaultPath = null
        entries.clear()
        scope.cancel()
    }
}
