// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.graphics.Bitmap
import android.graphics.Canvas as AndroidCanvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.text.Layout
import android.text.SpannableString
import android.text.Spanned
import android.text.StaticLayout
import android.text.TextDirectionHeuristics
import android.text.TextPaint
import android.text.style.AbsoluteSizeSpan
import android.text.style.BackgroundColorSpan
import android.text.style.ForegroundColorSpan
import android.text.style.RelativeSizeSpan
import android.text.style.ReplacementSpan
import android.text.style.StrikethroughSpan
import android.text.style.StyleSpan
import android.text.style.TypefaceSpan
import android.text.style.UnderlineSpan
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MediaBlock
import kotlin.math.max
import kotlin.math.min

internal data class HitArea(val rect: RectF, val link: TextLink?)

internal class PreparedText(
    val layout: StaticLayout,
    val widthPx: Int,
    val heightPx: Int,
    val textOffsetX: Float,
    val textOffsetY: Float,
    val hits: List<HitArea>,
    val rule: Boolean,
)

private class MathReplacementSpan(
    private val prepared: PreparedMath?,
    private val fallback: String,
    private val errorColor: Int,
    private val bodyColor: Int,
) : ReplacementSpan() {
    override fun getSize(
        paint: Paint,
        text: CharSequence,
        start: Int,
        end: Int,
        fm: Paint.FontMetricsInt?,
    ): Int = (prepared?.width ?: paint.measureText(fallback)).toInt()

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
            prepared.draw(canvas, x, baseline.toFloat(), if (paint.color == 0) bodyColor else paint.color)
        } else {
            val fallbackPaint = Paint(paint).apply {
                color = errorColor
                textSize = paint.textSize
            }
            canvas.drawText(fallback, x, baseline.toFloat(), fallbackPaint)
        }
    }
}

private class ImageReplacementSpan(
    private val image: LoadedImage?,
    private val fallback: String,
    private val color: Int,
) : ReplacementSpan() {
    override fun getSize(
        paint: Paint,
        text: CharSequence,
        start: Int,
        end: Int,
        fm: Paint.FontMetricsInt?,
    ): Int = (image?.widthPx ?: paint.measureText(fallback)).toInt()

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
            canvas.drawBitmap(
                bitmap,
                null,
                RectF(x, baseline - image.heightPx.toFloat(), x + image.widthPx, baseline.toFloat()),
                Paint(Paint.FILTER_BITMAP_FLAG),
            )
        } else {
            val fallbackPaint = Paint(paint).apply { color = color }
            canvas.drawText(fallback, x, baseline.toFloat(), fallbackPaint)
        }
    }
}

