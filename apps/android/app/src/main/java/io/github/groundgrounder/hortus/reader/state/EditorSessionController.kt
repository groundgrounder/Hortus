// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.state

import android.content.Context
import android.os.SystemClock
import androidx.compose.ui.text.input.TextFieldValue
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.DocumentIssue
import io.github.groundgrounder.hortus.core.DocumentUpdate
import io.github.groundgrounder.hortus.core.SaveChoice
import io.github.groundgrounder.hortus.core.model.NoteContent
import io.github.groundgrounder.hortus.data.DocumentRepository
import io.github.groundgrounder.hortus.reader.model.EditState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * 独立管理纯文本轻编辑状态机、单飞 saveMutex 保存队列、自动保存与冲突处理。
 */
internal class EditorSessionController(
    context: Context,
    private val scope: CoroutineScope,
    private val documentRepository: DocumentRepository,
    private val loadMutex: Mutex,
    private val onNotice: (String) -> Unit,
    private val onRefreshLocked: suspend () -> Unit,
    private val onOpenConflictCopyLocked: suspend (String) -> Unit,
) {
    private val appContext = context.applicationContext
    private val saveMutex = Mutex()
    private var activeSaveJob: Job? = null
    @Volatile
    private var pendingAutoSaveAfterComposition = false

    private val _editState = MutableStateFlow(EditState())
    val editState: StateFlow<EditState> = _editState.asStateFlow()

    val isActive: Boolean get() = _editState.value.active
    val isDirty: Boolean get() = _editState.value.dirty

    fun reset() {
        pendingAutoSaveAfterComposition = false
        _editState.value = EditState()
    }

    fun startEditing(content: NoteContent, canEdit: Boolean) {
        if (!canEdit) {
            onNotice(
                appContext.getString(
                    R.string.editor_read_only_reason,
                    content.readOnlyReason ?: "external",
                ),
            )
            return
        }
        pendingAutoSaveAfterComposition = false
        _editState.value = EditState(
            active = true,
            field = TextFieldValue(content.sourceText),
            savedText = content.sourceText,
            sessionId = SystemClock.uptimeMillis(),
            readOnlyReason = content.readOnlyReason,
        )
        DebugProbes.recordAction("edit_start:${content.path}")
    }

    fun updateEditText(value: TextFieldValue) {
        val current = _editState.value
        if (!current.active) return
        val dirty = value.text != current.savedText
        _editState.value = current.copy(
            field = value,
            dirty = dirty,
        )
        if (pendingAutoSaveAfterComposition && value.composition == null) {
            pendingAutoSaveAfterComposition = false
            if (dirty) autoSaveEdit()
        }
    }

    fun finishEditing(): Job? {
        val current = _editState.value
        if (!current.active) return null
        val job = scope.launch {
            while (_editState.value.active && (_editState.value.dirty || _editState.value.issue != null)) {
                if (!saveEditSuspending(SaveChoice.NORMAL)) return@launch
            }
            if (_editState.value.active && !_editState.value.dirty && _editState.value.issue == null) {
                pendingAutoSaveAfterComposition = false
                _editState.value = _editState.value.copy(active = false)
                onNotice(appContext.getString(R.string.editor_saved))
            }
        }
        activeSaveJob = job
        return job
    }

    fun saveEditedNote(): Job? {
        if (!_editState.value.active) return null
        val job = scope.launch {
            if (saveEditSuspending(SaveChoice.NORMAL)) {
                onNotice(appContext.getString(R.string.editor_saved))
            }
        }
        activeSaveJob = job
        return job
    }

    fun keepMine(): Job? {
        val current = _editState.value
        if (!current.active) return null
        val choice = if (current.issue == "missing") SaveChoice.RECREATE else SaveChoice.KEEP_MINE
        val job = scope.launch {
            if (saveEditSuspending(choice)) {
                _editState.value = _editState.value.copy(active = false)
                onNotice(appContext.getString(R.string.editor_saved))
            }
        }
        activeSaveJob = job
        return job
    }

    fun useDiskVersion(): Job? {
        if (!_editState.value.active) return null
        val job = scope.launch {
            saveMutex.withLock {
                if (!_editState.value.active) return@withLock
                _editState.value = _editState.value.copy(saving = true)
                val update = try {
                    loadMutex.withLock {
                        withContext(Dispatchers.IO) { documentRepository.useDiskVersion() }
                    }
                } catch (_: Exception) {
                    null
                }
                val text = update?.text
                if (text != null) {
                    _editState.value = _editState.value.copy(
                        active = false,
                        field = TextFieldValue(text),
                        savedText = text,
                        dirty = false,
                        saving = false,
                        issue = null,
                        lastSaveOk = false,
                    )
                    loadMutex.withLock { onRefreshLocked() }
                    onNotice(appContext.getString(R.string.editor_disk_loaded))
                } else {
                    _editState.value = _editState.value.copy(saving = false, issue = "save_failed")
                }
            }
        }
        activeSaveJob = job
        return job
    }

    fun saveConflictCopy(): Job? {
        if (!_editState.value.active) return null
        val job = scope.launch {
            if (saveEditSuspending(SaveChoice.CONFLICT_COPY)) {
                _editState.value = _editState.value.copy(active = false)
            }
        }
        activeSaveJob = job
        return job
    }

    fun autoSaveEdit(): Job? {
        val current = _editState.value
        if (!current.active || !current.dirty || current.saving) return null
        if (current.field.composition != null) {
            pendingAutoSaveAfterComposition = true
            return null
        }
        val job = scope.launch { saveEditSuspending(SaveChoice.NORMAL, skipIfComposing = true) }
        activeSaveJob = job
        return job
    }

    suspend fun flushBeforeNavigation(): Boolean {
        val ok = saveMutex.withLock {
            var attempts = 0
            while (_editState.value.active && (_editState.value.dirty || _editState.value.issue != null)) {
                if (attempts >= 3) return@withLock false
                attempts += 1
                if (!saveEditLocked(SaveChoice.NORMAL, skipIfComposing = false)) {
                    return@withLock false
                }
            }
            if (!_editState.value.active || (!_editState.value.dirty && _editState.value.issue == null)) {
                pendingAutoSaveAfterComposition = false
                _editState.value = EditState()
                true
            } else {
                false
            }
        }
        return ok
    }

    suspend fun saveEditSuspending(
        choice: SaveChoice,
        skipIfComposing: Boolean = false,
    ): Boolean {
        var needsResave = false
        val result = saveMutex.withLock {
            val ok = saveEditLocked(choice, skipIfComposing)
            val latest = _editState.value
            if (ok && latest.active && latest.dirty && latest.issue == null) {
                needsResave = true
            }
            ok
        }
        if (needsResave) {
            autoSaveEdit()
        }
        return result
    }

    private suspend fun saveEditLocked(choice: SaveChoice, skipIfComposing: Boolean): Boolean {
        val current = _editState.value
        if (!current.active) return true
        if (skipIfComposing && current.field.composition != null) {
            pendingAutoSaveAfterComposition = true
            return false
        }
        if (choice == SaveChoice.NORMAL && !current.dirty && current.issue == null) {
            return true
        }
        val text = current.field.text
        val sessionId = current.sessionId
        _editState.value = current.copy(saving = true, issue = null, choice = choice.name)
        val temporary = (appContext.externalCacheDir ?: appContext.cacheDir).path
        val update: DocumentUpdate? = try {
            loadMutex.withLock {
                withContext(Dispatchers.IO) {
                    DebugProbes.traceMainThread("save") {
                        documentRepository.replaceAndSave(text, temporary, choice)
                    }
                }
            }
        } catch (error: CancellationException) {
            throw error
        } catch (_: Exception) {
            null
        }
        if (update == null) {
            DebugProbes.recordSaveIssue("io.failure")
            _editState.value = _editState.value.copy(saving = false, issue = "save_failed", lastSaveOk = false)
            return false
        }
        DebugProbes.recordSaveIssue(update.issue.name)
        val copy = update.copyPath
        if (copy != null) {
            val diskUpdate = if (update.issue == DocumentIssue.MISSING) {
                null
            } else {
                try {
                    loadMutex.withLock {
                        withContext(Dispatchers.IO) { documentRepository.useDiskVersion() }
                    }
                } catch (_: Exception) {
                    null
                }
            }
            if (diskUpdate?.text == null) {
                pendingAutoSaveAfterComposition = false
                _editState.value = _editState.value.copy(
                    active = false,
                    saving = false,
                    dirty = false,
                    copyPath = copy,
                    lastSaveOk = true,
                    issue = null,
                )
                loadMutex.withLock { onOpenConflictCopyLocked(copy) }
            } else {
                _editState.value = _editState.value.copy(
                    saving = false,
                    dirty = false,
                    copyPath = copy,
                    lastSaveOk = true,
                    issue = null,
                )
                loadMutex.withLock { onRefreshLocked() }
            }
            onNotice(appContext.getString(R.string.editor_copy_saved, copy))
            return true
        }
        return when (update.issue) {
            DocumentIssue.CONFLICT -> {
                _editState.value = _editState.value.copy(saving = false, issue = "conflict", lastSaveOk = false)
                false
            }
            DocumentIssue.MISSING -> {
                _editState.value = _editState.value.copy(saving = false, issue = "missing", lastSaveOk = false)
                false
            }
            DocumentIssue.SAVE_FAILED, DocumentIssue.RECOVERY -> {
                _editState.value = _editState.value.copy(saving = false, issue = "save_failed", lastSaveOk = false)
                false
            }
            DocumentIssue.NONE -> {
                val latest = _editState.value
                val changedDuringSave = latest.active && latest.sessionId == sessionId && latest.field.text != text
                _editState.value = latest.copy(
                    savedText = text,
                    dirty = changedDuringSave,
                    saving = false,
                    issue = null,
                    lastSaveOk = true,
                    lastSaveMicros = update.saveMicros.toLong(),
                )
                loadMutex.withLock { onRefreshLocked() }
                true
            }
        }
    }

    suspend fun handleExternalRefreshWhileEditing(): Boolean {
        val update = try {
            withContext(Dispatchers.IO) { documentRepository.refreshSession() }
        } catch (_: Exception) {
            null
        } ?: return false
        if (_editState.value.dirty) {
            when (update.issue) {
                DocumentIssue.CONFLICT -> _editState.value = _editState.value.copy(issue = "conflict")
                DocumentIssue.MISSING -> _editState.value = _editState.value.copy(issue = "missing")
                else -> Unit
            }
            return false
        }
        val text = update.text
        if (text != null) {
            withContext(Dispatchers.IO) { documentRepository.acceptExternal(update.revision.toLong()) }
            _editState.value = _editState.value.copy(
                field = TextFieldValue(text),
                savedText = text,
                dirty = false,
                issue = null,
            )
        }
        return true
    }

    suspend fun awaitSaveIdle() {
        activeSaveJob?.join()
    }
}
