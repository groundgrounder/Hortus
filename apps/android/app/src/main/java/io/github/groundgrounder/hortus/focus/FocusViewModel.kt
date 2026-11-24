// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.focus

import android.app.Application
import android.util.TypedValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.app.HortusServices
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.reader.render.DiagramExportFormat
import io.github.groundgrounder.hortus.reader.render.DiagramExporter
import io.github.groundgrounder.hortus.reader.render.DiagramStateBus
import io.github.groundgrounder.hortus.reader.render.diagramUnitScale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

internal data class FocusUiState(
    val scene: SceneData? = null,
    val failure: String? = null,
    val collapsed: List<String> = emptyList(),
)

/**
 * 全屏图表页 ViewModel：复用进程级 HortusServices 共享 DiagramEngine 与 WorkspaceCore，
 * 避免独立新建引擎或句柄泄漏，并在退出/暂停时持久化视口状态。
 */
internal class FocusViewModel(application: Application) : AndroidViewModel(application) {
    private val services = HortusServices.get(application.applicationContext)
    private val settings = services.settingsRepository

    private var initialized = false
    private var payload: FocusPayload? = null

    @Volatile var viewportScale: Float = 0f
        private set
    @Volatile var viewportOffsetX: Float = 0f
        private set
    @Volatile var viewportOffsetY: Float = 0f
        private set

    private val _uiState = MutableStateFlow(FocusUiState())
    val uiState: StateFlow<FocusUiState> = _uiState.asStateFlow()

    val unitScale: Float
        get() {
            val metrics = getApplication<Application>().resources.displayMetrics
            val bodySizePx = TypedValue.applyDimension(
                TypedValue.COMPLEX_UNIT_SP,
                settings.settings.value.fontSizeSp,
                metrics,
            )
            return diagramUnitScale(bodySizePx, metrics.density)
        }

    fun initialize(payload: FocusPayload, dark: Boolean) {
        if (initialized) return
        initialized = true
        this.payload = payload
        _uiState.value = FocusUiState(collapsed = payload.collapsed)
        loadScene(payload, dark, payload.collapsed)
    }

    fun updateViewport(scale: Float, offsetX: Float, offsetY: Float) {
        viewportScale = scale
        viewportOffsetX = offsetX
        viewportOffsetY = offsetY
    }

    fun toggleNode(nodeId: String, dark: Boolean) {
        val currentPayload = payload ?: return
        val prev = _uiState.value.collapsed
        val next = if (nodeId in prev) prev - nodeId else prev + nodeId
        _uiState.value = _uiState.value.copy(collapsed = next)
        DiagramStateBus.publish(currentPayload.vaultPath, currentPayload.notePath, currentPayload.ordinal, next)
        DebugProbes.recordFocusCollapsed(next)
        DebugProbes.writeFocusDebug(getApplication())
        viewModelScope.launch {
            services.rememberCollapsed(currentPayload.vaultPath, currentPayload.notePath, currentPayload.ordinal, next)
        }
        loadScene(currentPayload, dark, next)
    }

    fun onDarkModeChanged(dark: Boolean) {
        val currentPayload = payload ?: return
        loadScene(currentPayload, dark, _uiState.value.collapsed)
    }

    private fun loadScene(payload: FocusPayload, dark: Boolean, collapsed: List<String>) {
        viewModelScope.launch {
            try {
                val rendered = services.renderDiagram(
                    documentKey = payload.documentKey,
                    engineText = payload.engineText,
                    mmd = payload.mmd,
                    ordinal = payload.ordinal,
                    width = payload.renderWidth,
                    dark = dark,
                    collapsed = collapsed,
                    revision = payload.revision,
                )
                _uiState.value = _uiState.value.copy(scene = rendered, failure = null)
                if (DebugProbes.enabled) {
                    withContext(Dispatchers.IO) {
                        DebugProbes.recordFocusScene(rendered, payload.documentKey, payload.ordinal)
                    }
                }
            } catch (error: Exception) {
                _uiState.value = _uiState.value.copy(
                    scene = null,
                    failure = error.message ?: error.javaClass.simpleName,
                )
            }
        }
    }

    suspend fun loadStoredFocus() = payload?.let {
        services.diagramFocus(it.vaultPath, it.notePath, it.ordinal)
    }

    suspend fun resolveVaultLink(target: String): String? {
        val currentPayload = payload ?: return null
        return services.resolveVaultLink(currentPayload.vaultPath, currentPayload.notePath, target)
    }

    suspend fun encode(format: DiagramExportFormat, dark: Boolean): ByteArray {
        val currentPayload = payload ?: throw IllegalStateException("diagram unavailable")
        val value = _uiState.value.scene ?: throw IllegalStateException("diagram unavailable")
        val collapsed = _uiState.value.collapsed
        return when (format) {
            DiagramExportFormat.Svg -> services.renderSvg(
                engineText = currentPayload.engineText,
                mmd = currentPayload.mmd,
                ordinal = currentPayload.ordinal,
                width = currentPayload.renderWidth,
                dark = dark,
                collapsed = collapsed,
            ).toByteArray(Charsets.UTF_8)
            DiagramExportFormat.Png -> DiagramExporter.encodePng(value, dark)
            DiagramExportFormat.Pdf -> DiagramExporter.encodePdf(value, dark)
        }
    }

    fun persistViewport() {
        val value = payload ?: return
        val scale = viewportScale
        if (scale <= 0f) return
        services.persistDiagramFocusAsync(
            vaultPath = value.vaultPath,
            note = value.notePath,
            block = value.ordinal,
            scale = scale.toDouble(),
            offsetX = viewportOffsetX.toDouble(),
            offsetY = viewportOffsetY.toDouble(),
        )
    }

    override fun onCleared() {
        persistViewport()
        super.onCleared()
    }
}
