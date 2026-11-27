// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.ui

import android.content.Context
import android.graphics.Canvas as AndroidCanvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.text.SpannableString
import android.text.Spanned
import android.text.style.AbsoluteSizeSpan
import android.text.style.BackgroundColorSpan
import android.text.style.ForegroundColorSpan
import android.text.style.RelativeSizeSpan
import android.text.style.ReplacementSpan
import android.text.style.StrikethroughSpan
import android.text.style.StyleSpan
import android.text.style.TypefaceSpan
import android.text.style.UnderlineSpan
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MediaBlock
import io.github.groundgrounder.hortus.core.model.LoadedImage
import io.github.groundgrounder.hortus.core.model.MASK_CODE_BLOCK
import io.github.groundgrounder.hortus.core.model.MASK_EMPHASIS
import io.github.groundgrounder.hortus.core.model.MASK_FENCED
import io.github.groundgrounder.hortus.core.model.MASK_FOOTNOTE
import io.github.groundgrounder.hortus.core.model.MASK_FRONTMATTER
import io.github.groundgrounder.hortus.core.model.MASK_HIGHLIGHT
import io.github.groundgrounder.hortus.core.model.MASK_INLINE_CODE
import io.github.groundgrounder.hortus.core.model.MASK_QUOTE
import io.github.groundgrounder.hortus.core.model.MASK_STRIKE
import io.github.groundgrounder.hortus.core.model.MASK_STRONG
import io.github.groundgrounder.hortus.core.model.MASK_TAG
import io.github.groundgrounder.hortus.core.model.ReadingRun
import io.github.groundgrounder.hortus.core.model.TextItem
import io.github.groundgrounder.hortus.reader.model.ReaderPalette
import io.github.groundgrounder.hortus.reader.model.calloutAccent
import io.github.groundgrounder.hortus.reader.model.toArgbCompat
import io.github.groundgrounder.hortus.reader.render.PreparedMath
import io.github.groundgrounder.hortus.reader.render.mathInlinePadding
import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min

private class MathReplacementSpan(
    private val prepared: PreparedMath?,
    private val fallback: String,
    private val errorColor: Int,
    private val bodyColor: Int,
) : ReplacementSpan() {
    private val padding = prepared?.let { mathInlinePadding(it.em) } ?: 0f
    private var fallbackPaint: Paint? = null

    override fun getSize(
        paint: Paint,
        text: CharSequence,
        start: Int,
        end: Int,
        fm: Paint.FontMetricsInt?,
    ): Int {
        val content = prepared?.width ?: paint.measureText(fallback)
        if (prepared != null && fm != null) {
            // 让行高容纳公式真实的上升与下降；小公式不额外撑高。
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
        canvas: AndroidCanvas,
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
            prepared.draw(
                canvas,
                x + padding,
                baseline - prepared.ascent,
                if (paint.color == 0) bodyColor else paint.color,
            )
        } else {
            val drawPaint = fallbackPaint ?: Paint(paint).also { fallbackPaint = it }
            drawPaint.set(paint)
            drawPaint.color = errorColor
            drawPaint.textSize = paint.textSize
            canvas.drawText(fallback, x, baseline.toFloat(), drawPaint)
        }
    }
}

