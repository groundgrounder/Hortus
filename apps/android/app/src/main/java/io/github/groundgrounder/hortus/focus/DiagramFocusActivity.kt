// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.focus

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.DocumentsContract
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.Canvas as ComposeCanvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.DebugProbes.debugFocusBackTop
import io.github.groundgrounder.hortus.MainActivity
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.settings.LaunchExtras
import io.github.groundgrounder.hortus.reader.model.readerPalette
import io.github.groundgrounder.hortus.reader.render.DiagramExportFormat
import io.github.groundgrounder.hortus.reader.render.DiagramExporter
import io.github.groundgrounder.hortus.reader.render.SceneRenderer
import io.github.groundgrounder.hortus.ui.HortusTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

private const val DOWNLOAD_DOCUMENT_URI =
    "content://com.android.externalstorage.documents/document/primary%3ADownload"

internal class DiagramFocusActivity : ComponentActivity() {
    companion object {
        private const val EXTRA_TOKEN = "io.github.groundgrounder.hortus.FOCUS_TOKEN"

        fun start(context: Context, token: Int) {
            context.startActivity(intent(context, token))
        }

        fun intent(context: Context, token: Int): Intent =
            Intent(context, DiagramFocusActivity::class.java).putExtra(EXTRA_TOKEN, token)
    }

    private var payloadToken: Int = -1
    private var payload: FocusPayload? = null
    private val viewModel: FocusViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        WindowCompat.setDecorFitsSystemWindows(window, false)
        payloadToken = intent.getIntExtra(EXTRA_TOKEN, -1)
        payload = FocusPayloads.get(payloadToken)
        setContent {
            HortusTheme {
                FocusScreen(payload, viewModel, ::finish)
            }
        }
    }

    override fun onPause() {
        super.onPause()
        if (payload == null) return
        DebugProbes.writeFocusDebug(applicationContext)
        viewModel.persistViewport()
    }

    override fun onDestroy() {
        if (isFinishing && payloadToken >= 0) {
            FocusPayloads.remove(payloadToken)
        }
        super.onDestroy()
    }
}

private data class SaveRequest(
    val bytes: ByteArray,
    val name: String,
    val format: DiagramExportFormat,
)

