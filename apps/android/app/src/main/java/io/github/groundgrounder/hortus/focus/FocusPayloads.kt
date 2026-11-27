// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.focus

data class FocusPayload(
    val vaultPath: String,
    val notePath: String,
    val documentKey: String,
    val engineText: String,
    val mmd: Boolean,
    val ordinal: Int,
    val revision: Long,
    val dark: Boolean,
    val collapsed: List<String>,
    val title: String,
    val renderWidth: Double,
)

object FocusPayloads {
    internal const val MAX_PAYLOADS = 8
    private var next = 1
    private val payloads = object : LinkedHashMap<Int, FocusPayload>(16, 0.75f, false) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Int, FocusPayload>): Boolean =
            size > MAX_PAYLOADS
    }

    fun put(payload: FocusPayload): Int = synchronized(this) {
        val token = next++
        payloads[token] = payload
        token
    }

    fun get(token: Int): FocusPayload? = synchronized(this) { payloads[token] }

    fun remove(token: Int): FocusPayload? = synchronized(this) { payloads.remove(token) }

    fun clear() = synchronized(this) { payloads.clear() }
}
