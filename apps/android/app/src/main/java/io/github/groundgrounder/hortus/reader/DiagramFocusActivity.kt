// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas as ComposeCanvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import io.github.groundgrounder.hortus.AndroidFonts
import io.github.groundgrounder.hortus.CoreEnvironment
import io.github.groundgrounder.hortus.MainActivity
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.DiagramEngine
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.WorkspaceCore
import io.github.groundgrounder.hortus.ui.HortusTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.max
import kotlin.math.min

internal class DiagramFocusActivity : ComponentActivity() {
    companion object {
        private const val EXTRA_TOKEN = "io.github.groundgrounder.hortus.FOCUS_TOKEN"

        fun start(context: Context, token: Int) {
            context.startActivity(intent(context, token))
        }

        fun intent(context: Context, token: Int): Intent =
            Intent(context, DiagramFocusActivity::class.java).putExtra(EXTRA_TOKEN, token)
    }

    private var payload: FocusPayload? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
        payload = FocusPayloads.get(intent.getIntExtra(EXTRA_TOKEN, -1))
        setContent {
            HortusTheme {
                FocusScreen(payload, ::finish)
            }
        }
    }

    override fun onPause() {
        super.onPause()
        val value = payload ?: return
        val scale = FocusStateRegistry.lastScale
        if (scale <= 0f) return
        val appContext = applicationContext
        CoroutineScope(Dispatchers.IO).launch {
            val core = try {
                CoreEnvironment.createWorkspaceCore(appContext)
            } catch (_: Exception) {
                return@launch
            }
            try {
                core.rememberDiagramFocus(
                    value.vaultPath,
                    value.notePath,
                    value.ordinal.toUInt(),
                    scale.toDouble(),
                    FocusStateRegistry.lastOffsetX.toDouble(),
                    FocusStateRegistry.lastOffsetY.toDouble(),
                )
            } catch (_: Exception) {
                Unit
            } finally {
                try {
                    core.close()
                } catch (_: Exception) {
                    Unit
                }
            }
        }
    }
}

