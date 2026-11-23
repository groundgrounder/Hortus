// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import java.util.concurrent.ConcurrentHashMap

internal data class TextLayoutKey(
    val itemKey: String,
    val text: String,
    val width: Int,
    val bodySize: Float,
    val paletteHash: Int,
    val density: Float,
)

/** 后台预排普通文本块，滚动时首次组合直接取用已完成的 StaticLayout。 */
internal object TextLayoutCache {
    private const val LIMIT = 1200
    private val entries = ConcurrentHashMap<TextLayoutKey, PreparedText>()

    fun get(key: TextLayoutKey): PreparedText? = entries[key]

    fun put(key: TextLayoutKey, value: PreparedText) {
        if (entries.size >= LIMIT) entries.clear()
        entries[key] = value
    }
}
