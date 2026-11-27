// SPDX-License-Identifier: AGPL-3.0-only
@file:Suppress("NOTHING_TO_INLINE")
package io.github.groundgrounder.hortus

import android.app.Application
import android.content.Context
import android.text.StaticLayout
import androidx.compose.ui.Modifier
import io.github.groundgrounder.hortus.core.EmbedContentInfo
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MediaBlock
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.SceneHit
import io.github.groundgrounder.hortus.core.model.LoadedImage
import io.github.groundgrounder.hortus.reader.model.ScrollTarget
import io.github.groundgrounder.hortus.reader.render.MathRenderResult
import io.github.groundgrounder.hortus.reader.ui.ReaderTextView

/** Release 与 benchmark 构建的零开销空探针。 */
internal object DebugProbes {
    const val enabled: Boolean = false

    inline fun init(application: Application) = Unit

    inline fun <T> traceMainThread(name: String, block: () -> T): T = block()

    inline fun recordImage(path: String, image: LoadedImage) = Unit

    inline fun recordMath(note: String, formula: MathBlock, result: MathRenderResult, bodySize: Float) = Unit

    inline fun recordDiagram(
        documentKey: String,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        scene: SceneData,
    ) = Unit

    inline fun recordDiagramFailure(key: String, ordinal: Int, reason: String) = Unit

    inline fun recordEmbed(key: String, info: EmbedContentInfo) = Unit

    inline fun recordExport(format: String, name: String, bytes: Int, path: String?) = Unit

    inline fun recordShare(
        action: String,
        mime: String,
        uri: String,
        name: String,
        bytes: Int,
        contentMatches: Boolean,
        contentBytes: Int,
    ) = Unit

    inline fun onDiagramPreviewRecorded(key: String, scene: SceneData, zoom: Float) = Unit

    inline fun onDiagramPreviewReplayed() = Unit

    inline fun Modifier.debugDiagramRect(key: String): Modifier = this

    inline fun Modifier.debugItemBounds(key: String, blockId: Long): Modifier = this

    inline fun Modifier.debugPanel(
        scope: String,
        key: String,
        kind: String,
        label: String,
        target: String?,
        tab: String,
    ): Modifier = this

    inline fun Modifier.debugPaneWidth(isSide: Boolean): Modifier = this

    inline fun clearSidePaneWidth() = Unit

    inline fun Modifier.debugTextHits(enabled: Boolean, noinline onRefresh: () -> Unit): Modifier = this

    inline fun Modifier.debugFocusBackTop(): Modifier = this

    inline fun retainPanels(scope: String, keys: Collection<String>) = Unit

    inline fun clearPanels(scope: String) = Unit

    inline fun clearTextHits(scope: String) = Unit

    inline fun clearAllHits() = Unit

    inline fun refreshTextHits(view: ReaderTextView) = Unit

    inline fun refreshTextLayout(view: ReaderTextView) = Unit

    inline fun recordTableLayout(
        media: MediaBlock,
        notePath: String,
        density: Float,
        layouts: List<StaticLayout>,
        rowHeights: List<Float>,
        mathResults: Map<String, MathRenderResult>,
    ) = Unit

    inline fun recordBlockMath(
        itemKey: String,
        notePath: String,
        blockId: Long,
        bodySizePx: Float,
        verticalPadding: Float,
        formulas: List<MathBlock>,
        results: Map<MathBlock, MathRenderResult>,
    ) = Unit

    inline fun recordInlinePreview(
        key: String,
        scene: SceneData,
        collapsed: List<String>,
        pictureKey: String,
        heightPx: Float,
        layoutWidth: Double,
        unitScale: Float,
        drawZoom: Float,
    ) = Unit

    inline fun onDiagramComposed(delta: Int) = Unit

    inline fun setVisibleDiagramCount(count: Int) = Unit

    inline fun markActivityCreated() = Unit

    inline fun markOpenStarted() = Unit

    inline fun markContentInstalled() = Unit

    inline fun markFirstFrame() = Unit

    inline fun markFullyDrawn() = Unit

    inline fun incrementOpenCount() = Unit

    inline fun recordAction(action: String) = Unit

    inline fun recordOpenTarget(target: ScrollTarget?) = Unit

    inline fun recordLayoutMode(mode: String) = Unit

    inline fun recordExternalMode(mode: String?) = Unit

    inline fun recordQuerySummary(summary: String, elapsedMs: Long) = Unit

    inline fun recordSearchMicros(elapsedMs: Long) = Unit

    inline fun recordSaveIssue(issue: String?) = Unit

    inline fun recordCreatedNote(path: String?) = Unit

    inline fun recordShortcutsPublished(titles: List<String>) = Unit

    inline fun recordShortcutLaunch(note: String?, vault: String?) = Unit

    inline fun recordFocusChrome(topInsetPx: Int, topBarBottomPx: Int) = Unit

    inline fun recordFocusScene(scene: SceneData, documentKey: String, ordinal: Int) = Unit

    inline fun recordFocusState(
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
    ) = Unit

    inline fun recordFocusCollapsed(collapsed: List<String>) = Unit

    inline fun recordFocusPointerEvent() = Unit

    inline fun recordFocusGestureDistance(distance: Float) = Unit

    inline fun recordFocusTap(sceneX: Float, sceneY: Float, hit: SceneHit?) = Unit

    inline fun writeFocusDebug(context: Context) = Unit
}
