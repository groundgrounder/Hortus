// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.ui

import android.graphics.Paint
import android.graphics.Typeface
import android.text.Layout
import android.text.SpannableString
import android.text.Spanned
import android.text.StaticLayout
import android.text.TextDirectionHeuristics
import android.text.TextPaint
import android.text.style.BackgroundColorSpan
import android.text.style.ForegroundColorSpan
import android.text.style.RelativeSizeSpan
import android.text.style.ReplacementSpan
import android.text.style.StrikethroughSpan
import android.text.style.StyleSpan
import android.text.style.TypefaceSpan
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MediaBlock
import io.github.groundgrounder.hortus.core.MediaRun
import io.github.groundgrounder.hortus.core.model.LoadedImage
import io.github.groundgrounder.hortus.core.model.MASK_EMPHASIS
import io.github.groundgrounder.hortus.core.model.MASK_HIGHLIGHT
import io.github.groundgrounder.hortus.core.model.MASK_INLINE_CODE
import io.github.groundgrounder.hortus.core.model.MASK_LINK
import io.github.groundgrounder.hortus.core.model.MASK_QUOTE
import io.github.groundgrounder.hortus.core.model.MASK_STRIKE
import io.github.groundgrounder.hortus.core.model.MASK_STRONG
import io.github.groundgrounder.hortus.reader.model.ItemRenderBridge
import io.github.groundgrounder.hortus.reader.model.ReaderPalette
import io.github.groundgrounder.hortus.reader.model.toArgbCompat
import io.github.groundgrounder.hortus.reader.render.MathRenderResult
import io.github.groundgrounder.hortus.reader.render.PreparedMath
import io.github.groundgrounder.hortus.reader.render.mathInlinePadding
import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min

private class CellMathSpan(
    private val prepared: PreparedMath?,
    private val fallback: String,
    private val errorColor: Int,
) : ReplacementSpan() {
    private val padding = prepared?.let { mathInlinePadding(it.em) } ?: 0f
    private var fallbackPaint: Paint? = null

    override fun getSize(paint: Paint, text: CharSequence, start: Int, end: Int, fm: Paint.FontMetricsInt?): Int {
        val content = prepared?.width ?: paint.measureText(fallback)
        if (prepared != null && fm != null) {
            val ascent = ceil(prepared.ascent).toInt()
            val descent = ceil(prepared.descent).toInt()
            if (fm.ascent > -ascent) fm.ascent = -ascent
            if (fm.top > -ascent) fm.top = -ascent
            if (fm.descent < descent) fm.descent = descent
            if (fm.bottom < descent) fm.bottom = descent
        }
        return ceil(content + padding * 2f).toInt()
    }

    override fun draw(
        canvas: android.graphics.Canvas,
        text: CharSequence,
        start: Int,
        end: Int,
        x: Float,
        top: Int,
        baseline: Int,
        bottom: Int,
        paint: Paint,
    ) {
        if (prepared != null) {
            prepared.draw(canvas, x + padding, baseline - prepared.ascent, paint.color)
        } else {
            val drawPaint = fallbackPaint ?: Paint(paint).also { fallbackPaint = it }
            drawPaint.set(paint)
            drawPaint.color = errorColor
            canvas.drawText(fallback, x, baseline.toFloat(), drawPaint)
        }
    }
}

private data class TableLayout(
    val layouts: List<StaticLayout>,
    val rowHeights: List<Float>,
    val totalHeight: Float,
)

