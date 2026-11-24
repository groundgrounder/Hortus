// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent
import io.github.groundgrounder.hortus.reader.DebugInspection
import io.github.groundgrounder.hortus.reader.DebugReadingExports
import io.github.groundgrounder.hortus.reader.DebugSessionMetrics
import io.github.groundgrounder.hortus.reader.KnowledgeTab
import io.github.groundgrounder.hortus.reader.SearchTab
import io.github.groundgrounder.hortus.reader.SideMode
import kotlinx.coroutines.delay
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** Debug 构建的检查入口；Release 源集里的同名对象什么也不做。 */
internal object DebugHooks {
    fun handlesLaunch(intent: Intent): Boolean =
        intent.getStringExtra("debug_vault") != null ||
            intent.getStringExtra("debug_note") != null

    fun onActivityCreate(activity: MainActivity, intent: Intent) = handle(activity, intent)
    fun onNewIntent(activity: MainActivity, intent: Intent) = handle(activity, intent)

    private fun handle(activity: MainActivity, intent: Intent) {
        fun decode(value: String?): String? = value?.let(android.net.Uri::decode)
        val vault = decode(intent.getStringExtra("debug_vault"))
        val note = decode(intent.getStringExtra("debug_note"))
        val heading = decode(intent.getStringExtra("debug_heading"))
        if (StoragePermissions.hasAllFilesAccess(activity)) {
            if (vault != null) {
                activity.controller.start(vault, note)
            } else if (note != null) {
                if (activity.controller.state.vaultPath == null) {
                    val savedVault = activity.controller.preferencesState.vaultPath
                    if (savedVault != null) activity.controller.start(savedVault, note)
                } else {
                    activity.controller.openPath(note, heading)
                }
            }
        }
        val action = intent.getStringExtra("debug_action")
        val dump = intent.getStringExtra("debug_dump")
        if (dump != null && dump.matches(Regex("[A-Za-z0-9_.-]{1,120}\\.json"))) {
            val wait = intent.getLongExtra("debug_wait_ms", 900L).coerceIn(0L, 20_000L)
            val expected = if (action == "create") null else note
            activity.lifecycleScope.launch {
                if (action != null) runCatching { perform(activity, intent, action) }
                DebugInspection.write(activity, dump, wait, expected)
            }
        } else if (action != null) {
            activity.lifecycleScope.launch {
                runCatching { perform(activity, intent, action) }
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

    private suspend fun perform(activity: MainActivity, intent: Intent, action: String) {
        val controller = activity.controller
        when (action) {
            "search" -> {
                awaitContent(activity)
                controller.openSearch()
                intent.getStringExtra("debug_tab")?.let { tab ->
                    controller.setSearchTab(if (tab == "full") SearchTab.FULL else SearchTab.QUICK)
                }
                intent.getStringExtra("debug_query")?.let { query ->
                    controller.setSearchQuery(android.net.Uri.decode(query))
                }
                delay(320)
            }
            "close_search" -> controller.closeSearch()
            "knowledge" -> {
                awaitContent(activity)
                val tab = when (intent.getStringExtra("debug_tab")) {
                    "backlinks" -> KnowledgeTab.BACKLINKS
                    "tags" -> KnowledgeTab.TAGS
                    else -> KnowledgeTab.OUTLINE
                }
                controller.openKnowledge(tab, intent.getStringExtra("debug_tag")?.let(android.net.Uri::decode))
                delay(500)
            }
            "close_knowledge" -> controller.closeKnowledge()
            "pane" -> controller.toggleSidePane()
            "finish_edit" -> {
                awaitEdit(activity)
                controller.finishEditing()
                delay(400)
            }
            "side" -> {
                awaitContent(activity)
                val mode = when (intent.getStringExtra("debug_tab")) {
                    "outline" -> SideMode.OUTLINE
                    "backlinks" -> SideMode.BACKLINKS
                    "tags" -> SideMode.TAGS
                    else -> SideMode.FILES
                }
                controller.selectSideMode(mode)
                delay(400)
            }
            "tag" -> {
                awaitContent(activity)
                controller.openTagInText(intent.getStringExtra("debug_tag") ?: "")
                delay(500)
            }
            "edit" -> {
                awaitContent(activity)
                controller.startEditing()
                awaitEdit(activity)
                applyEditFile(activity, intent)
                if (intent.getStringExtra("debug_save") != null) {
                    save(activity, intent.getStringExtra("debug_save") ?: "normal")
                }
            }
            "save" -> {
                awaitEdit(activity)
                save(activity, intent.getStringExtra("debug_save") ?: "normal")
            }
            "create" -> {
                awaitContent(activity)
                controller.openNewNoteDialog(
                    intent.getStringExtra("debug_parent")?.let(android.net.Uri::decode),
                )
                controller.updateNewNoteName(
                    intent.getStringExtra("debug_name")?.let(android.net.Uri::decode) ?: "Note",
                )
                delay(120)
                val previousCreated = DebugSessionMetrics.lastCreatedNote
                controller.createNewNote()
                var waited = 0
                while (waited < 15_000 && controller.newNoteState.creating) {
                    delay(50)
                    waited += 50
                }
                val created = DebugSessionMetrics.lastCreatedNote
                if (created != null && created != previousCreated &&
                    controller.newNoteState.error == null
                ) {
                    while (waited < 20_000 && controller.state.content?.path != created) {
                        delay(50)
                        waited += 50
                    }
                }
                if (intent.getStringExtra("debug_edit_file") != null ||
                    intent.getStringExtra("debug_save") != null
                ) {
                    awaitEdit(activity)
                    applyEditFile(activity, intent)
                    if (intent.getStringExtra("debug_save") != null) {
                        save(activity, intent.getStringExtra("debug_save") ?: "normal")
                    }
                }
                delay(300)
            }
            "pin" -> {
                awaitContent(activity)
                controller.pinCurrentNote()
                delay(300)
            }
            "shortcut" -> {
                val index = intent.getIntExtra("debug_index", 0)
                controller.launchShortcut(index)
                delay(800)
            }
        }
    }

    private suspend fun applyEditFile(activity: MainActivity, intent: Intent) {
        val name = intent.getStringExtra("debug_edit_file") ?: return
        if (!name.matches(Regex("[A-Za-z0-9_.-]{1,120}\\.txt"))) return
        val directory = activity.getExternalFilesDir(null) ?: return
        val text = withContext(Dispatchers.IO) {
            File(directory, name).takeIf { it.isFile }?.readText()
        } ?: return
        activity.controller.updateEditText(
            androidx.compose.ui.text.input.TextFieldValue(
                text,
                androidx.compose.ui.text.TextRange(text.length),
            ),
        )
    }

    private suspend fun save(activity: MainActivity, choice: String) {
        val controller = activity.controller
        when (choice) {
            "disk" -> controller.useDiskVersion()
            "copy" -> controller.saveConflictCopy()
            "keep" -> controller.keepMine()
            else -> controller.saveEditedNote()
        }
        var waited = 0
        while (waited < 15_000 && controller.editState.saving) {
            delay(50)
            waited += 50
        }
        delay(300)
    }

    private suspend fun awaitContent(activity: MainActivity) {
        var waited = 0
        while (waited < 15_000) {
            val state = activity.controller.state
            if (state.content != null && !state.loading) return
            delay(50)
            waited += 50
        }
    }

    private suspend fun awaitEdit(activity: MainActivity) {
        var waited = 0
        while (waited < 15_000 && !activity.controller.editState.active) {
            delay(50)
            waited += 50
        }
    }
}
