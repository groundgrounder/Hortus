// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.app

import android.content.Context
import io.github.groundgrounder.hortus.AndroidFonts
import io.github.groundgrounder.hortus.CoreEnvironment
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.HortusApplication
import io.github.groundgrounder.hortus.core.DiagramEngine
import io.github.groundgrounder.hortus.core.DiagramFocus
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MathEngine
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.VaultHandle
import io.github.groundgrounder.hortus.core.WorkspaceCore
import io.github.groundgrounder.hortus.reader.DiagramPreviewCache
import io.github.groundgrounder.hortus.reader.ImageLoader
import io.github.groundgrounder.hortus.reader.MathRenderResult
import io.github.groundgrounder.hortus.reader.MathSceneRenderer
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicLong

/**
 * 进程级原生服务容器：全进程唯一 WorkspaceCore、DiagramEngine、MathSceneRenderer 与缓存裁剪入口。
 * 消除多 Store 并发写 session.toml 竞态，并避免全屏图页重复创建引擎与 Vault 监听线程。
 */
internal class HortusServices(context: Context) : AutoCloseable {
    companion object {
        @Volatile
        private var fallback: HortusServices? = null

        fun get(context: Context): HortusServices {
            val app = context.applicationContext
            if (app is HortusApplication) return app.services
            return fallback ?: synchronized(this) {
                fallback ?: HortusServices(app).also { fallback = it }
            }
        }
    }

    private val appContext = context.applicationContext
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val workspaceMutex = Mutex()
    private val diagramMutex = Mutex()
    private val mathMutex = Mutex()
    private val resolverLock = Any()

    private var workspace: WorkspaceCore? = null
    private var diagramEngine: DiagramEngine? = null
    private var mathRenderer: MathSceneRenderer? = null
    private var activeVaultResolver: Pair<String, suspend (String, String) -> String?>? = null
    private val requestGeneration = AtomicLong()

    val fontPaths: List<String> by lazy {
        AndroidFonts.discover().mapNotNull { it.file?.path }.distinct()
    }

    private fun ensureWorkspace(): WorkspaceCore =
        workspace ?: CoreEnvironment.createWorkspaceCore(appContext).also { workspace = it }

    private fun ensureDiagramEngine(): DiagramEngine {
        diagramEngine?.let { return it }
        val engine = DebugProbes.traceMainThread("diagramRender") {
            DiagramEngine.withFontPaths(fontPaths)
        }
        diagramEngine = engine
        return engine
    }

    private fun ensureMathRenderer(): MathSceneRenderer {
        mathRenderer?.let { return it }
        val engine = DebugProbes.traceMainThread("mathRender") {
            MathEngine()
        }
        val renderer = MathSceneRenderer(appContext.assets, engine)
        mathRenderer = renderer
        return renderer
    }

    suspend fun <T> withWorkspace(block: (WorkspaceCore) -> T): T = withContext(Dispatchers.IO) {
        workspaceMutex.withLock {
            block(ensureWorkspace())
        }
    }

    suspend fun collapsedNodes(vaultPath: String, note: String, block: Int): List<String> =
        withWorkspace { core ->
            try {
                core.collapsedNodes(vaultPath, note, block.toUInt())
            } catch (_: Exception) {
                emptyList()
            }
        }

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
        }

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
        scope.launch {
            rememberDiagramFocus(vaultPath, note, block, scale, offsetX, offsetY)
        }
    }

    suspend fun renderDiagram(
        documentKey: String,
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        revision: Long,
    ): SceneData = withContext(Dispatchers.IO) {
        diagramMutex.withLock {
            val engine = ensureDiagramEngine()
            val request = requestGeneration.incrementAndGet()
            val identity = io.github.groundgrounder.hortus.core.DiagramIdentity(
                document = documentKey,
                block = ordinal.toUInt(),
                revision = revision.toULong(),
                dark = dark,
                fontGeneration = fontPaths.size.toULong(),
                requestGeneration = request.toULong(),
            )
            DebugProbes.traceMainThread("diagramRender") {
                engine.renderRequest(
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

    suspend fun renderSvg(
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
    ): String = withContext(Dispatchers.IO) {
        diagramMutex.withLock {
            val engine = ensureDiagramEngine()
            DebugProbes.traceMainThread("diagramRender") {
                engine.renderSvg(engineText, mmd, ordinal.toUInt(), width, dark, collapsed)
            }
        }
    }

    suspend fun prepareMath(
        formulas: List<MathBlock>,
        bodySize: Float,
        notePath: String,
    ): List<MathRenderResult> = withContext(Dispatchers.IO) {
        if (formulas.isEmpty()) return@withContext emptyList()
        mathMutex.withLock {
            val renderer = ensureMathRenderer()
            val em = (bodySize * io.github.groundgrounder.hortus.core.mathTypographyScale()).toFloat()
            formulas.map { formula ->
                val result = if (formula.latex.isBlank()) {
                    MathRenderResult(null, null, 0)
                } else {
                    renderer.render(formula.latex, formula.style, em)
                }
                DebugProbes.recordMath(notePath, formula, result, bodySize)
                result
            }
        }
    }

    fun registerActiveVault(vaultPath: String, resolver: suspend (String, String) -> String?) {
        synchronized(resolverLock) {
            activeVaultResolver = vaultPath to resolver
        }
    }

    fun unregisterActiveVault(vaultPath: String) {
        synchronized(resolverLock) {
            if (activeVaultResolver?.first == vaultPath) {
                activeVaultResolver = null
            }
        }
    }

    /**
     * 优先复用活跃库会话解析双链；若无匹配活跃库则用临时 VaultHandle 解析并在局部作用域内停止监听并关闭。
     */
    suspend fun resolveVaultLink(vaultPath: String, sourcePath: String, rawTarget: String): String? {
        if (vaultPath.isBlank()) return null
        val active = synchronized(resolverLock) {
            activeVaultResolver?.takeIf { it.first == vaultPath }?.second
        }
        if (active != null) {
            return active(sourcePath, rawTarget)
        }
        return withContext(Dispatchers.IO) {
            var handle: VaultHandle? = null
            try {
                handle = withWorkspace { it.openVault(vaultPath) }
                val deadline = android.os.SystemClock.uptimeMillis() + 10_000L
                while (android.os.SystemClock.uptimeMillis() < deadline) {
                    val batches = handle.waitForBatches()
                    if (batches.any { it.scanComplete }) break
                }
                handle.resolveLink(sourcePath, rawTarget).resolvedPath
            } catch (_: Exception) {
                null
            } finally {
                try {
                    handle?.stopWatching()
                } catch (_: Exception) {
                    Unit
                }
                try {
                    handle?.close()
                } catch (_: Exception) {
                    Unit
                }
            }
        }
    }

    fun trimMemory(level: Int) {
        ImageLoader.trimMemory(level)
        DiagramPreviewCache.trimMemory(level)
    }

    override fun close() {
        try {
            mathRenderer?.close()
        } catch (_: Exception) {
            Unit
        }
        mathRenderer = null
        try {
            diagramEngine?.close()
        } catch (_: Exception) {
            Unit
        }
        diagramEngine = null
        try {
            workspace?.close()
        } catch (_: Exception) {
            Unit
        }
        workspace = null
        scope.cancel()
    }
}