@Composable
internal fun TableBlockView(
    media: MediaBlock,
    contentWidth: Int,
    bodySizePx: Float,
    palette: ReaderPalette,
    renderBridge: ItemRenderBridge,
    notePath: String,
    modifier: Modifier = Modifier,
) {
    val density = LocalDensity.current.density
    val tableFontSize = bodySizePx * 0.94f
    val mathResults = remember(media.blockId, media.displayStart, tableFontSize) {
        mutableStateMapOf<String, MathRenderResult>()
    }
    LaunchedEffect(media.blockId, media.columns, tableFontSize) {
        val formulas = media.cells.flatMap { it.math }
        val pending = formulas.filter { cellMathKey(it) !in mathResults }
        if (pending.isNotEmpty()) {
            val values = renderBridge.prepareMath(pending, tableFontSize)
            pending.forEachIndexed { index, formula ->
                values.getOrNull(index)?.let { mathResults[cellMathKey(formula)] = it }
            }
        }
    }
    val prepared = remember(media, contentWidth, palette, tableFontSize, mathResults.size, density) {
        buildTable(media, tableFontSize, palette, density, mathResults)
    }
    val failures = media.cells.flatMap { cell ->
        cell.math.mapNotNull { formula ->
            val result = mathResults[cellMathKey(formula)]
            if (result != null && result.prepared == null && result.reason != null) formula to result.reason else null
        }
    }
    if (DebugProbes.enabled) {
        LaunchedEffect(prepared, media, notePath, density) {
            DebugProbes.recordTableLayout(
                media = media,
                notePath = notePath,
                density = density,
                layouts = prepared.layouts,
                rowHeights = prepared.rowHeights,
                mathResults = mathResults,
            )
        }
    }
    Column(modifier.fillMaxWidth()) {
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(with(LocalDensity.current) { prepared.totalHeight.toDp() }),
        ) {
            var y = 0f
            val columns = media.columns.map { it.toFloat() }
            val headerRows = 1
            var row = 0
            for (heightPx in prepared.rowHeights) {
                if (row < headerRows) {
                    drawRect(
                        color = palette.codeBackground.copy(alpha = 0.55f),
                        topLeft = Offset(0f, y),
                        size = Size(this.size.width, heightPx),
                    )
                }
                var x = 0f
                for (column in columns.indices) {
                    val index = row * columns.size + column
                    val layout = prepared.layouts.getOrNull(index) ?: continue
                    drawIntoCanvas { canvas ->
                        canvas.nativeCanvas.save()
                        val alignment = media.alignments.getOrNull(column)?.toInt() ?: 0
                        val available = max(0f, columns[column] - 20f * density)
                        val offset = when (alignment) {
                            1 -> (available - layout.width) / 2f
                            2 -> available - layout.width
                            else -> 0f
                        }
                        canvas.nativeCanvas.translate(x + 10f * density + offset, y + 8f * density)
                        layout.draw(canvas.nativeCanvas)
                        canvas.nativeCanvas.restore()
                    }
                    x += columns[column]
                }
                y += heightPx
                drawRect(
                    color = palette.rule,
                    topLeft = Offset(0f, y - max(0.5f, density / 2)),
                    size = Size(this.size.width, max(0.5f, density / 2)),
                )
                row += 1
            }
        }
        for ((formula, reason) in failures) {
            Text(
                "${formula.displaySource}: $reason",
                style = MaterialTheme.typography.labelSmall,
                color = palette.error,
            )
        }
    }
}

private fun cellMathKey(formula: MathBlock): String = "${formula.displayStart}:${formula.latex}:${formula.style}"