private class ImageReplacementSpan(
    private val image: LoadedImage?,
    private val fallback: String,
    private val color: Int,
) : ReplacementSpan() {
    private val dstRect = RectF()
    private val bitmapPaint = Paint(Paint.FILTER_BITMAP_FLAG)
    private var fallbackPaint: Paint? = null

    override fun getSize(
        paint: Paint,
        text: CharSequence,
        start: Int,
        end: Int,
        fm: Paint.FontMetricsInt?,
    ): Int {
        val bitmap = image?.bitmap
        if (bitmap != null && fm != null) {
            val height = image.heightPx
            if (fm.ascent > -height) fm.ascent = -height
            if (fm.top > -height) fm.top = -height
            if (fm.descent < 0) fm.descent = 0
            if (fm.bottom < 0) fm.bottom = 0
        }
        return (image?.widthPx ?: paint.measureText(fallback)).toInt()
    }

    override fun draw(
        canvas: AndroidCanvas,
        text: CharSequence,
        start: Int,
        end: Int,
        x: Float,
        top: Int,
        baseline: Int,
        bottom: Int,
        paint: Paint,
    ) {
        val bitmap = image?.bitmap
        if (bitmap != null) {
            dstRect.set(x, baseline - image.heightPx.toFloat(), x + image.widthPx, baseline.toFloat())
            canvas.drawBitmap(bitmap, null, dstRect, bitmapPaint)
        } else {
            val drawPaint = fallbackPaint ?: Paint(paint).also { fallbackPaint = it }
            drawPaint.set(paint)
            drawPaint.color = color
            canvas.drawText(fallback, x, baseline.toFloat(), drawPaint)
        }
    }
}

internal fun headingScale(level: Int): Float = when (level) {
    1 -> 1.6f
    2 -> 1.35f
    3 -> 1.15f
    else -> 1f
}

private fun maskAt(run: ReadingRun): Int = run.mask

internal fun textBlockVerticalPadding(item: TextItem, density: Float): Pair<Float, Float> {
    val top = when {
        item.headingLevel > 0 -> 14f * density
        item.code -> 10f * density
        item.callout != null -> 10f * density
        item.quote -> 6f * density
        else -> 2f * density
    }
    val bottom = when {
        item.headingLevel > 0 -> 6f * density
        item.code -> 12f * density
        item.callout != null -> 10f * density
        item.quote -> 6f * density
        else -> 8f * density
    }
    return top to bottom
}

internal fun textBlockWidths(item: TextItem, contentWidth: Int, density: Float): Triple<Float, Int, Int> {
    val codePadding = if (item.code) 10f * density else 0f
    val quotePadding = if (item.callout != null || item.quote) 14f * density else 0f
    val textWidth = max(1, (contentWidth - codePadding * 2 - quotePadding).toInt())
    return Triple(codePadding + quotePadding, codePadding.toInt(), textWidth)
}

internal fun mathKey(formula: MathBlock): String =
    "${formula.displayStart}:${formula.displayStart + formula.displayLength}:${formula.style}"

internal fun mediaKey(media: MediaBlock): String = "${media.displayStart}:${media.displayLength}"

