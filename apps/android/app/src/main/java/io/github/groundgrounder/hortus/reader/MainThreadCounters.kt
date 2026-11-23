// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.os.Looper
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * Debug 验收用的主线程计数。每一类重活都经过同一个入口，调用线程由 Looper 判定。
 * 计数在 release 中同样廉价，但不会通过调试入口暴露。
 */
internal object MainThreadCounters {
    val names = listOf(
        "readingModel",
        "mediaBlocks",
        "imageDecode",
        "diagramRender",
        "mathRender",
        "resolveEmbed",
        "vaultScan",
    )
    private val counts = ConcurrentHashMap<String, AtomicInteger>()

    fun increment(name: String) {
        counts.getOrPut(name) { AtomicInteger() }.incrementAndGet()
    }

    fun snapshot(): Map<String, Int> = names.associateWith { counts[it]?.get() ?: 0 }

    fun reset() {
        counts.clear()
    }

    inline fun <T> record(name: String, work: () -> T): T {
        if (Looper.myLooper() == Looper.getMainLooper()) increment(name)
        return work()
    }
}