private fun buildTable(
    media: MediaBlock,
    size: Float,
    palette: ReaderPalette,
    density: Float,
    math: Map<String, MathRenderResult>,
): TableLayout {
    val columns = media.columns.map { it.toFloat() }
    val rows = max(1, media.rows.toInt())
    val layouts = ArrayList<StaticLayout>(columns.size * rows)
    val rowHeights = FloatArray(rows)
    for (row in 0 until rows) {
        for (column in columns.indices) {
            val cell = media.cells.getOrNull(row * columns.size + column)
            val paint = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
                textSize = size
                color = palette.textArgb
                typeface = if (row == 0) Typeface.DEFAULT_BOLD else Typeface.DEFAULT
            }
            val text = cell?.text ?: ""
            val spannable = SpannableString(text)
            if (row == 0) {
                spannable.setSpan(StyleSpan(Typeface.BOLD), 0, text.length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            }
            for (run in cell?.runs.orEmpty()) {
                applyCellRun(spannable, run, text.length, palette)
            }
            for (formula in cell?.math.orEmpty()) {
                val local = formula.displayStart.toInt()
                if (local !in 0 until text.length) continue
                val result = math[cellMathKey(formula)]
                val fallback = formula.displaySource.ifBlank { formula.latex }
                spannable.setSpan(
                    CellMathSpan(result?.prepared, fallback, palette.errorArgb),
                    local,
                    min(local + 1, text.length),
                    Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
                )
            }
            val width = max(1, (columns[column] - 20f * density).toInt())
            val layout = StaticLayout.Builder
                .obtain(spannable, 0, spannable.length, paint, width)
                .setAlignment(Layout.Alignment.ALIGN_NORMAL)
                .setIncludePad(true)
                .setBreakStrategy(android.graphics.text.LineBreaker.BREAK_STRATEGY_HIGH_QUALITY)
                .setTextDirection(TextDirectionHeuristics.FIRSTSTRONG_LTR)
                .setLineSpacing(0f, 1.22f)
                .build()
            layouts += layout
            rowHeights[row] = max(rowHeights[row], layout.height.toFloat() + 16f * density)
        }
    }
    return TableLayout(layouts, rowHeights.toList(), rowHeights.sum())
}

