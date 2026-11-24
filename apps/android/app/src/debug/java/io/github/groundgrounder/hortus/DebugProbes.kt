// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.app.Application
import android.content.Context
import android.content.pm.ApplicationInfo
import android.graphics.RectF
import android.os.SystemClock
import android.text.Layout
import android.text.StaticLayout
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.layout.positionInWindow
import io.github.groundgrounder.hortus.core.EmbedContentInfo
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MediaBlock
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.SceneHit
import io.github.groundgrounder.hortus.core.ScenePrimitive
import io.github.groundgrounder.hortus.reader.DebugPreviewInspection
import io.github.groundgrounder.hortus.reader.DebugSessionMetrics
import io.github.groundgrounder.hortus.reader.DiagramInspection
import io.github.groundgrounder.hortus.reader.FocusChromeMetrics
import io.github.groundgrounder.hortus.reader.FocusDebug
import io.github.groundgrounder.hortus.reader.FocusStateRegistry
import io.github.groundgrounder.hortus.reader.HitRegistry
import io.github.groundgrounder.hortus.reader.ImageInspection
import io.github.groundgrounder.hortus.reader.Inspection
import io.github.groundgrounder.hortus.reader.LayoutInspection
import io.github.groundgrounder.hortus.reader.LineMetrics
import io.github.groundgrounder.hortus.reader.LoadedImage
import io.github.groundgrounder.hortus.reader.MainThreadCounters
import io.github.groundgrounder.hortus.reader.MathBoxMetrics
import io.github.groundgrounder.hortus.reader.MathRenderResult
import io.github.groundgrounder.hortus.reader.PanelHit
import io.github.groundgrounder.hortus.reader.PanelRegistry
import io.github.groundgrounder.hortus.reader.PreparedMath
import io.github.groundgrounder.hortus.reader.ReaderTextView
import io.github.groundgrounder.hortus.reader.ReaderTiming
import io.github.groundgrounder.hortus.reader.ScreenHit
import io.github.groundgrounder.hortus.reader.ScrollTarget
import io.github.groundgrounder.hortus.reader.diagramMainFontSize
import io.github.groundgrounder.hortus.reader.mathInlinePadding
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** Debug 构建的验收探针实现；Release 与 benchmark 由同名空实现替换。 */
internal object DebugProbes {
    const val enabled: Boolean = true