@Composable
private fun FocusScreen(payload: FocusPayload?, onClose: () -> Unit) {
    if (payload == null) return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val dark = isSystemInDarkTheme()
    val palette = readerPalette()
    var scene by remember { mutableStateOf<SceneData?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var collapsed by remember { mutableStateOf(payload.collapsed) }
    var viewport by remember { mutableStateOf(IntSize.Zero) }
    var scale by remember { mutableStateOf(0f) }
    var offsetX by remember { mutableStateOf(0f) }
    var offsetY by remember { mutableStateOf(0f) }
    var menuOpen by remember { mutableStateOf(false) }
    var pendingBytes by remember { mutableStateOf<ByteArray?>(null) }
    var pendingName by remember { mutableStateOf("") }
    var pendingFormat by remember { mutableStateOf(DiagramExportFormat.Png) }
    val engine = remember { FocusEngine(context.applicationContext) }
    val documentLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val uri = result.data?.data
        val bytes = pendingBytes
        pendingBytes = null
        if (uri != null && bytes != null) {
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { DiagramExporter.write(context, uri, bytes) }
                    Inspection.recordExport(pendingFormat.extension, pendingName, bytes.size, uri.toString())
                } catch (_: Exception) {
                    Unit
                }
            }
        }
    }

    fun clampOffsets(newScale: Float, x: Float, y: Float): Pair<Float, Float> {
        val value = scene ?: return x to y
        if (viewport.width == 0 || viewport.height == 0) return x to y
        val width = value.width.toFloat() * newScale
        val height = value.height.toFloat() * newScale
        val marginX = viewport.width * 0.5f
        val marginY = viewport.height * 0.5f
        val minX = min(0f, (viewport.width - width) / 2f) - marginX
        val maxX = max(0f, (width - viewport.width) / 2f) + marginX
        val minY = min(0f, (viewport.height - height) / 2f) - marginY
        val maxY = max(0f, (height - viewport.height) / 2f) + marginY
        return x.coerceIn(minX, maxX) to y.coerceIn(minY, maxY)
    }

    fun fitValues(): Pair<Float, Pair<Float, Float>> {
        val value = scene ?: return 1f to (0f to 0f)
        if (viewport.width == 0 || viewport.height == 0) return 1f to (0f to 0f)
        val fit = min(1f, min(viewport.width / value.width.toFloat(), viewport.height / value.height.toFloat()))
        return fit to (
            (viewport.width - value.width.toFloat() * fit) / 2f to
                (viewport.height - value.height.toFloat() * fit) / 2f
            )
    }

    fun applyFit() {
        val (fit, offsets) = fitValues()
        scale = fit
        offsetX = offsets.first
        offsetY = offsets.second
    }

    fun rememberNow() {
        FocusStateRegistry.lastScale = scale
        FocusStateRegistry.lastOffsetX = offsetX
        FocusStateRegistry.lastOffsetY = offsetY
        FocusStateRegistry.sceneWidth = scene?.width?.toFloat() ?: 0f
        FocusStateRegistry.sceneHeight = scene?.height?.toFloat() ?: 0f
        val base = scene?.primitives
            ?.filterIsInstance<io.github.groundgrounder.hortus.core.ScenePrimitive.Text>()
            ?.maxOfOrNull { it.span.fontSize.toFloat() } ?: 0f
        FocusStateRegistry.baseLargestFontPx = base
        FocusStateRegistry.largestFontPx = base * scale
    }

    fun toggleNode(nodeId: String) {
        val next = if (nodeId in collapsed) collapsed - nodeId else collapsed + nodeId
        collapsed = next
        scope.launch {
            try {
                engine.rememberCollapsed(next)
            } catch (_: Exception) {
                Unit
            }
        }
    }

    LaunchedEffect(payload.documentKey, payload.ordinal, dark, collapsed) {
        try {
            scene = engine.render(payload, collapsed, dark)
            failure = null
        } catch (error: Exception) {
            failure = error.message ?: error.javaClass.simpleName
            scene = null
        }
    }
    LaunchedEffect(scene, viewport) {
        val value = scene ?: return@LaunchedEffect
        if (viewport.width == 0 || viewport.height == 0) return@LaunchedEffect
        if (scale == 0f) {
            val stored = engine.loadFocus()
            if (stored != null) {
                scale = stored.scale.toFloat().coerceIn(0.25f, 4f)
                offsetX = stored.offsetX.toFloat()
                offsetY = stored.offsetY.toFloat()
                val (x, y) = clampOffsets(scale, offsetX, offsetY)
                offsetX = x
                offsetY = y
            } else {
                applyFit()
            }
        }
        rememberNow()
    }
    LaunchedEffect(scale, offsetX, offsetY, scene) { rememberNow() }

    Box(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .onSizeChanged { viewport = it },
    ) {
        val value = scene
        if (value != null) {
            ComposeCanvas(
                Modifier
                    .fillMaxSize()
                    .pointerInput(value) {
                        var lastTapUp = 0L
                        var lastTapPosition = androidx.compose.ui.geometry.Offset.Zero
                        awaitEachGesture {
                            val first = awaitFirstDown(requireUnconsumed = false)
                            var previousCentroid = first.position
                            var previousDistance = 0f
                            var moved = 0f
                            var tapPosition = first.position
                            while (true) {
                                val event = awaitPointerEvent()
                                FocusStateRegistry.pointerEvents += 1
                                if (event.type == PointerEventType.Release) break
                                val pressed = event.changes.filter { it.pressed }.ifEmpty { event.changes }
                                val centroid = pressed.map { it.position }
                                    .reduce { left, right -> left + right } / pressed.size.toFloat()
                                tapPosition = centroid
                                val distance = if (pressed.size >= 2) {
                                    (pressed[0].position - pressed[1].position).getDistance()
                                } else {
                                    0f
                                }
                                FocusStateRegistry.gestureDistance = distance
                                if (pressed.size >= 2 && previousDistance > 1f && distance > 1f) {
                                    val next = (scale * (distance / previousDistance)).coerceIn(0.25f, 4f)
                                    val sceneX = (centroid.x - offsetX) / scale
                                    val sceneY = (centroid.y - offsetY) / scale
                                    scale = next
                                    offsetX = centroid.x - sceneX * next
                                    offsetY = centroid.y - sceneY * next
                                }
                                val pan = centroid - previousCentroid
                                if (pan.getDistance() > 0.01f) {
                                    offsetX += pan.x
                                    offsetY += pan.y
                                    moved += pan.getDistance()
                                }
                                val (clampedX, clampedY) = clampOffsets(scale, offsetX, offsetY)
                                offsetX = clampedX
                                offsetY = clampedY
                                previousCentroid = centroid
                                previousDistance = distance
                                event.changes.forEach { it.consume() }
                            }
                            if (moved < 18f) {
                                val now = android.os.SystemClock.uptimeMillis()
                                val isDouble = now - lastTapUp < 340 &&
                                    (tapPosition - lastTapPosition).getDistance() < 120f
                                if (isDouble) {
                                    lastTapUp = 0
                                    val (fit, _) = fitValues()
                                    if (scale > fit + 0.05f) {
                                        applyFit()
                                    } else {
                                        scale = 1f
                                        offsetX = (viewport.width - value.width.toFloat()) / 2f
                                        offsetY = (viewport.height - value.height.toFloat()) / 2f
                                        val (x, y) = clampOffsets(scale, offsetX, offsetY)
                                        offsetX = x
                                        offsetY = y
                                    }
                                } else {
                                    lastTapUp = now
                                    lastTapPosition = tapPosition
                                    val hit = SceneRenderer.hitAt(
                                        value,
                                        (tapPosition.x - offsetX) / scale,
                                        (tapPosition.y - offsetY) / scale,
                                    )
                                    if (hit != null) {
                                        if (hit.canCollapse) {
                                            toggleNode(hit.nodeId)
                                        } else {
                                            hit.linkTarget?.let { target ->
                                                context.startActivity(
                                                    Intent(context, MainActivity::class.java).apply {
                                                        putExtra(MainActivity.EXTRA_OPEN_NOTE, target)
                                                        putExtra(MainActivity.EXTRA_OPEN_HEADING, hit.linkHeading)
                                                        addFlags(
                                                            Intent.FLAG_ACTIVITY_SINGLE_TOP or
                                                                Intent.FLAG_ACTIVITY_CLEAR_TOP,
                                                        )
                                                    },
                                                )
                                                onClose()
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
            ) {
                drawIntoCanvas { canvas ->
                    SceneRenderer.draw(
                        canvas.nativeCanvas,
                        value,
                        palette.textArgb,
                        zoom = scale,
                        translateX = offsetX,
                        translateY = offsetY,
                    )
                }
            }
        } else {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(failure ?: stringResource(R.string.reader_loading), color = palette.muted)
            }
        }
        Row(
            Modifier
                .fillMaxWidth()
                .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.92f))
                .padding(horizontal = 4.dp, vertical = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onClose) {
                Icon(Icons.Default.ArrowBack, contentDescription = stringResource(R.string.reader_back))
            }
            Text(
                payload.title.ifBlank { stringResource(R.string.focus_title) },
                maxLines = 1,
                modifier = Modifier.weight(1f),
            )
            Text(stringResource(R.string.focus_zoom, (scale * 100).toInt()))
            TextButton(onClick = { applyFit() }) { Text(stringResource(R.string.focus_fit)) }
            TextButton(
                onClick = {
                    scale = 1f
                    offsetX = (viewport.width - (scene?.width?.toFloat() ?: 0f)) / 2f
                    offsetY = (viewport.height - (scene?.height?.toFloat() ?: 0f)) / 2f
                },
            ) { Text(stringResource(R.string.focus_actual)) }
            Box {
                IconButton(onClick = { menuOpen = true }) {
                    Icon(Icons.Default.MoreVert, contentDescription = stringResource(R.string.diagram_menu))
                }
                DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                    for (format in DiagramExportFormat.entries) {
                        DropdownMenuItem(
                            text = {
                                Text(
                                    when (format) {
                                        DiagramExportFormat.Svg -> stringResource(R.string.export_svg)
                                        DiagramExportFormat.Png -> stringResource(R.string.export_png)
                                        DiagramExportFormat.Pdf -> stringResource(R.string.export_pdf)
                                    },
                                )
                            },
                            onClick = {
                                menuOpen = false
                                val current = scene ?: return@DropdownMenuItem
                                scope.launch {
                                    try {
                                        val bytes = when (format) {
                                            DiagramExportFormat.Svg -> engine.renderSvg(payload, collapsed, dark)
                                            DiagramExportFormat.Png -> DiagramExporter.png(current, dark)
                                            DiagramExportFormat.Pdf -> DiagramExporter.pdf(current, dark)
                                        }
                                        pendingBytes = bytes
                                        pendingFormat = format
                                        pendingName = DiagramExporter.defaultName(payload.notePath, payload.ordinal, format)
                                        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                                            addCategory(Intent.CATEGORY_OPENABLE)
                                            type = format.mime
                                            putExtra(Intent.EXTRA_TITLE, pendingName)
                                        }
                                        documentLauncher.launch(intent)
                                    } catch (_: Exception) {
                                        Unit
                                    }
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}

private class FocusEngine(private val appContext: Context) {
    private var engine: DiagramEngine? = null
    private var workspace: WorkspaceCore? = null
    private var payload: FocusPayload? = null

    private suspend fun prepare() {
        if (engine != null) return
        withContext(Dispatchers.IO) {
            val paths = AndroidFonts.discover().mapNotNull { it.file?.path }.distinct()
            engine = DiagramEngine.withFontPaths(paths)
            workspace = CoreEnvironment.createWorkspaceCore(appContext)
        }
    }

    suspend fun render(value: FocusPayload, collapsed: List<String>, dark: Boolean): SceneData {
        prepare()
        payload = value
        return withContext(Dispatchers.IO) {
            val diagram = engine ?: throw IllegalStateException("diagram engine unavailable")
            diagram.renderRequest(
                source = value.engineText,
                mmd = value.mmd,
                width = value.renderWidth,
                collapsed = collapsed,
                identity = io.github.groundgrounder.hortus.core.DiagramIdentity(
                    document = value.documentKey,
                    block = value.ordinal.toUInt(),
                    revision = value.revision.toULong(),
                    dark = dark,
                    fontGeneration = 1u,
                    requestGeneration = 1u,
                ),
                previous = null,
                viewport = emptyList(),
            )
        }
    }

    suspend fun renderSvg(value: FocusPayload, collapsed: List<String>, dark: Boolean): ByteArray {
        prepare()
        payload = value
        return withContext(Dispatchers.IO) {
            val diagram = engine ?: throw IllegalStateException("diagram engine unavailable")
            diagram.renderSvg(
                value.engineText,
                value.mmd,
                value.ordinal.toUInt(),
                value.renderWidth,
                dark,
                collapsed,
            ).toByteArray(Charsets.UTF_8)
        }
    }

    suspend fun rememberCollapsed(collapsed: List<String>) {
        val value = payload ?: return
        prepare()
        withContext(Dispatchers.IO) {
            try {
                workspace?.rememberDiagram(value.vaultPath, value.notePath, value.ordinal.toUInt(), collapsed)
            } catch (_: Exception) {
                Unit
            }
        }
    }

    suspend fun loadFocus(): io.github.groundgrounder.hortus.core.DiagramFocus? {
        val value = payload ?: return null
        prepare()
        return withContext(Dispatchers.IO) {
            try {
                workspace?.diagramFocus(value.vaultPath, value.notePath, value.ordinal.toUInt())
            } catch (_: Exception) {
                null
            }
        }
    }
}
