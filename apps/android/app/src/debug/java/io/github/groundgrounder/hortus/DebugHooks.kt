// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent
import io.github.groundgrounder.hortus.reader.DebugInspection
import io.github.groundgrounder.hortus.reader.DebugReadingExports
import kotlinx.coroutines.delay
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch

/** Debug 构建的检查入口；Release 源集里的同名对象什么也不做。 */
internal object DebugHooks {
    fun onActivityCreate(activity: MainActivity, intent: Intent) = handle(activity, intent)
    fun onNewIntent(activity: MainActivity, intent: Intent) = handle(activity, intent)

    private fun handle(activity: MainActivity, intent: Intent) {
        fun decode(value: String?): String? = value?.let(android.net.Uri::decode)
        val vault = decode(intent.getStringExtra("debug_vault"))
        val note = decode(intent.getStringExtra("debug_note"))
        if (vault != null) {
            activity.openDebugVault(vault, note)
        } else if (note != null) {
            activity.openDebugNote(note, decode(intent.getStringExtra("debug_heading")))
        }
        val dump = intent.getStringExtra("debug_dump")
        if (dump != null && dump.matches(Regex("[A-Za-z0-9_.-]{1,120}\\.json"))) {
            val wait = intent.getLongExtra("debug_wait_ms", 900L).coerceIn(0L, 20_000L)
            activity.lifecycleScope.launch {
                DebugInspection.write(activity, dump, wait, note)
            }
        }
        val export = intent.getStringExtra("debug_export_all")
        if (export != null && export.matches(Regex("[A-Za-z0-9_.-]{1,120}\\.json"))) {
            val wait = intent.getLongExtra("debug_wait_ms", 1500L).coerceIn(0L, 20_000L)
            activity.lifecycleScope.launch {
                delay(wait)
                DebugReadingExports.exportAll(activity, export, note)
            }
        }
    }
}