    fun init(application: Application) {
        Inspection.recordingEnabled =
            (application.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
    }

    inline fun <T> traceMainThread(name: String, block: () -> T): T =
        MainThreadCounters.record(name, block)

    fun recordImage(path: String, image: LoadedImage) {
        ImageInspection.record(path, image)
    }

    fun recordMath(note: String, formula: MathBlock, result: MathRenderResult, bodySize: Float) {
        Inspection.recordMath(note, formula, result, bodySize)
    }

    fun recordDiagram(
        documentKey: String,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        scene: SceneData,
    ) {
        if (!Inspection.recordingEnabled) return
        val counts = DiagramInspection.measureScene(scene, documentKey)
        DiagramInspection.record(
            "$documentKey#$ordinal@${width.toInt()}/$dark",
            scene,
            counts,
            dark,
            collapsed,
        )
    }

    fun recordDiagramFailure(key: String, ordinal: Int, reason: String) {
        Inspection.recordDiagramFailure(key, ordinal, reason)
    }

    fun recordEmbed(key: String, info: EmbedContentInfo) {
        Inspection.recordEmbed(key, info)
    }

    fun recordExport(format: String, name: String, bytes: Int, path: String?) {
        Inspection.recordExport(format, name, bytes, path)
    }

    fun recordShare(
        action: String,
        mime: String,
        uri: String,
        name: String,
        bytes: Int,
        contentMatches: Boolean,
        contentBytes: Int,
    ) {
        Inspection.recordShare(action, mime, uri, name, bytes, contentMatches, contentBytes)
    }

    fun onDiagramPreviewRecorded(key: String, scene: SceneData, zoom: Float) {
        if (Inspection.recordingEnabled) {
            DebugPreviewInspection.record(key, scene, zoom)
        }
    }

    fun onDiagramPreviewReplayed() {
        DebugPreviewInspection.noteReplay()
    }

    fun Modifier.debugDiagramRect(key: String): Modifier = onGloballyPositioned { coordinates ->
        val origin = coordinates.positionInWindow()
        Inspection.recordDiagramRect(
            key,
            origin.x,
            origin.y,
            coordinates.size.width.toFloat(),
            coordinates.size.height.toFloat(),
        )
    }

    fun Modifier.debugItemBounds(key: String, blockId: Long): Modifier =
        onGloballyPositioned { coordinates ->
            LayoutInspection.recordItemBounds(
                key,
                blockId,
                coordinates.positionInWindow().y,
                coordinates.size.height.toFloat(),
            )
        }

    fun Modifier.debugPanel(
        scope: String,
        key: String,
        kind: String,
        label: String,
        target: String?,
        tab: String,
    ): Modifier = onGloballyPositioned { coordinates ->
        val position = coordinates.positionInWindow()
        PanelRegistry.put(
            scope,
            key,
            PanelHit(
                kind = kind,
                label = label,
                x = position.x,
                y = position.y,
                width = coordinates.size.width.toFloat(),
                height = coordinates.size.height.toFloat(),
                target = target,
                tab = tab,
            ),
        )
    }

    fun Modifier.debugPaneWidth(isSide: Boolean): Modifier = onGloballyPositioned { coordinates ->
        if (isSide) {
            DebugSessionMetrics.sidePaneWidthPx = coordinates.size.width
        } else {
            DebugSessionMetrics.detailPaneWidthPx = coordinates.size.width
        }
    }

    fun clearSidePaneWidth() {
        DebugSessionMetrics.sidePaneWidthPx = 0
    }

    fun Modifier.debugTextHits(enabled: Boolean, onRefresh: () -> Unit): Modifier =
        onGloballyPositioned { if (enabled) onRefresh() }

    fun Modifier.debugFocusBackTop(): Modifier = onGloballyPositioned { coordinates ->
        FocusChromeMetrics.topBarContentTopPx = coordinates.positionInRoot().y.roundToInt()
    }

    fun retainPanels(scope: String, keys: Collection<String>) {
        PanelRegistry.retain(scope, keys)
    }

    fun clearPanels(scope: String) {
        PanelRegistry.clear(scope)
    }

    fun clearTextHits(scope: String) {
        HitRegistry.replace(scope, emptyList())
    }

    fun clearAllHits() {
        HitRegistry.clear()
    }

    fun refreshTextHits(view: ReaderTextView) {
        if (view.links.isEmpty() && view.tags.isEmpty()) {
            HitRegistry.replace("text:${view.itemKey}", emptyList())
            return
        }
        val layout = view.layout ?: return
        val location = IntArray(2)
        view.getLocationOnScreen(location)
        val hits = ArrayList<ScreenHit>(view.links.size + view.tags.size)
        for (link in view.links) {
            for (rect in layoutRects(layout, link.start, link.end)) {
                hits += ScreenHit(
                    kind = "link",
                    label = link.alias ?: link.target,
                    x = location[0] + view.totalPaddingLeft + rect.left,
                    y = location[1] + view.totalPaddingTop + rect.top,
                    width = rect.width(),
                    height = rect.height(),
                    target = link.resolvedPath ?: link.target,
                    heading = link.heading,
                    canCollapse = false,
                    nodeId = null,
                )
            }
        }
        for (tag in view.tags) {
            for (rect in layoutRects(layout, tag.start, tag.end)) {
                hits += ScreenHit(
                    kind = "tag",
                    label = tag.tag,
                    x = location[0] + view.totalPaddingLeft + rect.left,
                    y = location[1] + view.totalPaddingTop + rect.top,
                    width = rect.width(),
                    height = rect.height(),
                    target = tag.tag,
                    heading = null,
                    canCollapse = false,
                    nodeId = null,
                )
            }
        }
        HitRegistry.replace("text:${view.itemKey}", hits)
    }

    fun refreshTextLayout(view: ReaderTextView) {
        refreshTextHits(view)
        if (!Inspection.recordingEnabled) return
        val layout = view.layout ?: return
        val lines = ArrayList<LineMetrics>(layout.lineCount)
        for (line in 0 until layout.lineCount) {
            lines += LineMetrics(
                index = line,
                top = layout.getLineTop(line) + view.totalPaddingTop.toFloat(),
                bottom = layout.getLineBottom(line) + view.totalPaddingTop.toFloat(),
                baselineY = layout.getLineBaseline(line) + view.totalPaddingTop.toFloat(),
            )
        }
        val boxes = ArrayList<MathBoxMetrics>()
        val mathByOffset = view.mathByOffset
        val mathLatex = view.mathLatex
        for (offset in mathByOffset.keys) {
            val prepared = mathByOffset[offset] ?: continue
            if (offset !in 0 until layout.text.length) continue
            val line = layout.getLineForOffset(offset)
            val padding = mathInlinePadding(prepared.em)
            val baseline = layout.getLineBaseline(line) + view.totalPaddingTop.toFloat()
            boxes += MathBoxMetrics(
                formulaKey = "$offset:${mathLatex[offset].orEmpty()}",
                latex = mathLatex[offset].orEmpty(),
                lineIndex = line,
                x = layout.getPrimaryHorizontal(offset) + view.totalPaddingLeft + padding,
                top = baseline - prepared.ascent,
                baselineY = baseline,
                bottom = baseline + prepared.descent,
                width = prepared.width + padding * 2f,
                em = prepared.em,
                rendered = true,
            )
        }
        LayoutInspection.recordText(
            "text:${view.itemKey}",
            view.notePath,
            view.blockId,
            lines,
            boxes,
        )
    }

    private fun layoutRects(layout: Layout, start: Int, end: Int): List<RectF> {
        if (start >= end || start < 0) return emptyList()
        val first = layout.getLineForOffset(start)
        val last = layout.getLineForOffset((end - 1).coerceAtLeast(start))
        val result = ArrayList<RectF>(last - first + 1)
        for (line in first..last) {
            val lineStart = layout.getLineStart(line)
            val lineEnd = layout.getLineEnd(line)
            val s = max(start, lineStart).coerceAtMost(lineEnd)
            val e = min(end, lineEnd).coerceAtLeast(s)
            val x1 = if (s >= lineEnd) layout.getLineRight(line) else layout.getPrimaryHorizontal(s)
            val x2 = if (e >= lineEnd) layout.getLineRight(line) else layout.getPrimaryHorizontal(e)
            result += RectF(
                min(x1, x2),
                layout.getLineTop(line).toFloat(),
                max(x1, x2),
                layout.getLineBottom(line).toFloat(),
            )
        }
        return result
    }

    fun recordTableLayout(
        media: MediaBlock,
        notePath: String,
        density: Float,
        layouts: List<StaticLayout>,
        rowHeights: List<Float>,
        mathResults: Map<String, MathRenderResult>,
    ) {
        if (!Inspection.recordingEnabled) return
        val columns = media.columns.map { it.toFloat() }
        val cells = ArrayList<JSONObject>()
        var y = 0f
        for (row in 0 until max(1, media.rows.toInt())) {
            var x = 0f
            for (column in columns.indices) {
                val index = row * columns.size + column
                val layout = layouts.getOrNull(index)
                val cell = media.cells.getOrNull(index)
                if (layout != null) {
                    val alignment = media.alignments.getOrNull(column)?.toInt() ?: 0
                    val available = max(0f, columns[column] - 20f * density)
                    val offset = when (alignment) {
                        1 -> (available - layout.width) / 2f
                        2 -> available - layout.width
                        else -> 0f
                    }
                    val cellX = x + 10f * density + offset
                    val cellY = y + 8f * density
                    val lines = JSONArray()
                    for (line in 0 until layout.lineCount) {
                        lines.put(JSONObject()
                            .put("line", line)
                            .put("top", cellY + layout.getLineTop(line))
                            .put("bottom", cellY + layout.getLineBottom(line))
                            .put("baselineY", cellY + layout.getLineBaseline(line)))
                    }
                    val formulas = JSONArray()
                    for (formula in cell?.math.orEmpty()) {
                        val local = formula.displayStart.toInt()
                        if (local !in 0 until layout.text.length) continue
                        val math = mathResults["${formula.displayStart}:${formula.latex}:${formula.style}"]?.prepared
                        val formulaLine = layout.getLineForOffset(local)
                        val padding = math?.let { mathInlinePadding(it.em) } ?: 0f
                        formulas.put(JSONObject()
                            .put("key", "${formula.displayStart}:${formula.latex}:${formula.style}")
                            .put("latex", formula.latex)
                            .put("line", formulaLine)
                            .put("x", cellX + layout.getPrimaryHorizontal(local) + padding)
                            .put("top", if (math != null) cellY + layout.getLineBaseline(formulaLine) - math.ascent else 0f)
                            .put("baselineY", cellY + layout.getLineBaseline(formulaLine))
                            .put("bottom", if (math != null) cellY + layout.getLineBaseline(formulaLine) + math.descent else 0f)
                            .put("width", if (math != null) math.width + padding * 2f else 0f)
                            .put("em", math?.em ?: 0f)
                            .put("rendered", math != null))
                    }
                    cells.add(JSONObject()
                        .put("row", row)
                        .put("column", column)
                        .put("x", cellX)
                        .put("y", cellY)
                        .put("width", layout.width)
                        .put("height", layout.height)
                        .put("lines", lines)
                        .put("formulas", formulas))
                }
                x += columns[column]
            }
            y += rowHeights.getOrElse(row) { 0f }
        }
        LayoutInspection.recordTable(
            key = "table:${media.blockId}:${media.displayStart}",
            note = notePath,
            blockId = media.blockId.toLong(),
            columns = columns,
            rowHeights = rowHeights,
            cells = cells,
        )
    }

    fun recordBlockMath(
        itemKey: String,
        notePath: String,
        blockId: Long,
        bodySizePx: Float,
        verticalPadding: Float,
        formulas: List<MathBlock>,
        results: Map<MathBlock, MathRenderResult>,
    ) {
        if (!Inspection.recordingEnabled || results.size < formulas.size) return
        val records = JSONArray()
        for (formula in formulas) {
            val result = results[formula]
            val prepared = result?.prepared
            records.put(
                JSONObject()
                    .put("latex", formula.latex)
                    .put("rendered", prepared != null)
                    .put("reason", result?.reason)
                    .put("ascent", prepared?.ascent)
                    .put("descent", prepared?.descent)
                    .put("boxHeight", prepared?.height)
                    .put("width", prepared?.width)
                    .put("em", prepared?.em)
                    .put("bodySizePx", bodySizePx)
                    .put("topGapPx", verticalPadding)
                    .put("bottomGapPx", verticalPadding)
                    .put("gapEm", verticalPadding / bodySizePx),
            )
        }
        LayoutInspection.recordBlockMath(
            itemKey,
            notePath,
            blockId,
            JSONObject().put("formulas", records),
        )
    }

    fun recordInlinePreview(
        key: String,
        scene: SceneData,
        collapsed: List<String>,
        pictureKey: String,
        heightPx: Float,
        layoutWidth: Double,
        unitScale: Float,
        drawZoom: Float,
    ) {
        if (!Inspection.recordingEnabled) return
        Inspection.recordInlinePreview(
            key,
            scene.hitRegions.size,
            scene.primitives.count {
                it is ScenePrimitive.Text && it.span.text.trim().startsWith("+")
            },
            collapsed,
            pictureKey,
            heightPx,
            layoutWidth,
            unitScale,
            drawZoom,
            diagramMainFontSize(scene) * drawZoom,
        )
    }

    fun onDiagramComposed(delta: Int) {
        if (delta > 0) {
            val count = Inspection.composedDiagramCanvases.addAndGet(delta)
            if (count > Inspection.maxComposedDiagramCanvases.get()) {
                Inspection.maxComposedDiagramCanvases.set(count)
            }
        } else {
            Inspection.composedDiagramCanvases.addAndGet(delta)
        }
    }

    fun setVisibleDiagramCount(count: Int) {
        Inspection.visibleDiagramCanvases.set(count)
        if (count > Inspection.maxVisibleDiagramCanvases.get()) {
            Inspection.maxVisibleDiagramCanvases.set(count)
        }
    }

    fun markActivityCreated() {
        ReaderTiming.activityCreatedAt = SystemClock.uptimeMillis()
    }

    fun markOpenStarted() {
        ReaderTiming.openStartedAt = SystemClock.uptimeMillis()
        ReaderTiming.firstFrameAt = 0
    }

    fun markContentInstalled() {
        ReaderTiming.contentInstalledAt = SystemClock.uptimeMillis()
    }

    fun markFirstFrame() {
        if (ReaderTiming.firstFrameAt == 0L) {
            ReaderTiming.firstFrameAt = SystemClock.uptimeMillis()
        }
    }

    fun markFullyDrawn() {
        ReaderTiming.fullyDrawnAt = SystemClock.uptimeMillis()
    }

    fun incrementOpenCount() {
        ReaderTiming.openCount += 1
    }

    fun recordAction(action: String) {
        DebugSessionMetrics.lastAction = action
    }

    fun recordOpenTarget(target: ScrollTarget?) {
        DebugSessionMetrics.lastOpenTarget = target
    }

    fun recordLayoutMode(mode: String) {
        DebugSessionMetrics.layoutMode = mode
    }

    fun recordExternalMode(mode: String?) {
        DebugSessionMetrics.externalMode = mode
    }

    fun recordQuerySummary(summary: String, elapsedMs: Long) {
        DebugSessionMetrics.querySummary = summary
        DebugSessionMetrics.lastSearchMicros = elapsedMs
    }

    fun recordSearchMicros(elapsedMs: Long) {
        DebugSessionMetrics.lastSearchMicros = elapsedMs
    }

    fun recordSaveIssue(issue: String?) {
        DebugSessionMetrics.lastSaveIssue = issue
    }

    fun recordCreatedNote(path: String?) {
        DebugSessionMetrics.lastCreatedNote = path
    }

    fun recordShortcutsPublished(titles: List<String>) {
        DebugSessionMetrics.shortcutsPublished = titles
    }

    fun recordShortcutLaunch(note: String?, vault: String?) {
        DebugSessionMetrics.lastShortcutLaunchNote = note
        DebugSessionMetrics.lastShortcutLaunchVault = vault
    }

    fun recordFocusChrome(topInsetPx: Int, topBarBottomPx: Int) {
        FocusChromeMetrics.topInsetPx = topInsetPx
        FocusChromeMetrics.topBarBottomPx = topBarBottomPx
    }

    fun recordFocusScene(scene: SceneData, documentKey: String, ordinal: Int) {
        FocusDebug.measureFocusScene(scene, documentKey, ordinal)
    }

    fun recordFocusState(
        scale: Float,
        offsetX: Float,
        offsetY: Float,
        fitScale: Float,
        unitScale: Float,
        scene: SceneData?,
        documentKey: String,
        notePath: String,
        ordinal: Int,
        collapsed: List<String>,
    ) {
        FocusStateRegistry.lastScale = scale
        FocusStateRegistry.lastOffsetX = offsetX
        FocusStateRegistry.lastOffsetY = offsetY
        FocusStateRegistry.sceneWidth = scene?.width?.toFloat() ?: 0f
        FocusStateRegistry.sceneHeight = scene?.height?.toFloat() ?: 0f
        FocusStateRegistry.fitScale = fitScale
        val base = scene?.primitives
            ?.filterIsInstance<ScenePrimitive.Text>()
            ?.maxOfOrNull { it.span.fontSize.toFloat() } ?: 0f
        FocusStateRegistry.sceneFontSize = base
        FocusStateRegistry.unitScale = unitScale
        FocusStateRegistry.drawnFontSize = base * scale * unitScale
        FocusStateRegistry.sceneKey = "$documentKey#$ordinal"
        FocusStateRegistry.notePath = notePath
        FocusStateRegistry.ordinal = ordinal
        FocusStateRegistry.collapsed = collapsed
        FocusStateRegistry.visibleNodes = scene?.hitRegions?.size ?: 0
        FocusStateRegistry.hits = scene?.hitRegions.orEmpty()
        FocusStateRegistry.plusLabels = scene?.primitives?.count {
            it is ScenePrimitive.Text && it.span.text.trim().startsWith("+")
        } ?: 0
    }

    fun recordFocusCollapsed(collapsed: List<String>) {
        FocusStateRegistry.collapsed = collapsed
    }

    fun recordFocusPointerEvent() {
        FocusStateRegistry.pointerEvents += 1
    }

    fun recordFocusGestureDistance(distance: Float) {
        FocusStateRegistry.gestureDistance = distance
    }

    fun recordFocusTap(sceneX: Float, sceneY: Float, hit: SceneHit?) {
        FocusStateRegistry.lastTapSceneX = sceneX
        FocusStateRegistry.lastTapSceneY = sceneY
        FocusStateRegistry.lastHitLabel = hit?.label ?: ""
        FocusStateRegistry.lastHitNodeId = hit?.nodeId ?: ""
        FocusStateRegistry.lastHitCanCollapse = hit?.canCollapse == true
    }

    fun writeFocusDebug(context: Context) {
        FocusDebug.write(context, FocusDebug.stateJson())
    }
}
