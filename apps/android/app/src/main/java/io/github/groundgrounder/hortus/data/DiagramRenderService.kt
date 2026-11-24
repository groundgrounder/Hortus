// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.data

import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.core.DiagramEngine
import io.github.groundgrounder.hortus.core.DiagramFailure
import io.github.groundgrounder.hortus.core.DiagramFocus
import io.github.groundgrounder.hortus.core.DiagramIdentity
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.WorkspaceCore
import io.github.groundgrounder.hortus.core.model.DiagramRenderResult
import io.github.groundgrounder.hortus.reader.render.SceneTextMetrics
import io.github.groundgrounder.hortus.reader.render.previewDrawZoom
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicLong

/**
 * 图表布局渲染、缓存（含 vaultPath 隔离）、IO 文本度量预热、SVG 导出与折叠/专注状态读写服务。
 */
internal class DiagramRenderService(
    private val fontPaths: () -> List<String>,
    private val withWorkspace: suspend ((WorkspaceCore) -> Any?) -> Any?,
    private val asyncScope: CoroutineScope,
) : AutoCloseable {
    private val diagramMutex = Mutex()
    private val requestGeneration = AtomicLong()
    private var engine: DiagramEngine? = null

    private val diagramCache = object : LinkedHashMap<String, DiagramRenderResult>(48, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, DiagramRenderResult>): Boolean =
            size > 24
    }

    private fun ensureEngine(): DiagramEngine {
        engine?.let { return it }
        val created = DebugProbes.traceMainThread("diagramRender") {
            DiagramEngine.withFontPaths(fontPaths())
        }
        engine = created
        return created
    }

    suspend fun renderDiagramRaw(
        documentKey: String,
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        revision: Long,
    ): SceneData = withContext(Dispatchers.IO) {
        val paths = fontPaths()
        diagramMutex.withLock {
            val active = ensureEngine()
            val request = requestGeneration.incrementAndGet()
            val identity = DiagramIdentity(
                document = documentKey,
                block = ordinal.toUInt(),
                revision = revision.toULong(),
                dark = dark,
                fontGeneration = paths.size.toULong(),
                requestGeneration = request.toULong(),
            )
            DebugProbes.traceMainThread("diagramRender") {
                active.renderRequest(
                    source = engineText,
                    mmd = mmd,
                    width = width,
                    collapsed = collapsed,
                    identity = identity,
                    previous = null,
                    viewport = emptyList(),
                )
            }
        }
    }

    suspend fun renderDiagram(
        vaultPath: String,
        documentKey: String,
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        revision: Long,
        previewContentWidthPx: Int = 0,
        previewUnitScale: Float = 0f,
    ): DiagramRenderResult = withContext(Dispatchers.IO) {
        val key = "$vaultPath|${engineText.hashCode()}:${engineText.length}:$mmd:$ordinal:${width.toInt()}:$dark:${collapsed.sorted()}:$documentKey:$revision"
        val cached = synchronized(diagramCache) { diagramCache[key] }
        if (cached != null) {
            prewarmIfRequested(cached.scene, previewContentWidthPx, previewUnitScale)
            return@withContext cached
        }
        try {
            val scene = renderDiagramRaw(
                documentKey = documentKey,
                engineText = engineText,
                mmd = mmd,
                ordinal = ordinal,
                width = width,
                dark = dark,
                collapsed = collapsed,
                revision = revision,
            )
            prewarmIfRequested(scene, previewContentWidthPx, previewUnitScale)
            DebugProbes.recordDiagram(documentKey, ordinal, width, dark, collapsed, scene)
            val result = DiagramRenderResult(scene, null, scene.identity, scene.width, scene.height)
            synchronized(diagramCache) { diagramCache[key] = result }
            result
        } catch (failure: DiagramFailure) {
            val reason = diagramFailureText(failure)
            DebugProbes.recordDiagramFailure("${documentKey}#${ordinal}", ordinal, reason)
            DiagramRenderResult(null, reason, null, 0.0, 0.0)
        } catch (error: Exception) {
            val reason = error.message ?: error.javaClass.simpleName
            DebugProbes.recordDiagramFailure("${documentKey}#${ordinal}", ordinal, reason)
            DiagramRenderResult(null, reason, null, 0.0, 0.0)
        }
    }

    private fun prewarmIfRequested(
        scene: SceneData?,
        previewContentWidthPx: Int,
        previewUnitScale: Float,
    ) {
        if (scene == null || previewContentWidthPx <= 0 || previewUnitScale <= 0f) return
        val drawZoom = previewDrawZoom(scene.width, previewContentWidthPx, previewUnitScale)
        SceneTextMetrics.prewarmScene(scene, drawZoom)
    }

    suspend fun renderSvg(
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
    ): String = withContext(Dispatchers.IO) {
        diagramMutex.withLock {
            val active = ensureEngine()
            DebugProbes.traceMainThread("diagramRender") {
                active.renderSvg(engineText, mmd, ordinal.toUInt(), width, dark, collapsed)
            }
        }
    }

    @Suppress("UNCHECKED_CAST")
    suspend fun collapsedNodes(vaultPath: String, note: String, block: Int): List<String> =
        withWorkspace { core ->
            try {
                core.collapsedNodes(vaultPath, note, block.toUInt())
            } catch (_: Exception) {
                emptyList<String>()
            }
        } as List<String>

    suspend fun rememberCollapsed(
        vaultPath: String,
        note: String,
        block: Int,
        collapsed: List<String>,
    ) {
        withWorkspace { core ->
            try {
                core.rememberDiagram(vaultPath, note, block.toUInt(), collapsed)
            } catch (_: Exception) {
                Unit
            }
        }
    }

    suspend fun diagramFocus(vaultPath: String, note: String, block: Int): DiagramFocus? =
        withWorkspace { core ->
            try {
                core.diagramFocus(vaultPath, note, block.toUInt())
            } catch (_: Exception) {
                null
            }
        } as DiagramFocus?

    suspend fun rememberDiagramFocus(
        vaultPath: String,
        note: String,
        block: Int,
        scale: Double,
        offsetX: Double,
        offsetY: Double,
    ) {
        withWorkspace { core ->
            try {
                core.rememberDiagramFocus(
                    vaultPath,
                    note,
                    block.toUInt(),
                    scale,
                    offsetX,
                    offsetY,
                )
            } catch (_: Exception) {
                Unit
            }
        }
    }

    fun persistDiagramFocusAsync(
        vaultPath: String,
        note: String,
        block: Int,
        scale: Double,
        offsetX: Double,
        offsetY: Double,
    ) {
        asyncScope.launch {
            rememberDiagramFocus(vaultPath, note, block, scale, offsetX, offsetY)
        }
    }

    fun clearCache() {
        synchronized(diagramCache) { diagramCache.clear() }
    }

    private fun diagramFailureText(failure: DiagramFailure): String = when (failure) {
        is DiagramFailure.NotATree -> "diagram.not_a_tree: ${failure.multiParent.joinToString()}"
        is DiagramFailure.Unsupported -> "diagram.unsupported: ${failure.diagramType}"
        is DiagramFailure.UnsupportedConstruct -> "diagram.unsupported: ${failure.kind} (line ${failure.line})"
        is DiagramFailure.Syntax -> "diagram.syntax: ${failure.kind} (line ${failure.line})"
        is DiagramFailure.Engine -> "diagram.engine: ${failure.kind}"
        else -> failure.toString()
    }

    override fun close() {
        clearCache()
        try {
            engine?.close()
        } catch (_: Exception) {
            Unit
        }
        engine = null
    }
}