internal fun buildTextSpannable(
    item: TextItem,
    blockSize: Float,
    palette: ReaderPalette,
    math: Map<String, PreparedMath?>,
    images: Map<String, LoadedImage?>,
    context: Context? = null,
): SpannableString {
    val bodyBaseSize = if (item.headingLevel > 0) blockSize / headingScale(item.headingLevel) else blockSize
    val spannable = SpannableString(item.text.replace('\uE000', '\u200B'))
    if (item.headingLevel > 0) {
        spannable.setSpan(
            StyleSpan(Typeface.BOLD),
            0,
            item.text.length,
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    if (item.callout != null) {
        val firstLineEnd = item.text.indexOf('\n').let { if (it < 0) item.text.length else it }
        if (firstLineEnd > 0) {
            spannable.setSpan(
                StyleSpan(Typeface.BOLD),
                0,
                firstLineEnd,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
            spannable.setSpan(
                ForegroundColorSpan(calloutAccent(item.callout.kind, palette).toArgbCompat()),
                0,
                firstLineEnd,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
    }
    if (item.fenced || item.code) {
        val start = if (item.fenced) {
            min(item.text.length, item.text.indexOf('\n').let { if (it < 0) item.text.length else it } + 1)
        } else {
            0
        }
        if (start < item.text.length) {
            spannable.setSpan(TypefaceSpan("monospace"), start, item.text.length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            spannable.setSpan(
                RelativeSizeSpan(0.94f),
                start,
                item.text.length,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
    }
    for (run in item.runs) {
        val start = max(run.displayStart, item.displayStart) - item.displayStart
        val end = min(run.displayEnd, item.displayStart + item.text.length) - item.displayStart
        if (end <= start) continue
        val mask = maskAt(run)
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
            spannable.setSpan(
                RelativeSizeSpan(0.93f),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
            spannable.setSpan(
                BackgroundColorSpan(palette.codeBackground.copy(alpha = 0.9f).toArgbCompat()),
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
        if (mask and MASK_HIGHLIGHT != 0) {
            spannable.setSpan(
                BackgroundColorSpan(palette.highlight.toArgbCompat()),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
        if (mask and MASK_TAG != 0) {
            spannable.setSpan(
                ForegroundColorSpan(palette.accentArgb),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
            spannable.setSpan(
                BackgroundColorSpan(palette.tagBackground.toArgbCompat()),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
        if (mask and MASK_FOOTNOTE != 0) {
            spannable.setSpan(
                ForegroundColorSpan(palette.accentArgb),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
            spannable.setSpan(
                RelativeSizeSpan(0.85f),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
        if (mask and MASK_FRONTMATTER != 0) {
            spannable.setSpan(TypefaceSpan("monospace"), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            spannable.setSpan(
                ForegroundColorSpan(palette.mutedArgb),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
        val heading = (mask ushr 24) and 7
        if (heading > 0 && heading != item.headingLevel) {
            spannable.setSpan(
                AbsoluteSizeSpan((bodyBaseSize * headingScale(heading)).toInt(), false),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
    }
    for (link in item.links) {
        val color = if (link.resolution == "note" || link.resolution == "attachment") {
            palette.accentArgb
        } else {
            palette.mutedArgb
        }
        spannable.setSpan(
            ForegroundColorSpan(color),
            link.start.coerceIn(0, item.text.length),
            link.end.coerceIn(0, item.text.length),
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
        if (link.resolution == "note" || link.resolution == "attachment") {
            spannable.setSpan(
                UnderlineSpan(),
                link.start.coerceIn(0, item.text.length),
                link.end.coerceIn(0, item.text.length),
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
    }
    for (tag in item.tags) {
        spannable.setSpan(
            ForegroundColorSpan(palette.accentArgb),
            tag.start.coerceIn(0, item.text.length),
            tag.end.coerceIn(0, item.text.length),
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    if (item.taskLine) {
        val first = item.text.indexOfFirst { !it.isWhitespace() }
        if (first >= 0 && first + 1 <= item.text.length) {
            spannable.setSpan(
                ForegroundColorSpan(palette.accentArgb),
                first,
                min(first + 1, item.text.length),
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
    }
    for (formula in item.inlineMath) {
        val local = (formula.displayStart.toLong() - item.displayStart).toInt()
        if (local !in 0 until item.text.length) continue
        val prepared = math[mathKey(formula)]
        val fallback = formula.displaySource.ifBlank { "\$${formula.latex}\$" }.ifBlank { "∅" }
        spannable.setSpan(
            MathReplacementSpan(prepared, fallback, palette.errorArgb, palette.textArgb),
            local,
            min(local + 1, item.text.length),
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    for (media in item.inlineImages) {
        val local = (media.displayStart.toLong() - item.displayStart).toInt()
        if (local !in 0 until item.text.length) continue
        val image = images[mediaKey(media)]
        val rawReason = image?.reason?.takeIf { it.isNotBlank() } ?: media.imageReason
        val localizedReason = if (rawReason.isNotBlank() && context != null) {
            localizedRenderError(context, rawReason)
        } else {
            rawReason
        }
        val fallback = localizedReason.ifBlank { media.imageAlt.ifBlank { "[image]" } }
        spannable.setSpan(
            ImageReplacementSpan(image, fallback, palette.errorArgb),
            local,
            min(local + 1, item.text.length),
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    return spannable
}