private fun applyCellRun(
    spannable: SpannableString,
    run: MediaRun,
    length: Int,
    palette: ReaderPalette,
) {
    val start = run.start.toInt().coerceIn(0, length)
    val end = (run.start + run.length).toInt().coerceIn(0, length)
    if (end <= start) return
    val mask = run.mask.toInt()
    if (mask and MASK_STRONG != 0) {
        spannable.setSpan(StyleSpan(Typeface.BOLD), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
    }
    if (mask and MASK_EMPHASIS != 0) {
        spannable.setSpan(StyleSpan(Typeface.ITALIC), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
    }
    if (mask and MASK_STRIKE != 0) {
        spannable.setSpan(StrikethroughSpan(), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
    }
    if (mask and MASK_INLINE_CODE != 0) {
        spannable.setSpan(TypefaceSpan("monospace"), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        spannable.setSpan(RelativeSizeSpan(0.92f), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        spannable.setSpan(
            BackgroundColorSpan(palette.codeBackground.toArgbCompat()),
            start,
            end,
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    if (mask and MASK_LINK != 0) {
        spannable.setSpan(
            ForegroundColorSpan(palette.accentArgb),
            start,
            end,
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    if (mask and MASK_HIGHLIGHT != 0) {
        spannable.setSpan(
            BackgroundColorSpan(palette.highlight.toArgbCompat()),
            start,
            end,
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    if (mask and MASK_QUOTE != 0) {
        spannable.setSpan(
            ForegroundColorSpan(palette.mutedArgb),
            start,
            end,
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
}

@Composable
internal fun ImageBlockView(
    media: MediaBlock,
    contentWidth: Int,
    palette: ReaderPalette,
    renderBridge: ItemRenderBridge,
    modifier: Modifier = Modifier,
) {
    var image by remember(media.blockId, media.imagePath, contentWidth) {
        mutableStateOf<LoadedImage?>(null)
    }
    LaunchedEffect(media.blockId, media.imagePath, contentWidth, media.imageWidth) {
        image = renderBridge.loadImage(
            media.imagePath,
            contentWidth,
            media.imageWidth?.toInt(),
        )
    }
    val loaded = image
    val maximum = min(contentWidth.toFloat(), (media.imageWidth?.toFloat() ?: contentWidth.toFloat()))
    if (loaded?.bitmap != null) {
        val width = min(maximum, loaded.widthPx.toFloat()).toInt().coerceAtLeast(1)
        val height = (loaded.heightPx.toFloat() * width / loaded.widthPx).toInt().coerceAtLeast(1)
        Image(
            bitmap = loaded.bitmap.asImageBitmap(),
            contentDescription = media.imageAlt.ifBlank { null },
            contentScale = ContentScale.FillWidth,
            modifier = modifier
                .width(with(LocalDensity.current) { width.toDp() })
                .height(with(LocalDensity.current) { height.toDp() })
                .padding(vertical = 4.dp),
        )
    } else {
        val reason = if (loaded == null) "" else loaded.reason.ifBlank { media.imageReason }
        Column(
            modifier
                .fillMaxWidth()
                .background(palette.codeBackground, RoundedCornerShape(8.dp))
                .padding(12.dp),
        ) {
            Text(
                media.imageAlt.ifBlank { media.imageTarget }.ifBlank { "image" },
                style = MaterialTheme.typography.bodyMedium,
                color = palette.muted,
                maxLines = 3,
                overflow = TextOverflow.Ellipsis,
            )
            if (reason.isNotEmpty()) {
                Text(
                    reason,
                    style = MaterialTheme.typography.labelSmall,
                    color = palette.error,
                )
            }
        }
    }
}

@Composable
internal fun MathBlockView(
    formulas: List<MathBlock>,
    itemKey: String,
    blockId: Long,
    notePath: String,
    bodySizePx: Float,
    palette: ReaderPalette,
    renderBridge: ItemRenderBridge,
    modifier: Modifier = Modifier,
) {
    val verticalPadding = bodySizePx * 0.5f
    val results = remember(formulas, bodySizePx) { mutableStateMapOf<MathBlock, MathRenderResult>() }
    LaunchedEffect(formulas, bodySizePx, results.size, notePath, itemKey) {
        if (results.size < formulas.size) {
            val values = renderBridge.prepareMath(formulas, bodySizePx)
            formulas.forEachIndexed { index, formula ->
                values.getOrNull(index)?.let { results[formula] = it }
            }
        }
        if (DebugProbes.enabled) {
            DebugProbes.recordBlockMath(
                itemKey = itemKey,
                notePath = notePath,
                blockId = blockId,
                bodySizePx = bodySizePx,
                verticalPadding = verticalPadding,
                formulas = formulas,
                results = results,
            )
        }
    }
    Column(modifier.fillMaxWidth()) {
        for (formula in formulas) {
            val result = results[formula]
            val prepared = result?.prepared
            if (prepared != null) {
                val blockHeight = verticalPadding * 2f + prepared.height
                Canvas(
                    Modifier
                        .fillMaxWidth()
                        .height(with(LocalDensity.current) { blockHeight.toDp() }),
                ) {
                    val x = max(0f, (size.width - prepared.width) / 2f)
                    prepared.draw(
                        drawContext.canvas.nativeCanvas,
                        x,
                        verticalPadding,
                        palette.textArgb,
                    )
                }
            } else if (result != null && result.reason != null) {
                Column(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = with(LocalDensity.current) { verticalPadding.toDp() })
                        .background(palette.codeBackground, RoundedCornerShape(6.dp))
                        .padding(10.dp),
                ) {
                    Text(
                        formula.displaySource.ifBlank { formula.latex },
                        style = MaterialTheme.typography.bodyMedium,
                        color = palette.error,
                    )
                    Text(
                        result.reason,
                        style = MaterialTheme.typography.labelSmall,
                        color = palette.error,
                    )
                }
            } else {
                Text(
                    formula.displaySource.ifBlank { formula.latex },
                    style = MaterialTheme.typography.bodyMedium,
                    color = palette.muted,
                    modifier = Modifier.padding(vertical = with(LocalDensity.current) { verticalPadding.toDp() }),
                )
            }
        }
    }
}
