// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.state

import android.content.Context
import android.content.Intent
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.model.NoteContent
import io.github.groundgrounder.hortus.core.model.RecentNote
import io.github.groundgrounder.hortus.core.settings.LaunchExtras
import io.github.groundgrounder.hortus.core.settings.ShortcutStore
import io.github.groundgrounder.hortus.data.VaultRepository
import io.github.groundgrounder.hortus.reader.model.NewNoteState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 管理新建笔记弹窗状态机与最近/固定桌面快捷方式。
 */
internal class NewNoteShortcutController(
    context: Context,
    private val scope: CoroutineScope,
    private val vaultRepository: VaultRepository,
    private val shortcuts: ShortcutStore,
    private val currentContent: () -> NoteContent?,
    private val onNotice: (String) -> Unit,
    private val onCreatedAndOpen: suspend (path: String) -> Unit,
) {
    private val appContext = context.applicationContext
    private val _newNoteState = MutableStateFlow(NewNoteState())
    val newNoteState: StateFlow<NewNoteState> = _newNoteState.asStateFlow()

    var pendingEditPath: String? = null

    fun reset() {
        pendingEditPath = null
        _newNoteState.value = NewNoteState()
    }

    fun openNewNoteDialog(parent: String? = null) {
        val content = currentContent()
        if (content?.externalPreview == true) return
        val defaultParent = parent
            ?: content?.path?.substringBeforeLast('/', "")
            ?: ""
        _newNoteState.value = NewNoteState(open = true, parent = defaultParent)
    }

    fun dismissNewNoteDialog() {
        _newNoteState.value = NewNoteState()
    }

    fun updateNewNoteName(name: String) {
        _newNoteState.value = _newNoteState.value.copy(name = name, error = null)
    }

    fun selectNewNoteParent(parent: String) {
        _newNoteState.value = _newNoteState.value.copy(parent = parent, error = null)
    }

    fun createNewNote(): Job? {
        val dialog = _newNoteState.value
        if (dialog.creating) return null
        val name = dialog.name.trim()
        if (name.isEmpty()) {
            _newNoteState.value = dialog.copy(error = "vault.invalid_name")
            return null
        }
        val parent = dialog.parent
        _newNoteState.value = dialog.copy(creating = true, error = null)
        return scope.launch {
            val entry = try {
                withContext(Dispatchers.IO) {
                    DebugProbes.traceMainThread("createNote") {
                        vaultRepository.createNote(parent, name)
                    }
                }
            } catch (error: Exception) {
                val raw = error.message ?: "vault.create_failed"
                val code = raw.removePrefix("kind=").substringBefore(',')
                _newNoteState.value = _newNoteState.value.copy(creating = false, error = code)
                return@launch
            }
            if (entry == null) {
                _newNoteState.value = _newNoteState.value.copy(creating = false, error = "vault.create_failed")
                return@launch
            }
            DebugProbes.recordCreatedNote(entry.path)
            _newNoteState.value = NewNoteState()
            pendingEditPath = entry.path
            onCreatedAndOpen(entry.path)
        }
    }

    fun pinCurrentNote(): Job? {
        val content = currentContent() ?: return null
        if (content.externalPreview) {
            onNotice(appContext.getString(R.string.shortcut_unsupported))
            return null
        }
        return scope.launch {
            val pinned = shortcuts.requestPin(
                RecentNote(content.vaultPath, content.path, content.title, System.currentTimeMillis()),
            )
            onNotice(
                appContext.getString(
                    if (pinned) R.string.shortcut_pinned else R.string.shortcut_unsupported,
                ),
            )
        }
    }

    fun rememberShortcut(content: NoteContent) {
        if (content.externalPreview) return
        scope.launch(Dispatchers.IO) {
            try {
                val notes = shortcuts.remember(
                    content.vaultPath,
                    content.path,
                    content.title,
                    System.currentTimeMillis(),
                )
                DebugProbes.recordShortcutsPublished(shortcuts.publish(notes))
            } catch (_: Exception) {
                Unit
            }
        }
    }

    fun launchShortcutAsync(index: Int): Job = scope.launch {
        val intent = shortcuts.launchIntent(index) ?: return@launch
        DebugProbes.recordShortcutLaunch(
            intent.getStringExtra(LaunchExtras.EXTRA_OPEN_NOTE),
            intent.getStringExtra(LaunchExtras.EXTRA_OPEN_VAULT),
        )
        appContext.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    fun knownShortcutVaults(): Set<String> = shortcuts.currentNotes().mapTo(HashSet()) { it.vault }

    fun shortcutNotesSync(): List<RecentNote> = shortcuts.loadSync()
}
