// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.data

import android.content.Context
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.core.DocumentIssue
import io.github.groundgrounder.hortus.core.DocumentSession
import io.github.groundgrounder.hortus.core.DocumentUpdate
import io.github.groundgrounder.hortus.core.KnowledgeMarkupInfo
import io.github.groundgrounder.hortus.core.ReadingModel
import io.github.groundgrounder.hortus.core.SaveChoice
import io.github.groundgrounder.hortus.core.diagramBlocks
import io.github.groundgrounder.hortus.core.model.NoteContent
import io.github.groundgrounder.hortus.core.model.OutlineRow
import io.github.groundgrounder.hortus.core.model.ReadingModelDecoder
import io.github.groundgrounder.hortus.core.model.buildReaderItems
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File

/**
 * 统一串行持有 DocumentSession，负责开关文档、按宽度重排、增量更新、落盘与只读预览。
 */
internal class DocumentRepository(
    context: Context,
    private val openDocumentSession: suspend (String, String) -> DocumentSession,
    private val vaultKnowledgeMarkup: suspend (String, String) -> KnowledgeMarkupInfo,
    private val onSavedContent: suspend (String, String) -> Unit,
    private val onContentMutated: () -> Unit,
) : AutoCloseable {
    private val appContext = context.applicationContext
    private val sessionMutex = Mutex()

    private var document: DocumentSession? = null
    private var vaultPath: String? = null
    private var notePath: String? = null

    val currentPath: String? get() = notePath

    suspend fun openNote(
        vaultRoot: String,
        path: String,
        width: Int,
        bodySize: Float,
    ): NoteContent = withContext(Dispatchers.IO) {
        sessionMutex.withLock {
            closeDocumentLocked()
            val session = openDocumentSession(vaultRoot, path)
            document = session
            vaultPath = vaultRoot
            notePath = path
            buildContent(session, vaultRoot, path, width, bodySize)
        }
    }

    private suspend fun buildContent(
        session: DocumentSession,
        root: String,
        path: String,
        width: Int,
        bodySize: Float,
    ): NoteContent {
        val initial = session.initial()
        val wire: ReadingModel = DebugProbes.traceMainThread("readingModel") {
            session.readingModel()
        }
        val model = ReadingModelDecoder.decodeFull(wire)
        val snapshot = DebugProbes.traceMainThread("readingModel") { session.snapshot() }
        val math = DebugProbes.traceMainThread("readingModel") { session.mathBlocks() }
        val media = DebugProbes.traceMainThread("mediaBlocks") {
            session.mediaBlocks(width.toDouble(), bodySize.toDouble())
        }
        val knowledge = DebugProbes.traceMainThread("readingModel") {
            vaultKnowledgeMarkup(path, snapshot.text)
        }
        val outline = DebugProbes.traceMainThread("readingModel") { session.outline() }
        val diagrams = DebugProbes.traceMainThread("readingModel") { diagramBlocks(snapshot.text) }
        val isMmd = path.lowercase().endsWith(".mmd")
        val items = buildReaderItems(
            model = model,
            sourceText = snapshot.text,
            media = media,
            math = math,
            knowledge = knowledge,
            diagramBlocks = diagrams,
            documentKey = path,
            documentIsMmd = isMmd,
        )
        return NoteContent(
            vaultPath = root,
            path = path,
            revision = model.revision,
            model = model,
            items = items,
            media = media,
            math = math,
            knowledge = knowledge,
            sourceText = snapshot.text,
            outline = outline,
            readOnlyReason = initial.readOnlyReason,
            diagramBlocks = diagrams,
            width = width,
            bodySize = bodySize,
            isMmd = isMmd,
        )
    }

    suspend fun rebuildForWidth(content: NoteContent, width: Int, bodySize: Float): NoteContent =
        withContext(Dispatchers.IO) {
            sessionMutex.withLock {
                if (content.width == width && content.bodySize == bodySize) return@withLock content
                val media = DebugProbes.traceMainThread("mediaBlocks") {
                    document?.mediaBlocks(width.toDouble(), bodySize.toDouble()) ?: content.media
                }
                val items = buildReaderItems(
                    model = content.model,
                    sourceText = content.sourceText,
                    media = media,
                    math = content.math,
                    knowledge = content.knowledge,
                    diagramBlocks = content.diagramBlocks,
                    documentKey = content.path,
                    documentIsMmd = content.isMmd,
                )
                content.copy(
                    items = items,
                    media = media,
                    width = width,
                    bodySize = bodySize,
                )
            }
        }

    suspend fun refresh(content: NoteContent, width: Int, bodySize: Float): NoteContent? =
        withContext(Dispatchers.IO) {
            sessionMutex.withLock {
                val session = document ?: return@withLock null
                if (notePath != content.path) return@withLock null
                val update = DebugProbes.traceMainThread("readingModel") { session.refresh() }
                val revised = update.revision.toLong()
                if (update.text != null) {
                    DebugProbes.traceMainThread("readingModel") { session.acceptExternal(update.revision) }
                }
                if (revised == content.revision && update.text == null) return@withLock null
                var wire = DebugProbes.traceMainThread("readingModel") {
                    session.readingModelSince(content.revision.toULong())
                }
                val model = try {
                    when {
                        ReadingModelDecoder.isFull(wire) -> ReadingModelDecoder.decodeFull(wire)
                        ReadingModelDecoder.isEmptyDelta(wire) ->
                            content.model.copy(revision = wire.revision.toLong(), rebuilt = wire.rebuilt.toLong())
                        else -> ReadingModelDecoder.applyDelta(content.model, wire)
                    }
                } catch (_: Exception) {
                    wire = DebugProbes.traceMainThread("readingModel") { session.readingModel() }
                    ReadingModelDecoder.decodeFull(wire)
                }
                val snapshot = DebugProbes.traceMainThread("readingModel") { session.snapshot() }
                val math = DebugProbes.traceMainThread("readingModel") { session.mathBlocks() }
                val media = DebugProbes.traceMainThread("mediaBlocks") {
                    session.mediaBlocks(width.toDouble(), bodySize.toDouble())
                }
                val knowledge = DebugProbes.traceMainThread("readingModel") {
                    vaultKnowledgeMarkup(content.path, snapshot.text)
                }
                val outline = DebugProbes.traceMainThread("readingModel") { session.outline() }
                val diagrams = DebugProbes.traceMainThread("readingModel") { diagramBlocks(snapshot.text) }
                val items = buildReaderItems(
                    model = model,
                    sourceText = snapshot.text,
                    media = media,
                    math = math,
                    knowledge = knowledge,
                    diagramBlocks = diagrams,
                    documentKey = content.path,
                    documentIsMmd = content.isMmd,
                )
                content.copy(
                    revision = model.revision,
                    model = model,
                    items = items,
                    media = media,
                    math = math,
                    knowledge = knowledge,
                    sourceText = snapshot.text,
                    outline = outline,
                    diagramBlocks = diagrams,
                    width = width,
                    bodySize = bodySize,
                )
            }
        }

    suspend fun outlineRows(): List<OutlineRow> = withContext(Dispatchers.IO) {
        sessionMutex.withLock {
            document?.outline()?.headings?.map {
                OutlineRow(it.level.toInt(), it.title, it.start.toInt())
            } ?: emptyList()
        }
    }

    suspend fun sessionText(): String? = withContext(Dispatchers.IO) {
        sessionMutex.withLock {
            document?.snapshot()?.text
        }
    }

    suspend fun replaceAndSave(
        text: String,
        temporary: String,
        choice: SaveChoice,
    ): DocumentUpdate? = withContext(Dispatchers.IO) {
        sessionMutex.withLock {
            val session = document ?: return@withLock null
            val current = session.snapshot().text
            if (current != text) {
                session.replace(0uL, current.length.toULong(), text)
            }
            val update = session.save(temporary, choice)
            if (update.issue == DocumentIssue.NONE && !update.dirty && update.copyPath == null) {
                notePath?.let { onSavedContent(it, text) }
                onContentMutated()
            } else if (update.copyPath != null) {
                onContentMutated()
            }
            update
        }
    }

    suspend fun refreshSession(): DocumentUpdate? = withContext(Dispatchers.IO) {
        sessionMutex.withLock {
            document?.refresh()
        }
    }

    suspend fun acceptExternal(revision: Long) = withContext(Dispatchers.IO) {
        sessionMutex.withLock {
            document?.acceptExternal(revision.toULong())
            Unit
        }
    }

    suspend fun useDiskVersion(): DocumentUpdate? = withContext(Dispatchers.IO) {
        sessionMutex.withLock {
            val session = document ?: return@withLock null
            val update = session.useDisk()
            if (update.text != null) {
                session.acceptExternal(update.revision)
                onContentMutated()
            }
            update
        }
    }

    fun cleanupExternalPreview() {
        runCatching {
            File(appContext.cacheDir, "external-preview").deleteRecursively()
        }
    }

    suspend fun openExternalPreview(
        fileName: String,
        text: String,
        width: Int,
        bodySize: Float,
        activeVaultPath: String?,
    ): NoteContent = withContext(Dispatchers.IO) {
        sessionMutex.withLock {
            closeDocumentLocked()
            val directory = File(appContext.cacheDir, "external-preview").apply {
                if (exists()) deleteRecursively()
                mkdirs()
            }
            val base = fileName.substringAfterLast('/').ifBlank { "preview.md" }
            val cleaned = base.replace(Regex("[^\\p{L}\\p{N}._ -]"), "_").take(100)
                .ifBlank { "preview.md" }
            val safe = if (cleaned.lowercase().endsWith(".md") ||
                cleaned.lowercase().endsWith(".mmd")
            ) {
                cleaned
            } else {
                "$cleaned.md"
            }
            File(directory, safe).writeText(text)
            val session = openDocumentSession(directory.absolutePath, safe)
            document = session
            notePath = null
            buildContent(session, directory.absolutePath, safe, width, bodySize).copy(
                readOnlyReason = "preview.external",
                externalPreview = true,
                vaultPath = activeVaultPath ?: directory.absolutePath,
            )
        }
    }

    suspend fun closeDocument() = withContext(Dispatchers.IO) {
        sessionMutex.withLock { closeDocumentLocked() }
    }

    private fun closeDocumentLocked() {
        try {
            document?.close()
        } catch (_: Exception) {
            Unit
        }
        document = null
        notePath = null
    }

    override fun close() {
        runBlocking {
            withTimeoutOrNull(3_000L) {
                sessionMutex.withLock {
                    closeDocumentLocked()
                }
            } ?: closeDocumentLocked()
        }
        cleanupExternalPreview()
    }
}
