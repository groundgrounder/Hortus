// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas as AndroidCanvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.text.Layout
import android.text.SpannableString
import android.text.Spanned
import android.text.TextPaint
import android.text.TextUtils
import android.text.style.AbsoluteSizeSpan
import android.text.style.BackgroundColorSpan
import android.text.style.ForegroundColorSpan
import android.text.style.RelativeSizeSpan
import android.text.style.ReplacementSpan
import android.text.style.StrikethroughSpan
import android.text.style.StyleSpan
import android.text.style.TypefaceSpan
import android.text.style.UnderlineSpan
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.ViewGroup
import android.widget.TextView
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MediaBlock
import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min

internal fun mathInlinePadding(em: Float): Float = max(1f, em * 0.08f)

private class MathReplacementSpan(
    private val prepared: PreparedMath?,
    private val fallback: String,
    private val errorColor: Int,
    private val bodyColor: Int,
) : ReplacementSpan() {
    private val padding = prepared?.let { mathInlinePadding(it.em) } ?: 0f

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

internal fun textBlockWidths(item: TextItem, contentWidth: Int, density: Float): Triple<Float, Int, Int> {
    val codePadding = if (item.code) 10f * density else 0f
    val quotePadding = if (item.callout != null || item.quote) 14f * density else 0f
    val textWidth = max(1, (contentWidth - codePadding * 2 - quotePadding).toInt())
    return Triple(codePadding + quotePadding, codePadding.toInt(), textWidth)
}

/**
 * 原生 TextView 承载阅读文本：系统负责选择手柄、复制工具栏和文本绘制。
 * 只覆盖触摸以保留双链单击，并且不叠加任何自绘控件。
 */
internal class ReaderTextView(context: Context) : TextView(context) {
    var itemKey: String = ""
    var blockId: Long = 0
    var notePath: String = ""
    var onLink: ((TextLink) -> Unit)? = null
    var onLayoutReady: (() -> Unit)? = null

    private var item: TextItem? = null
    private var palette: ReaderPalette? = null
    private var mathByOffset: Map<Int, PreparedMath?> = emptyMap()
    private var mathLatex: Map<Int, String> = emptyMap()
    private var links: List<TextLink> = emptyList()
    private var appliedSpannable: SpannableString? = null
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop
    private var downX = 0f
    private var downY = 0f
    private var moved = false

    init {
        includeFontPadding = false
        setTextIsSelectable(true)
        isVerticalScrollBarEnabled = false
        isHorizontalScrollBarEnabled = false
        setLineSpacing(0f, 1.34f)
        breakStrategy = android.graphics.text.LineBreaker.BREAK_STRATEGY_HIGH_QUALITY
        hyphenationFrequency = Layout.HYPHENATION_FREQUENCY_NONE
        gravity = Gravity.START or Gravity.TOP
        paint.isLinearText = true
        paint.isSubpixelText = true
        setBackgroundColor(android.graphics.Color.TRANSPARENT)
    }

    fun configure(
        item: TextItem,
        notePath: String,
        blockSize: Float,
        contentWidth: Int,
        palette: ReaderPalette,
        density: Float,
        spannable: SpannableString,
        math: Map<Int, PreparedMath?>,
        latex: Map<Int, String>,
        onLink: (TextLink) -> Unit,
    ) {
        this.item = item
        this.itemKey = item.key
        this.blockId = item.blockId
        this.notePath = notePath
        this.palette = palette
        this.mathByOffset = math
        this.mathLatex = latex
        this.links = item.links
        if (item.links.isEmpty()) HitRegistry.replace("text:${item.key}", emptyList())
        this.onLink = onLink
        val (left, right, _) = textBlockWidths(item, contentWidth.coerceAtLeast(1), density)
        val (top, bottom) = textBlockVerticalPadding(item, density)
        setPadding(left.toInt(), top.toInt(), right, bottom.toInt())
        setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, blockSize)
        typeface = if (item.code) Typeface.MONOSPACE else Typeface.DEFAULT
        setTextColor(
            when {
                item.quote -> palette.mutedArgb
                else -> palette.textArgb
            },
        )
        setLineSpacing(0f, if (item.code) 1.18f else 1.34f)
        if (appliedSpannable !== spannable) {
            appliedSpannable = spannable
            setText(spannable, BufferType.SPANNABLE)
        }
        setTextIsSelectable(true)
        // 链接命中区依赖布局；首帧布局完成后再刷新一次，避免打开新笔记时漏登记。
        post { refreshLayoutData() }
    }

    override fun onDraw(canvas: AndroidCanvas) {
        val item = item
        val palette = palette
        if (item != null && palette != null) {
            val density = resources.displayMetrics.density
            val fullWidth = width.toFloat()
            if (item.code) {
                canvas.drawRoundRect(
                    0f,
                    0f,
                    fullWidth,
                    height.toFloat(),
                    8f * density,
                    8f * density,
                    Paint(Paint.ANTI_ALIAS_FLAG).apply { color = palette.codeBackgroundArgb },
                )
            }
            if (item.callout != null) {
                canvas.drawRoundRect(
                    0f,
                    0f,
                    fullWidth,
                    height.toFloat(),
                    8f * density,
                    8f * density,
                    Paint(Paint.ANTI_ALIAS_FLAG).apply {
                        color = calloutBackground(item.callout.kind, palette).toArgbCompat()
                    },
                )
                canvas.drawRect(
                    0f,
                    0f,
                    3f * density,
                    height.toFloat(),
                    Paint(Paint.ANTI_ALIAS_FLAG).apply {
                        color = calloutAccent(item.callout.kind, palette).toArgbCompat()
                    },
                )
            } else if (item.quote) {
                canvas.drawRect(
                    0f,
                    0f,
                    3f * density,
                    height.toFloat(),
                    Paint(Paint.ANTI_ALIAS_FLAG).apply { color = palette.ruleArgb },
                )
            }
            if (item.rule) {
                canvas.drawRect(
                    0f,
                    height / 2f,
                    fullWidth,
                    height / 2f + max(1f, density),
                    Paint(Paint.ANTI_ALIAS_FLAG).apply { color = palette.ruleArgb },
                )
            }
        }
        super.onDraw(canvas)
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        super.onLayout(changed, left, top, right, bottom)
        if (changed) {
            post { onLayoutReady?.invoke() }
        }
    }

    private fun linkAt(x: Float, y: Float): TextLink? {
        val layout = layout ?: return null
        val vertical = (y - totalPaddingTop).toInt()
        if (vertical < 0 || vertical > layout.height) return null
        val line = layout.getLineForVertical(vertical)
        val horizontal = x - totalPaddingLeft
        if (horizontal < layout.getLineLeft(line) - 2f ||
            horizontal > layout.getLineRight(line) + 2f
        ) {
            return null
        }
        val offset = layout.getOffsetForHorizontal(line, horizontal)
        return links.firstOrNull { offset >= it.start && offset < it.end }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = event.x
                downY = event.y
                moved = false
            }
            MotionEvent.ACTION_MOVE -> {
                if (kotlin.math.hypot(event.x - downX, event.y - downY) > touchSlop) moved = true
            }
        }
        val handled = super.onTouchEvent(event)
        if (event.actionMasked == MotionEvent.ACTION_UP && !moved) {
            val start = selectionStart
            val end = selectionEnd
            val hasSelection = start >= 0 && end >= 0 && start != end
            if (!hasSelection) {
                linkAt(event.x, event.y)?.let { link ->
                    onLink?.invoke(link)
                    return true
                }
            }
        }
        return handled
    }

    fun refreshHits() {
        if (links.isEmpty()) return
        val layout = layout ?: return
        val location = IntArray(2)
        getLocationOnScreen(location)
        val hits = ArrayList<ScreenHit>(links.size)
        for (link in links) {
            for (rect in layoutRects(layout, link.start, link.end)) {
                hits += ScreenHit(
                    kind = "link",
                    label = link.alias ?: link.target,
                    x = location[0] + totalPaddingLeft + rect.left,
                    y = location[1] + totalPaddingTop + rect.top,
                    width = rect.width(),
                    height = rect.height(),
                    target = link.resolvedPath ?: link.target,
                    heading = link.heading,
                    canCollapse = false,
                    nodeId = null,
                )
            }
        }
        HitRegistry.replace("text:$itemKey", hits)
    }

    fun refreshLayoutData() {
        refreshHits()
        if (!Inspection.recordingEnabled) return
        val layout = layout ?: return
        val item = item ?: return

        val lines = ArrayList<LineMetrics>(layout.lineCount)
        for (line in 0 until layout.lineCount) {
            lines += LineMetrics(
                index = line,
                top = layout.getLineTop(line) + totalPaddingTop.toFloat(),
                bottom = layout.getLineBottom(line) + totalPaddingTop.toFloat(),
                baselineY = layout.getLineBaseline(line) + totalPaddingTop.toFloat(),
            )
        }
        val boxes = ArrayList<MathBoxMetrics>()
        for (offset in mathByOffset.keys) {
            val prepared = mathByOffset[offset] ?: continue
            if (offset !in 0 until layout.text.length) continue
            val line = layout.getLineForOffset(offset)
            val padding = mathInlinePadding(prepared.em)
            val baseline = layout.getLineBaseline(line) + totalPaddingTop.toFloat()
            boxes += MathBoxMetrics(
                formulaKey = "$offset:${mathLatex[offset].orEmpty()}",
                latex = mathLatex[offset].orEmpty(),
                lineIndex = line,
                x = layout.getPrimaryHorizontal(offset) + totalPaddingLeft + padding,
                top = baseline - prepared.ascent,
                baselineY = baseline,
                bottom = baseline + prepared.descent,
                width = prepared.width + padding * 2f,
                em = prepared.em,
                rendered = true,
            )
        }
        LayoutInspection.recordText(
            "text:$itemKey",
            notePath,
            blockId,
            lines,
            boxes,
        )
    }
}