@Composable
private fun FocusScreen(
    payload: FocusPayload?,
    viewModel: FocusViewModel,
    onClose: () -> Unit,
) {
    if (payload == null) return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val dark = isSystemInDarkTheme()
    val palette = readerPalette()
    val density = LocalDensity.current
    val unitScale = viewModel.unitScale
    val systemInsets = WindowInsets.statusBars.union(WindowInsets.displayCutout)
    val topInsetPx = systemInsets.getTop(density)
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    val scene = uiState.scene
    val failure = uiState.failure
    val collapsed = uiState.collapsed
    var viewport by remember { mutableStateOf(IntSize.Zero) }
    var topBarBottomPx by remember { mutableFloatStateOf(0f) }
    var scale by remember { mutableFloatStateOf(0f) }
    var offsetX by remember { mutableFloatStateOf(0f) }
    var offsetY by remember { mutableFloatStateOf(0f) }
    var menuOpen by remember { mutableStateOf(false) }
    var pendingSave by remember { mutableStateOf<SaveRequest?>(null) }

    LaunchedEffect(payload) {
        viewModel.initialize(payload, dark)
    }
    LaunchedEffect(dark) {
        viewModel.onDarkModeChanged(dark)
    }

    val documentLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val uri = result.data?.data
        val request = pendingSave
        pendingSave = null
        if (uri != null && request != null) {
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { DiagramExporter.write(context, uri, request.bytes) }
                    DebugProbes.recordExport(
                        request.format.extension,
                        request.name,
                        request.bytes.size,
                        uri.toString(),
                    )
                } catch (_: Exception) {
                    Unit
                }
            }
        }
    }

    fun fitScale(): Float {
        val value = scene ?: return 1f
        if (viewport.width == 0 || value.width <= 0.0) return 1f
        return min(1f, viewport.width / (value.width.toFloat() * unitScale))
    }

    fun clampOffsets(newScale: Float, x: Float, y: Float): Pair<Float, Float> {
        val value = scene ?: return x to y
        if (viewport.width == 0 || viewport.height == 0) return x to y
        val width = value.width.toFloat() * newScale * unitScale
        val height = value.height.toFloat() * newScale * unitScale
        val top = topBarBottomPx
        val contentHeight = (viewport.height - top).coerceAtLeast(1f)
        val marginX = viewport.width * 0.5f
        val marginY = contentHeight * 0.5f
        val minX = min(0f, (viewport.width - width) / 2f) - marginX
        val maxX = max(0f, (width - viewport.width) / 2f) + marginX
        val minY = top + min(0f, (contentHeight - height) / 2f) - marginY
        val maxY = top + max(0f, (height - contentHeight) / 2f) + marginY
        return x.coerceIn(minX, maxX) to y.coerceIn(minY, maxY)
    }

    fun applyFit() {
        val value = scene ?: return
        val fit = fitScale()
        val top = topBarBottomPx
        val contentHeight = (viewport.height - top).coerceAtLeast(1f)
        val drawnWidth = value.width.toFloat() * fit * unitScale
        val drawnHeight = value.height.toFloat() * fit * unitScale
        scale = fit
        offsetX = (viewport.width - drawnWidth) / 2f
        offsetY = if (drawnHeight <= contentHeight) {
            top + (contentHeight - drawnHeight) / 2f
        } else {
            top
        }
        val (x, y) = clampOffsets(scale, offsetX, offsetY)
        offsetX = x
        offsetY = y
    }

    fun applyActual() {
        val value = scene ?: return
        val top = topBarBottomPx
        val contentHeight = (viewport.height - top).coerceAtLeast(1f)
        val drawnWidth = value.width.toFloat() * unitScale
        val drawnHeight = value.height.toFloat() * unitScale
        scale = 1f
        offsetX = (viewport.width - drawnWidth) / 2f
        offsetY = top + (contentHeight - drawnHeight) / 2f
        val (x, y) = clampOffsets(scale, offsetX, offsetY)
        offsetX = x
        offsetY = y
    }

    fun rememberNow() {
        viewModel.updateViewport(scale, offsetX, offsetY)
        DebugProbes.recordFocusState(
            scale = scale,
            offsetX = offsetX,
            offsetY = offsetY,
            fitScale = fitScale(),
            unitScale = unitScale,
            scene = scene,
            documentKey = payload.documentKey,
            notePath = payload.notePath,
            ordinal = payload.ordinal,
            collapsed = collapsed,
        )
    }

    fun toggleNode(nodeId: String) {
        viewModel.toggleNode(nodeId, dark)
    }

    fun openNote(target: String, heading: String?) {
        scope.launch {
            val resolved = viewModel.resolveVaultLink(target) ?: target
            context.startActivity(
                Intent(context, MainActivity::class.java).apply {
                    putExtra(LaunchExtras.EXTRA_OPEN_NOTE, resolved)
                    putExtra(LaunchExtras.EXTRA_OPEN_HEADING, heading)
                    putExtra(LaunchExtras.EXTRA_OPEN_VAULT, payload.vaultPath)
                },
            )
        }
    }

    fun share(format: DiagramExportFormat) {
        scope.launch {
            try {
                val bytes = withContext(Dispatchers.IO) { viewModel.encode(format, dark) }
                val name = DiagramExporter.defaultName(payload.notePath, payload.ordinal, format)
                val file = withContext(Dispatchers.IO) {
                    DiagramExporter.shareFile(context, name, bytes)
                }
                val uri = DiagramExporter.shareUri(context, file)
                if (DebugProbes.enabled) {
                    val readBack = withContext(Dispatchers.IO) { DiagramExporter.readBack(context, uri) }
                    DebugProbes.recordShare(
                        Intent.ACTION_SEND,
                        format.mime,
                        uri.toString(),
                        name,
                        bytes.size,
                        readBack?.contentEquals(bytes) == true,
                        readBack?.size ?: -1,
                    )
                }
                val intent = DiagramExporter.shareIntent(context, file, format, payload.title)
                context.startActivity(Intent.createChooser(intent, context.getString(R.string.share_sheet_title)))
            } catch (_: Exception) {
                Unit
            }
        }
    }

    fun save(format: DiagramExportFormat) {
        scope.launch {
            try {
                val bytes = withContext(Dispatchers.IO) { viewModel.encode(format, dark) }
                val name = DiagramExporter.defaultName(payload.notePath, payload.ordinal, format)
                pendingSave = SaveRequest(bytes, name, format)
                val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                    addCategory(Intent.CATEGORY_OPENABLE)
                    type = format.mime
                    putExtra(Intent.EXTRA_TITLE, name)
                    putExtra(DocumentsContract.EXTRA_INITIAL_URI, Uri.parse(DOWNLOAD_DOCUMENT_URI))
                }
                documentLauncher.launch(intent)
            } catch (_: Exception) {
                Unit
            }
        }
    }

    LaunchedEffect(scene, viewport, topBarBottomPx) {
        if (scene == null) return@LaunchedEffect
        if (viewport.width == 0 || viewport.height == 0 || topBarBottomPx <= 0f) return@LaunchedEffect
        if (scale == 0f) {
            val fit = fitScale()
            val stored = viewModel.loadStoredFocus()
            if (stored != null && stored.scale >= fit - 0.01 && stored.scale <= 4.0) {
                scale = stored.scale.toFloat().coerceIn(fit, 4f)
                val (x, y) = clampOffsets(scale, stored.offsetX.toFloat(), stored.offsetY.toFloat())
                offsetX = x
                offsetY = y
            } else {
                applyFit()
            }
        } else {
            val (x, y) = clampOffsets(scale, offsetX, offsetY)
            offsetX = x
            offsetY = y
        }
        rememberNow()
        DebugProbes.writeFocusDebug(context)
    }
    LaunchedEffect(scale, offsetX, offsetY, scene, collapsed) { rememberNow() }

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
                        var lastTapPosition = Offset.Zero
                        awaitEachGesture {
                            val first = awaitFirstDown(requireUnconsumed = false)
                            var previousCentroid = first.position
                            var previousDistance = 0f
                            var moved = 0f
                            var tapPosition = first.position
                            while (true) {
                                val event = awaitPointerEvent()
                                DebugProbes.recordFocusPointerEvent()
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
                                DebugProbes.recordFocusGestureDistance(distance)
                                val fit = fitScale()
                                if (pressed.size >= 2 && previousDistance > 1f && distance > 1f) {
                                    val next = (scale * (distance / previousDistance)).coerceIn(fit, 4f)
                                    val sceneX = (centroid.x - offsetX) / (scale * unitScale)
                                    val sceneY = (centroid.y - offsetY) / (scale * unitScale)
                                    scale = next
                                    offsetX = centroid.x - sceneX * next * unitScale
                                    offsetY = centroid.y - sceneY * next * unitScale
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
                                    if (scale > fitScale() + 0.05f) {
                                        applyFit()
                                    } else {
                                        applyActual()
                                    }
                                } else {
                                    lastTapUp = now
                                    lastTapPosition = tapPosition
                                    val sceneX = (tapPosition.x - offsetX) / (scale * unitScale)
                                    val sceneY = (tapPosition.y - offsetY) / (scale * unitScale)
                                    val hit = SceneRenderer.hitAt(value, sceneX, sceneY)
                                    DebugProbes.recordFocusTap(sceneX, sceneY, hit)
                                    if (hit != null) {
                                        if (hit.canCollapse) {
                                            toggleNode(hit.nodeId)
                                        } else {
                                            hit.linkTarget?.let { target -> openNote(target, hit.linkHeading) }
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
                        zoom = scale * unitScale,
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
                .windowInsetsPadding(systemInsets)
                .onGloballyPositioned { coordinates ->
                    val bottom = coordinates.positionInRoot().y + coordinates.size.height
                    DebugProbes.recordFocusChrome(topInsetPx, bottom.roundToInt())
                    if (abs(bottom - topBarBottomPx) > 0.5f) {
                        topBarBottomPx = bottom
                    }
                }
                .padding(horizontal = 4.dp, vertical = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(
                onClick = onClose,
                modifier = Modifier.debugFocusBackTop(),
            ) {
                Icon(Icons.Default.ArrowBack, contentDescription = stringResource(R.string.reader_back))
            }
            Text(
                payload.title.ifBlank { stringResource(R.string.focus_title) },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(stringResource(R.string.focus_zoom, (scale * 100).roundToInt()))
            Box {
                IconButton(onClick = { menuOpen = true }) {
                    Icon(Icons.Default.MoreVert, contentDescription = stringResource(R.string.diagram_menu))
                }
                DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.focus_fit)) },
                        onClick = {
                            menuOpen = false
                            applyFit()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.focus_actual)) },
                        onClick = {
                            menuOpen = false
                            applyActual()
                        },
                    )
                    for (format in DiagramExportFormat.entries) {
                        DropdownMenuItem(
                            text = {
                                Text(
                                    when (format) {
                                        DiagramExportFormat.Svg -> stringResource(R.string.share_svg)
                                        DiagramExportFormat.Png -> stringResource(R.string.share_png)
                                        DiagramExportFormat.Pdf -> stringResource(R.string.share_pdf)
                                    },
                                )
                            },
                            onClick = {
                                menuOpen = false
                                share(format)
                            },
                        )
                    }
                    for (format in DiagramExportFormat.entries) {
                        DropdownMenuItem(
                            text = {
                                Text(
                                    when (format) {
                                        DiagramExportFormat.Svg -> stringResource(R.string.save_svg)
                                        DiagramExportFormat.Png -> stringResource(R.string.save_png)
                                        DiagramExportFormat.Pdf -> stringResource(R.string.save_pdf)
                                    },
                                )
                            },
                            onClick = {
                                menuOpen = false
                                save(format)
                            },
                        )
                    }
                }
            }
        }
    }
}
