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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.SceneData
import kotlin.math.max

@Composable
internal fun DiagramBlock(
    item: DiagramItem,
    engineText: String,
    contentWidth: Int,
    dark: Boolean,
    palette: ReaderPalette,
    repository: ReaderRepository,
    notePath: String,
    onOpenLink: (String, String?) -> Unit,
    onFocus: (DiagramItem, SceneData, List<String>) -> Unit,
    onExport: (DiagramExportFormat, DiagramItem, SceneData?, List<String>) -> Unit,
    modifier: Modifier = Modifier,
) {
    val scope = rememberCoroutineScope()
    var collapsed by remember(item.documentKey, item.ordinal) { mutableStateOf<List<String>?>(null) }
    var result by remember(item.documentKey, item.ordinal) { mutableStateOf<DiagramRenderResult?>(null) }
    var menuOpen by remember { mutableStateOf(false) }
    LaunchedEffect(item.documentKey, item.ordinal) {
        collapsed = withContext(Dispatchers.IO) {
            repository.collapsedNodes(notePath, item.ordinal)
        }
    }
    LaunchedEffect(
        item.documentKey,
        item.ordinal,
        engineText.hashCode(),
        contentWidth,
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
            width = contentWidth.toDouble(),
            dark = dark,
            collapsed = current,
            revision = engineText.hashCode().toLong(),
            )
        }
    }
    val scene = result?.scene
    val density = LocalDensity.current.density
    val scale = if (scene != null && scene.width > 0.0) {
        contentWidth.toFloat() / scene.width.toFloat()
    } else {
        1f
    }
    var origin by remember(item.documentKey, item.ordinal) { mutableStateOf(Offset.Zero) }
    LaunchedEffect(scene, origin, scale, collapsed) {
        val value = scene ?: return@LaunchedEffect
        HitRegistry.replace(
            "diagram:${item.documentKey}:${item.ordinal}",
            value.hitRegions.map { hit ->
                ScreenHit(
                    kind = "node",
                    label = hit.label,
                    x = origin.x + hit.rect.x.toFloat() * scale,
                    y = origin.y + hit.rect.y.toFloat() * scale,
                    width = hit.rect.width.toFloat() * scale,
                    height = hit.rect.height.toFloat() * scale,
                    target = hit.linkTarget,
                    heading = hit.linkHeading,
                    canCollapse = hit.canCollapse,
                    nodeId = hit.nodeId,
                )
            },
        )
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
            HitRegistry.replace("diagram:${item.documentKey}:${item.ordinal}", emptyList())
        }
    }
    val canvasHeight = if (scene != null) {
        max(1, (scene.height * scale).toInt())
    } else {
        (120 * density).toInt()
    }
    Box(modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        if (scene != null) {
            Canvas(
                Modifier
                    .fillMaxWidth()
                    .height(with(LocalDensity.current) { canvasHeight.toDp() })
                    .onGloballyPositioned { origin = it.positionInWindow() }
                    .pointerInput(scene, scale, collapsed) {
                        detectTapGestures { point ->
                            val hit = SceneRenderer.hitAt(
                                scene,
                                point.x / scale,
                                point.y / scale,
                            )
                            if (hit != null) {
                                if (hit.canCollapse) {
                                    val current = collapsed.orEmpty()
                                    val next = if (hit.nodeId in current) {
                                        current - hit.nodeId
                                    } else {
                                        current + hit.nodeId
                                    }
                                    collapsed = next
                                    val key = item.documentKey
                                    scope.launch(Dispatchers.IO) {
                                        repository.rememberCollapsed(notePath, item.ordinal, next)
                                    }
                                } else {
                                    hit.linkTarget?.let { target -> onOpenLink(target, hit.linkHeading) }
                                }
                            }
                        }
                    },
            ) {
                drawIntoCanvas { canvas ->
                    SceneRenderer.draw(
                        canvas.nativeCanvas,
                        scene,
                        palette.textArgb,
                        zoom = scale,
                    )
                }
            }
            Box(Modifier.align(Alignment.TopEnd)) {
                IconButton(
                    onClick = { menuOpen = true },
                    modifier = Modifier
                        .size(32.dp)
                        .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.75f), RoundedCornerShape(16.dp)),
                ) {
                    Icon(
                        Icons.Default.MoreVert,
                        contentDescription = stringResource(R.string.diagram_menu),
                    )
                }
                DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.diagram_focus)) },
                        onClick = {
                            menuOpen = false
                            onFocus(item, scene, collapsed.orEmpty())
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.export_svg)) },
                        onClick = {
                            menuOpen = false
                            onExport(DiagramExportFormat.Svg, item, scene, collapsed.orEmpty())
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.export_png)) },
                        onClick = {
                            menuOpen = false
                            onExport(DiagramExportFormat.Png, item, scene, collapsed.orEmpty())
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.export_pdf)) },
                        onClick = {
                            menuOpen = false
                            onExport(DiagramExportFormat.Pdf, item, scene, collapsed.orEmpty())
                        },
                    )
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
