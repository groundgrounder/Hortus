// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent
import io.github.groundgrounder.hortus.reader.DebugInspection
import io.github.groundgrounder.hortus.reader.DebugReadingExports
import io.github.groundgrounder.hortus.reader.DebugSessionMetrics
import io.github.groundgrounder.hortus.reader.KnowledgeTab
import io.github.groundgrounder.hortus.reader.SearchTab
import io.github.groundgrounder.hortus.reader.SideMode
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
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
        val initialSequence = activity.controller.state.openSequence
        var openJob: Job? = null
        var expectedVault: String? = null
        var expectedNote: String? = null
        var minSequence = 0L
        if (StoragePermissions.hasAllFilesAccess(activity)) {
            if (vault != null) {
                expectedVault = vault
                expectedNote = note
                val current = activity.controller.state
                if (current.vaultPath == vault && !current.loading && note != null) {
                    openJob = activity.controller.openNote(note, heading)
                    if (current.content?.path != note) {
                        minSequence = initialSequence + 1L
                    }
                } else {
                    openJob = activity.controller.start(vault, note)
                    minSequence = initialSequence + 1L
                }
            } else if (note != null) {
                expectedNote = note
                val current = activity.controller.state
                if (current.vaultPath == null) {
                    val savedVault = activity.controller.preferencesState.vaultPath
                    if (savedVault != null) {
                        expectedVault = savedVault
                        openJob = activity.controller.start(savedVault, note)
                        minSequence = initialSequence + 1L
                    }
                } else {
                    expectedVault = current.vaultPath
                    openJob = activity.controller.openPath(note, heading)
                    if (current.content?.path != note) {
                        minSequence = initialSequence + 1L
                    }
                }
            }
        }
        if (openJob == null && intent.action == Intent.ACTION_VIEW) {
            openJob = activity.pendingIntentJob
            minSequence = initialSequence + 1L
        }
        val action = intent.getStringExtra("debug_action")
        val dump = intent.getStringExtra("debug_dump")
        if (dump != null && dump.matches(Regex("[A-Za-z0-9_.-]{1,120}\\.json"))) {
            val wait = intent.getLongExtra("debug_wait_ms", 900L).coerceIn(0L, 20_000L)
            val isCreate = action == "create"
            val inspectNote = if (isCreate) null else expectedNote
            val inspectVault = if (isCreate) expectedVault else expectedVault
            val inspectMinSeq = if (isCreate) 0L else minSequence
            activity.lifecycleScope.launch {
                if (action != null) {
                    runCatching {
                        perform(activity, intent, action, expectedVault, expectedNote, minSequence, openJob)
                    }
                }
                DebugInspection.write(
                    activity = activity,
                    name = dump,
                    waitMs = wait,
                    expectedNote = inspectNote,
                    expectedVault = inspectVault,
                    minSequence = inspectMinSeq,
                    openJob = if (action == null) openJob else null,
                )
            }
        } else if (action != null) {
            activity.lifecycleScope.launch {
                runCatching {
                    perform(activity, intent, action, expectedVault, expectedNote, minSequence, openJob)
                }
            }
        }
        val export = intent.getStringExtra("debug_export_all")
        if (export != null && export.matches(Regex("[A-Za-z0-9_.-]{1,120}\\.json"))) {
            val wait = intent.getLongExtra("debug_wait_ms", 1500L).coerceIn(0L, 20_000L)
            activity.lifecycleScope.launch {
                openJob?.join()
                awaitContent(activity, expectedVault, expectedNote, minSequence, openJob = null)
                if (wait > 0L) delay(wait)
                DebugReadingExports.exportAll(
                    activity = activity,
                    name = export,
                    expectedNote = expectedNote,
                    expectedVault = expectedVault,
                    minSequence = minSequence,
                    openJob = openJob,
                )
            }
        }
    }

    private suspend fun perform(
        activity: MainActivity,
        intent: Intent,
        action: String,
        expectedVault: String?,
        expectedNote: String?,
        minSequence: Long,
        openJob: Job?,
    ) {
        val controller = activity.controller
        when (action) {
            "search" -> {
                awaitContent(activity, expectedVault, expectedNote, minSequence, openJob)
                controller.openSearch()
                intent.getStringExtra("debug_tab")?.let { tab ->
                    controller.setSearchTab(if (tab == "full") SearchTab.FULL else SearchTab.QUICK)
                }
                intent.getStringExtra("debug_query")?.let { query ->
                    controller.setSearchQuery(android.net.Uri.decode(query))
                }
                controller.awaitSearchIdle()
            }
            "close_search" -> controller.closeSearch()
            "knowledge" -> {
                awaitContent(activity, expectedVault, expectedNote, minSequence, openJob)
                val tab = when (intent.getStringExtra("debug_tab")) {
                    "backlinks" -> KnowledgeTab.BACKLINKS
                    "tags" -> KnowledgeTab.TAGS
                    else -> KnowledgeTab.OUTLINE
                }
                controller.openKnowledge(tab, intent.getStringExtra("debug_tag")?.let(android.net.Uri::decode))
                controller.awaitKnowledgeIdle()
            }
            "close_knowledge" -> controller.closeKnowledge()
            "pane" -> controller.toggleSidePane()
            "finish_edit" -> {
                awaitEdit(activity)
                controller.finishEditing()?.join()
                controller.awaitSaveIdle()
            }
            "side" -> {
                awaitContent(activity, expectedVault, expectedNote, minSequence, openJob)
                val mode = when (intent.getStringExtra("debug_tab")) {
                    "outline" -> SideMode.OUTLINE
                    "backlinks" -> SideMode.BACKLINKS
                    "tags" -> SideMode.TAGS
                    else -> SideMode.FILES
                }
                controller.selectSideMode(mode)
                controller.awaitKnowledgeIdle()
            }
            "tag" -> {
                awaitContent(activity, expectedVault, expectedNote, minSequence, openJob)
                controller.openTagInText(intent.getStringExtra("debug_tag") ?: "")
                controller.awaitKnowledgeIdle()
            }
            "edit" -> {
                awaitContent(activity, expectedVault, expectedNote, minSequence, openJob)
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
                awaitContent(activity, expectedVault, expectedNote, minSequence, openJob)
                controller.openNewNoteDialog(
                    intent.getStringExtra("debug_parent")?.let(android.net.Uri::decode),
                )
                controller.updateNewNoteName(
                    intent.getStringExtra("debug_name")?.let(android.net.Uri::decode) ?: "Note",
                )
                val previousCreated = DebugSessionMetrics.lastCreatedNote
                controller.createNewNote()?.join()
                val created = DebugSessionMetrics.lastCreatedNote
                if (created != null && created != previousCreated &&
                    controller.newNoteState.error == null
                ) {
                    awaitContent(activity, expectedVault = expectedVault, expectedNote = created)
                }
                if (intent.getStringExtra("debug_edit_file") != null ||
                    intent.getStringExtra("debug_save") != null
                ) {
                    awaitEdit(activity, expectedNote = created)
                    applyEditFile(activity, intent)
                    if (intent.getStringExtra("debug_save") != null) {
                        save(activity, intent.getStringExtra("debug_save") ?: "normal")
                    }
                }
            }
            "pin" -> {
                awaitContent(activity, expectedVault, expectedNote, minSequence, openJob)
                controller.pinCurrentNote()?.join()
            }
            "shortcut" -> {
                val previousPending = activity.pendingIntentJob
                val index = intent.getIntExtra("debug_index", 0)
                if (controller.launchShortcut(index)) {
                    var waited = 0
                    while (waited < 5_000 && activity.pendingIntentJob === previousPending) {
                        delay(50)
                        waited += 50
                    }
                    activity.pendingIntentJob?.join()
                    awaitContent(
                        activity,
                        expectedNote = DebugSessionMetrics.lastShortcutLaunchNote,
                    )
                }
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
        val job = when (choice) {
            "disk" -> controller.useDiskVersion()
            "copy" -> controller.saveConflictCopy()
            "keep" -> controller.keepMine()
            else -> controller.saveEditedNote()
        }
        job?.join()
        controller.awaitSaveIdle()
        var waited = 0
        while (waited < 15_000 && controller.editState.saving) {
            delay(50)
            waited += 50
        }
    }

    private suspend fun awaitContent(
        activity: MainActivity,
        expectedVault: String? = null,
        expectedNote: String? = null,
        minSequence: Long = 0L,
        openJob: Job? = null,
    ) {
        openJob?.join()
        var waited = 0
        while (waited < 15_000) {
            val state = activity.controller.state
            val content = state.content
            if (content != null &&
                !state.loading &&
                (expectedVault == null || state.vaultPath == expectedVault) &&
                (expectedNote == null || content.path == expectedNote) &&
                state.openSequence >= minSequence
            ) {
                return
            }
            delay(50)
            waited += 50
        }
    }

    private suspend fun awaitEdit(activity: MainActivity, expectedNote: String? = null) {
        var waited = 0
        while (waited < 15_000) {
            val editState = activity.controller.editState
            val content = activity.controller.state.content
            if (editState.active && (expectedNote == null || content?.path == expectedNote)) {
                return
            }
            delay(50)
            waited += 50
        }
    }
}
