// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.app

import android.content.Context
import io.github.groundgrounder.hortus.HortusApplication
import io.github.groundgrounder.hortus.core.AndroidFonts
import io.github.groundgrounder.hortus.core.CoreEnvironment
import io.github.groundgrounder.hortus.core.DiagramFocus
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.VaultHandle
import io.github.groundgrounder.hortus.core.WorkspaceCore
import io.github.groundgrounder.hortus.core.settings.AppSettingsRepository
import io.github.groundgrounder.hortus.core.settings.ShortcutStore
import io.github.groundgrounder.hortus.data.DiagramRenderService
import io.github.groundgrounder.hortus.data.DocumentRepository
import io.github.groundgrounder.hortus.data.EmbedRepository
import io.github.groundgrounder.hortus.data.ImageLoader
import io.github.groundgrounder.hortus.data.MathRenderService
import io.github.groundgrounder.hortus.data.VaultRepository
import io.github.groundgrounder.hortus.reader.render.DiagramExporter
import io.github.groundgrounder.hortus.reader.render.DiagramPreviewCache
import io.github.groundgrounder.hortus.reader.render.DiagramStateBus
import io.github.groundgrounder.hortus.reader.render.MathRenderResult
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File

/**
 * 进程级原生服务容器：统一管理全进程唯一 WorkspaceCore、DiagramRenderService、MathRenderService、
 * AppSettingsRepository、ShortcutStore 与缓存清理入口。
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
    private val resolverLock = Any()

    private var workspace: WorkspaceCore? = null
    private var activeVaultResolver: Pair<String, suspend (String, String) -> String?>? = null

    val fontPaths: List<String> by lazy {
        AndroidFonts.discover().mapNotNull { it.file?.path }.distinct()
    }

    val settingsRepository = AppSettingsRepository(appContext)
    val shortcutStore = ShortcutStore(appContext)
    val mathService = MathRenderService(appContext)
    val diagramService = DiagramRenderService(
        fontPaths = { fontPaths },
        withWorkspace = { block -> withWorkspace(block) },
        asyncScope = scope,
    )

    private fun ensureWorkspace(): WorkspaceCore =
        workspace ?: CoreEnvironment.createWorkspaceCore(appContext).also { workspace = it }

    suspend fun <T> withWorkspace(block: (WorkspaceCore) -> T): T = withContext(Dispatchers.IO) {
        workspaceMutex.withLock {
            block(ensureWorkspace())
        }
    }

    fun createVaultRepository(onMutated: () -> Unit): VaultRepository = VaultRepository(
        openVaultHandle = { path -> withWorkspace { it.openVault(path) } },
        registerActiveVault = ::registerActiveVault,
        unregisterActiveVault = ::unregisterActiveVault,
        onVaultReset = {
            diagramService.clearCache()
            DiagramPreviewCache.reset()
            DiagramStateBus.clear()
            onMutated()
        },
        onVaultMutated = onMutated,
    )

    fun createDocumentRepository(
        vaultRepository: VaultRepository,
        onMutated: () -> Unit,
    ): DocumentRepository = DocumentRepository(
        context = appContext,
        openDocumentSession = { root, path -> withWorkspace { it.openDocument(root, path) } },
        vaultKnowledgeMarkup = vaultRepository::knowledgeMarkup,
        onSavedContent = vaultRepository::upsertNoteContent,
        onContentMutated = onMutated,
    )

    fun createEmbedRepository(vaultRepository: VaultRepository): EmbedRepository =
        EmbedRepository(vaultRepository)

    suspend fun collapsedNodes(vaultPath: String, note: String, block: Int): List<String> =
        diagramService.collapsedNodes(vaultPath, note, block)

    suspend fun rememberCollapsed(
        vaultPath: String,
        note: String,
        block: Int,
        collapsed: List<String>,
    ) = diagramService.rememberCollapsed(vaultPath, note, block, collapsed)

    suspend fun diagramFocus(vaultPath: String, note: String, block: Int): DiagramFocus? =
        diagramService.diagramFocus(vaultPath, note, block)

    suspend fun rememberDiagramFocus(
        vaultPath: String,
        note: String,
        block: Int,
        scale: Double,
        offsetX: Double,
        offsetY: Double,
    ) = diagramService.rememberDiagramFocus(vaultPath, note, block, scale, offsetX, offsetY)

    fun persistDiagramFocusAsync(
        vaultPath: String,
        note: String,
        block: Int,
        scale: Double,
        offsetX: Double,
        offsetY: Double,
    ) = diagramService.persistDiagramFocusAsync(vaultPath, note, block, scale, offsetX, offsetY)

    suspend fun renderDiagram(
        documentKey: String,
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        revision: Long,
    ): SceneData = diagramService.renderDiagramRaw(
        documentKey = documentKey,
        engineText = engineText,
        mmd = mmd,
        ordinal = ordinal,
        width = width,
        dark = dark,
        collapsed = collapsed,
        revision = revision,
    )

    suspend fun renderSvg(
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
    ): String = diagramService.renderSvg(engineText, mmd, ordinal, width, dark, collapsed)

    suspend fun prepareMath(
        formulas: List<MathBlock>,
        bodySize: Float,
        notePath: String,
    ): List<MathRenderResult> = mathService.prepareMath(formulas, bodySize, notePath)

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
                    if (batches.isEmpty() || batches.any { it.scanComplete }) break
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

    /** M2b 清理缓存挂点：仅清理渲染缓存与临时导出/预览目录，不触碰配置与数据目录。 */
    suspend fun clearCaches() = withContext(Dispatchers.IO) {
        ImageLoader.clear()
        DiagramPreviewCache.reset()
        diagramService.clearCache()
        mathService.clearCache()
        runCatching { DiagramExporter.shareDirectory(appContext).deleteRecursively() }
        runCatching { File(appContext.cacheDir, "external-preview").deleteRecursively() }
        Unit
    }

    override fun close() {
        mathService.close()
        diagramService.close()
        try {
            workspace?.close()
        } catch (_: Exception) {
            Unit
        }
        workspace = null
        scope.cancel()
    }
}
