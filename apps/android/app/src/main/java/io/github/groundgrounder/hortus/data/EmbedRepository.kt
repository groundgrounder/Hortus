// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.data

import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.core.EmbedContentInfo
import io.github.groundgrounder.hortus.core.model.EmbeddedContent
import io.github.groundgrounder.hortus.core.model.buildEmbeddedItems
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicLong

/**
 * 笔记与图表内嵌解析与 LRU 缓存仓储，键含 vaultPath 与失效版本号。
 */
internal class EmbedRepository(
    private val vaultRepository: VaultRepository,
) {
    private val mutex = Mutex()
    private val version = AtomicLong(1L)
    private val _mutationVersion = MutableStateFlow(1L)
    val mutationVersion: StateFlow<Long> = _mutationVersion.asStateFlow()
    private val cache = object : LinkedHashMap<String, EmbeddedContent>(24, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, EmbeddedContent>): Boolean =
            size > 16
    }

    fun invalidate() {
        val next = version.incrementAndGet()
        _mutationVersion.value = next
        synchronized(cache) { cache.clear() }
    }

    suspend fun resolveEmbed(
        sourcePath: String,
        target: String,
        heading: String?,
        alias: String?,
        ancestors: List<String>,
        width: Double,
        size: Double,
    ): EmbeddedContent = withContext(Dispatchers.IO) {
        mutex.withLock {
            val root = vaultRepository.vaultRoot ?: ""
            val currentVersion = version.get()
            val key = "$root|$currentVersion|$sourcePath|$target|$heading|$alias|${ancestors.joinToString(">")}|${width.toInt()}|${size.toInt()}"
            synchronized(cache) { cache[key]?.let { return@withLock it } }
            val info = DebugProbes.traceMainThread("resolveEmbed") {
                vaultRepository.resolveEmbedRaw(
                    sourcePath = sourcePath,
                    target = target,
                    heading = heading,
                    alias = alias,
                    ancestors = ancestors,
                    width = width,
                    size = size,
                )
            } ?: return@withLock brokenEmbed(target, heading)
            val content = buildEmbeddedItems(info, info.resolvedPath ?: target)
            synchronized(cache) { cache[key] = content }
            content
        }
    }

    private fun brokenEmbed(target: String, heading: String?): EmbeddedContent = EmbeddedContent(
        EmbedContentInfo(
            kind = "broken",
            resolvedPath = null,
            displayTitle = target,
            heading = heading,
            headingFound = false,
            isMmd = false,
            content = "",
            text = "",
            blocks = emptyList(),
            media = emptyList(),
            math = emptyList(),
            diagrams = emptyList(),
            links = emptyList(),
            tags = emptyList(),
            candidates = emptyList(),
        ),
        emptyList(),
        emptyList(),
        emptyList(),
    )
}