private fun headingScale(level: Int): Float = when (level) {
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

@Composable
internal fun TextBlockView(
    item: TextItem,
    contentWidth: Int,
    bodySizePx: Float,
    palette: ReaderPalette,
    repository: ReaderRepository,
    onLink: (TextLink) -> Unit,
    modifier: Modifier = Modifier,
) {
    val density = LocalDensity.current.density
    val blockScale = headingScale(item.headingLevel)
    val blockSize = bodySizePx * blockScale
    val mathResults = remember(item.key) { mutableStateMapOf<String, MathRenderResult>() }
    val images = remember(item.key) { mutableStateMapOf<String, LoadedImage>() }
    LaunchedEffect(item.key, item.text, item.inlineMath, blockSize) {
        val pending = item.inlineMath.filter { mathKey(it) !in mathResults }
        if (pending.isNotEmpty()) {
            val values = withContext(Dispatchers.IO) { repository.prepareMath(pending, blockSize) }
            pending.forEachIndexed { index, formula ->
                values.getOrNull(index)?.let { mathResults[mathKey(formula)] = it }
            }
        }
        val imageTarget = max(1, contentWidth - (24f * density).toInt())
        for (media in item.inlineImages) {
            val key = mediaKey(media)
            if (key in images) continue
            images[key] = ImageLoader.load(
                media.imagePath,
                imageTarget,
                media.imageWidth?.toInt(),
            )
        }
    }
    val layoutKey = TextLayoutKey(
        item.key,
        item.text,
        contentWidth,
        blockSize,
        palette.hashCode(),
        density,
    )
    val useCache = item.inlineMath.isEmpty() && item.inlineImages.isEmpty()
    val prepared = (if (useCache) TextLayoutCache.get(layoutKey) else null) ?: remember(
        item,
        contentWidth,
        palette,
        blockSize,
        mathResults.size,
        images.size,
        density,
    ) {
        runCatching {
            prepareText(
                item = item.copy(text = item.text.replace('\uE000', '\u200B')),
                contentWidth = contentWidth,
                blockSize = blockSize,
                palette = palette,
                density = density,
                math = item.inlineMath.associateBy { mathKey(it) }.mapValues { mathResults[mathKey(it.value)] },
                images = item.inlineImages.associateBy { mediaKey(it) }.mapValues { images[mediaKey(it.value)] },
            )
        }.getOrNull()?.also { if (useCache) TextLayoutCache.put(layoutKey, it) }
    }
    val current = prepared
    if (current == null) {
        Box(modifier.fillMaxWidth().height((48 * density).dp))
        return
    }
    var origin by remember(item.key) { mutableStateOf(Offset.Zero) }
    LaunchedEffect(current.hits, origin, item.key) {
        HitRegistry.replace(
            "text:${item.key}",
            current.hits.mapNotNull { hit ->
                hit.link?.let { link ->
                    ScreenHit(
                        kind = "link",
                        label = link.alias ?: link.target,
                        x = origin.x + hit.rect.left,
                        y = origin.y + hit.rect.top,
                        width = hit.rect.width(),
                        height = hit.rect.height(),
                        target = link.resolvedPath ?: link.target,
                        heading = link.heading,
                        canCollapse = false,
                        nodeId = null,
                    )
                }
            },
        )
    }
    DisposableEffect(item.key) {
        onDispose { HitRegistry.replace("text:${item.key}", emptyList()) }
    }
    val failures = item.inlineMath.mapNotNull { formula ->
        val result = mathResults[mathKey(formula)]
        if (result != null && result.prepared == null && result.reason != null) formula to result.reason else null
    }
    Column(modifier = modifier.fillMaxWidth()) {
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(with(LocalDensity.current) { current.heightPx.toDp() })
                .onGloballyPositioned { origin = it.positionInWindow() }
                .pointerInput(current.hits, item.key) {
                    detectTapGestures { point ->
                        val hit = current.hits.firstOrNull { it.rect.contains(point.x, point.y) }
                        if (hit?.link != null) onLink(hit.link)
                    }
                },
        ) {
            val top = current.textOffsetY
            if (item.code) {
                drawRoundRect(
                    color = palette.codeBackground,
                    topLeft = Offset(0f, top),
                    size = Size(size.width, size.height - top),
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(8f * density, 8f * density),
                )
            }
            if (item.callout != null) {
                val accent = calloutAccent(item.callout.kind, palette)
                drawRoundRect(
                    color = calloutBackground(item.callout.kind, palette),
                    topLeft = Offset(0f, top),
                    size = Size(size.width, size.height - top),
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(8f * density, 8f * density),
                )
                drawRect(
                    color = accent,
                    topLeft = Offset(0f, top),
                    size = Size(3f * density, size.height - top),
                )
            } else if (item.quote) {
                drawRect(
                    color = palette.rule,
                    topLeft = Offset(0f, top),
                    size = Size(3f * density, size.height - top),
                )
            }
            if (item.rule) {
                drawRect(
                    color = palette.rule,
                    topLeft = Offset(0f, size.height / 2),
                    size = Size(size.width, max(1f, density)),
                )
            }
            drawIntoCanvas { canvas ->
                val native = canvas.nativeCanvas
                native.save()
                native.translate(current.textOffsetX, current.textOffsetY)
                current.layout.draw(native)
                native.restore()
            }
        }
        for ((formula, reason) in failures) {
            androidx.compose.material3.Text(
                "${formula.displaySource}: $reason",
                style = androidx.compose.material3.MaterialTheme.typography.labelSmall,
                color = palette.error,
                modifier = Modifier.padding(start = 8.dp),
            )
        }
    }
}

