// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.SceneData
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min

@Composable
internal fun DiagramBlock(
    item: DiagramItem,
    engineText: String,
    contentWidth: Int,
    bodySizePx: Float,
    dark: Boolean,
    palette: ReaderPalette,
    repository: ReaderRepository,
    notePath: String,
    onFocus: (DiagramItem, SceneData, List<String>) -> Unit,
    modifier: Modifier = Modifier,
) {
    var collapsed by remember(item.documentKey, item.ordinal) { mutableStateOf<List<String>?>(null) }
    var result by remember(item.documentKey, item.ordinal) { mutableStateOf<DiagramRenderResult?>(null) }
    LaunchedEffect(item.documentKey, item.ordinal, DiagramStateBus.version) {
        collapsed = DiagramStateBus.nodes(notePath, item.ordinal)
            ?: withContext(Dispatchers.IO) {
                repository.collapsedNodes(notePath, item.ordinal)
            }
    }
    val density = LocalDensity.current.density
    val unitScale = diagramUnitScale(bodySizePx, density)
    val layoutWidth = (contentWidth / unitScale).toDouble()
    LaunchedEffect(
        item.documentKey,
        item.ordinal,
        engineText.hashCode(),
        contentWidth,
        bodySizePx,
        dark,
        collapsed,
        item.source,
    ) {
        val current = collapsed ?: return@LaunchedEffect
        result = withContext(Dispatchers.IO) {
            repository.renderDiagram(
                documentKey = item.documentKey,
                engineText = engineText,
                mmd = item.mmd,
                ordinal = item.ordinal,
                width = layoutWidth,
                dark = dark,
                collapsed = current,
                revision = engineText.hashCode().toLong(),
            )
        }
    }
    val scene = result?.scene
    val drawZoom = if (scene != null && scene.width > 0.0) {
        min(unitScale, contentWidth.toFloat() / scene.width.toFloat())
    } else {
        unitScale
    }
    LaunchedEffect(result) {
        result?.failure?.takeIf { it.isNotBlank() }?.let { reason ->
            Inspection.recordDiagramFailure("${item.documentKey}#${item.ordinal}", item.ordinal, reason)
        }
    }
    DisposableEffect(item.documentKey, item.ordinal) {
        val count = Inspection.composedDiagramCanvases.incrementAndGet()
        if (count > Inspection.maxComposedDiagramCanvases.get()) {
            Inspection.maxComposedDiagramCanvases.set(count)
        }
        onDispose {
            Inspection.composedDiagramCanvases.decrementAndGet()
        }
    }
    val canvasHeight = if (scene != null) {
        max(1, ceil(scene.height.toFloat() * drawZoom).toInt())
    } else {
        (120 * density).toInt()
    }
    Box(modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        if (scene != null) {
            val key = buildString {
                append(item.documentKey).append('#').append(item.ordinal)
                append('@').append(contentWidth).append('/').append(dark)
                append('/').append(palette.textArgb)
                append('/').append(unitScale).append('/').append(drawZoom)
                append('/').append(collapsed.orEmpty().joinToString(","))
                append('/').append(engineText.hashCode())
            }
            val renderNode = remember(scene, key) {
                DiagramPreviewCache.renderNode(
                    key,
                    scene,
                    contentWidth,
                    canvasHeight,
                    drawZoom,
                    palette.textArgb,
                )
            }
            LaunchedEffect(scene, key) {
                if (!Inspection.recordingEnabled) return@LaunchedEffect
                Inspection.recordInlinePreview(
                    "diagram:${item.documentKey}:${item.ordinal}",
                    scene.hitRegions.size,
                    scene.primitives.count {
                        it is io.github.groundgrounder.hortus.core.ScenePrimitive.Text &&
                            it.span.text.trim().startsWith("+")
                    },
                    collapsed.orEmpty(),
                    key,
                    canvasHeight.toFloat(),
                    layoutWidth,
                    unitScale,
                    drawZoom,
                    diagramMainFontSize(scene) * drawZoom,
                )
            }
            Canvas(
                Modifier
                    .fillMaxWidth()
                    .height(with(LocalDensity.current) { canvasHeight.toDp() })
                    .onGloballyPositioned { coordinates ->
                        val origin = coordinates.positionInWindow()
                        Inspection.recordDiagramRect(
                            "diagram:${item.documentKey}:${item.ordinal}",
                            origin.x,
                            origin.y,
                            coordinates.size.width.toFloat(),
                            coordinates.size.height.toFloat(),
                        )
                    }
                    .pointerInput(scene, collapsed) {
                        detectTapGestures {
                            onFocus(item, scene, collapsed.orEmpty())
                        }
                    },
            ) {
                drawIntoCanvas { canvas ->
                    canvas.nativeCanvas.drawRenderNode(renderNode)
                    DiagramPreviewCache.noteReplay()
                }
            }
        } else {
            val failure = result?.failure
            Column(
                Modifier
                    .fillMaxWidth()
                    .background(palette.codeBackground, RoundedCornerShape(8.dp))
                    .padding(12.dp),
            ) {
                Text(
                    stringResource(R.string.diagram_source),
                    style = MaterialTheme.typography.labelMedium,
                    color = palette.muted,
                )
                Text(
                    engineText.substring(
                        item.sourceStart.coerceIn(0, engineText.length),
                        (item.sourceStart + item.sourceLength).coerceIn(0, engineText.length),
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = palette.text,
                )
                if (failure != null) {
                    Text(
                        failure,
                        style = MaterialTheme.typography.labelSmall,
                        color = palette.error,
                    )
                }
            }
        }
    }
}