@Composable
internal fun TextBlockView(
    item: TextItem,
    contentWidth: Int,
    bodySizePx: Float,
    palette: ReaderPalette,
    repository: ReaderRepository,
    notePath: String,
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
    val mathByLocal = HashMap<Int, PreparedMath?>()
    val mathLatex = HashMap<Int, String>()
    for (formula in item.inlineMath) {
        val local = (formula.displayStart.toLong() - item.displayStart).toInt()
        if (local !in 0 until item.text.length) continue
        mathByLocal[local] = mathResults[mathKey(formula)]?.prepared
        mathLatex[local] = formula.latex
    }
    val spannable = remember(
        item.key,
        item.text,
        contentWidth,
        blockSize,
        palette,
        mathResults.size,
        images.size,
        density,
    ) {
        buildTextSpannable(
            item = item,
            blockSize = blockSize,
            palette = palette,
            math = mathResults.mapValues { it.value.prepared },
            images = images,
        )
    }
    var textView by remember(item.key) { mutableStateOf<ReaderTextView?>(null) }
    val failures = item.inlineMath.mapNotNull { formula ->
        val result = mathResults[mathKey(formula)]
        if (result != null && result.prepared == null && result.reason != null) formula to result.reason else null
    }
    Column(modifier.fillMaxWidth()) {
        AndroidView(
            factory = { context ->
                ReaderTextView(context).apply {
                    layoutParams = ViewGroup.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT,
                    )
                    onLayoutReady = { refreshLayoutData() }
                    textView = this
                }
            },
            update = { view ->
                view.configure(
                    item = item,
                    notePath = notePath,
                    blockSize = blockSize,
                    contentWidth = contentWidth,
                    palette = palette,
                    density = density,
                    spannable = spannable,
                    math = mathByLocal,
                    latex = mathLatex,
                    onLink = onLink,
                )
                view.refreshLayoutData()
            },
            modifier = Modifier
                .fillMaxWidth()
                .onGloballyPositioned { if (item.links.isNotEmpty()) textView?.refreshHits() },
        )
        for ((formula, reason) in failures) {
            Text(
                "${formula.displaySource}: $reason",
                style = MaterialTheme.typography.labelSmall,
                color = palette.error,
                modifier = Modifier.padding(start = 8.dp),
            )
        }
    }
    DisposableEffect(item.key) {
        onDispose { HitRegistry.replace("text:${item.key}", emptyList()) }
    }
}

private fun mathKey(formula: MathBlock): String = "${formula.displayStart}:${formula.displayEnd()}:${formula.style}"
private fun MathBlock.displayEnd(): ULong = displayStart + displayLength
private fun mediaKey(media: MediaBlock): String = "${media.displayStart}:${media.displayLength}"

internal fun buildTextSpannable(
    item: TextItem,
    blockSize: Float,
    palette: ReaderPalette,
    math: Map<String, PreparedMath?>,
    images: Map<String, LoadedImage?>,
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
        val fallback = media.imageReason.ifBlank { media.imageAlt.ifBlank { "[image]" } }
        spannable.setSpan(
            ImageReplacementSpan(image, fallback, palette.errorArgb),
            local,
            min(local + 1, item.text.length),
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
    }
    return spannable
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

private fun androidx.compose.ui.graphics.Color.toArgbCompat(): Int = android.graphics.Color.argb(
    (alpha * 255).toInt().coerceIn(0, 255),
    (red * 255).toInt().coerceIn(0, 255),
    (green * 255).toInt().coerceIn(0, 255),
    (blue * 255).toInt().coerceIn(0, 255),
)