private fun mathKey(formula: MathBlock): String = "${formula.displayStart}:${formula.displayEnd()}:${formula.style}"
private fun MathBlock.displayEnd(): ULong = displayStart + displayLength
private fun mediaKey(media: MediaBlock): String = "${media.displayStart}:${media.displayLength}"

internal fun prepareText(
    item: TextItem,
    contentWidth: Int,
    blockSize: Float,
    palette: ReaderPalette,
    density: Float,
    math: Map<String, MathRenderResult?>,
    images: Map<String, LoadedImage?>,
): PreparedText {
    val codePadding = if (item.code) 10f * density else 0f
    val quotePadding = if (item.callout != null || item.quote) 14f * density else 0f
    val width = max(1, (contentWidth - codePadding * 2 - quotePadding).toInt())
    val bodyBaseSize = if (item.headingLevel > 0) blockSize / headingScale(item.headingLevel) else blockSize
    val paint = TextPaint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG).apply {
        textSize = blockSize
        color = when {
            item.code -> palette.textArgb
            item.quote -> palette.mutedArgb
            item.headingLevel > 0 -> palette.textArgb
            else -> palette.textArgb
        }
        typeface = if (item.code) Typeface.MONOSPACE else Typeface.DEFAULT
        isLinearText = true
        hinting = Paint.HINTING_OFF
    }
    val spannable = SpannableString(item.text)
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
        val start = if (item.fenced) min(item.text.length, item.text.indexOf('\n').let { if (it < 0) item.text.length else it } + 1) else 0
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
        val result = math[mathKey(formula)]
        val prepared = result?.prepared
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
        val fallback = media.imageReason.ifBlank { media.imageAlt.ifBlank { "[image]" } }
        spannable.setSpan(
            ImageReplacementSpan(image, fallback, palette.errorArgb),
            local,
            min(local + 1, item.text.length),
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    val builder = StaticLayout.Builder
        .obtain(spannable, 0, spannable.length, paint, width)
        .setAlignment(Layout.Alignment.ALIGN_NORMAL)
        .setIncludePad(true)
        .setBreakStrategy(android.graphics.text.LineBreaker.BREAK_STRATEGY_HIGH_QUALITY)
        .setHyphenationFrequency(Layout.HYPHENATION_FREQUENCY_NONE)
        .setTextDirection(TextDirectionHeuristics.FIRSTSTRONG_LTR)
        .setLineSpacing(0f, if (item.code) 1.18f else 1.34f)
    val layout = builder.build()
    val padTop = textBlockVerticalPadding(item, density).first
    val padBottom = textBlockVerticalPadding(item, density).second
    val offsetX = codePadding + quotePadding
    val hits = ArrayList<HitArea>()
    for (link in item.links) {
        hits += layoutRects(layout, link.start, link.end).map { rect ->
            HitArea(RectF(rect.left + offsetX, rect.top + padTop, rect.right + offsetX, rect.bottom + padTop), link)
        }
    }
    val height = (layout.height + padTop + padBottom).toInt()
    return PreparedText(
        layout = layout,
        widthPx = width,
        heightPx = height,
        textOffsetX = offsetX,
        textOffsetY = padTop,
        hits = hits,
        rule = item.rule,
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

private fun Color.toArgbCompat(): Int = android.graphics.Color.argb(
    (alpha * 255).toInt().coerceIn(0, 255),
    (red * 255).toInt().coerceIn(0, 255),
    (green * 255).toInt().coerceIn(0, 255),
    (blue * 255).toInt().coerceIn(0, 255),
)
