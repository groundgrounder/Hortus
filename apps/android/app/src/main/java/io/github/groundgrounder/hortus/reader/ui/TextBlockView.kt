// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Canvas as AndroidCanvas
import android.graphics.Paint
import android.graphics.Typeface
import android.text.Layout
import android.text.Selection
import android.text.Spannable
import android.text.SpannableString
import android.util.TypedValue
import android.view.Gravity
import android.view.MotionEvent
import android.view.ViewConfiguration
import android.view.ViewGroup
import android.widget.TextView
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.DebugProbes.debugTextHits
import io.github.groundgrounder.hortus.core.model.LoadedImage
import io.github.groundgrounder.hortus.core.model.TextItem
import io.github.groundgrounder.hortus.core.model.TextLink
import io.github.groundgrounder.hortus.core.model.TextTag
import io.github.groundgrounder.hortus.reader.model.ItemRenderBridge
import io.github.groundgrounder.hortus.reader.model.ReaderPalette
import io.github.groundgrounder.hortus.reader.model.calloutAccent
import io.github.groundgrounder.hortus.reader.model.calloutBackground
import io.github.groundgrounder.hortus.reader.model.toArgbCompat
import io.github.groundgrounder.hortus.reader.render.MathRenderResult
import io.github.groundgrounder.hortus.reader.render.PreparedMath
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/**
 * 原生 TextView 承载阅读文本：系统负责选择手柄、复制工具栏和文本绘制。
 * 支持 AndroidView onReset 视图池复用，仅覆盖触摸以保留双链单击，并且不叠加任何自绘控件。
 */
internal class ReaderTextView(context: Context) : TextView(context) {
    var itemKey: String = ""
    var blockId: Long = 0
    var notePath: String = ""
    var onLink: ((TextLink) -> Unit)? = null
    var onTag: ((String) -> Unit)? = null
    var onLayoutReady: (() -> Unit)? = null

    private var item: TextItem? = null
    private var palette: ReaderPalette? = null
    internal var mathByOffset: Map<Int, PreparedMath?> = emptyMap()
        private set
    internal var mathLatex: Map<Int, String> = emptyMap()
        private set
    internal var links: List<TextLink> = emptyList()
        private set
    internal var tags: List<TextTag> = emptyList()
        private set
    private var appliedSpannable: SpannableString? = null
    private val bgPaint = Paint(Paint.ANTI_ALIAS_FLAG)
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

    fun resetForReuse() {
        val previousKey = itemKey
        if (DebugProbes.enabled && previousKey.isNotEmpty()) {
            DebugProbes.clearTextHits("text:$previousKey")
        }
        val currentText = text
        if (currentText is Spannable) {
            Selection.removeSelection(currentText)
        }
        clearFocus()
        item = null
        itemKey = ""
        blockId = 0
        notePath = ""
        palette = null
        onLink = null
        onTag = null
        links = emptyList()
        tags = emptyList()
        mathByOffset = emptyMap()
        mathLatex = emptyMap()
        appliedSpannable = null
        moved = false
        setText("", BufferType.SPANNABLE)
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
        onTag: (String) -> Unit,
    ) {
        this.item = item
        this.itemKey = item.key
        this.blockId = item.blockId
        this.notePath = notePath
        this.palette = palette
        this.mathByOffset = math
        this.mathLatex = latex
        this.links = item.links
        this.tags = item.tags
        if (DebugProbes.enabled && item.links.isEmpty() && item.tags.isEmpty()) {
            DebugProbes.clearTextHits("text:${item.key}")
        }
        this.onLink = onLink
        this.onTag = onTag
        val (left, right, _) = textBlockWidths(item, contentWidth.coerceAtLeast(1), density)
        val (top, bottom) = textBlockVerticalPadding(item, density)
        setPadding(left.toInt(), top.toInt(), right, bottom.toInt())
        setTextSize(TypedValue.COMPLEX_UNIT_PX, blockSize)
        typeface = if (item.code) Typeface.MONOSPACE else Typeface.DEFAULT
        setTextColor(
            if (item.quote) palette.mutedArgb else palette.textArgb,
        )
        setLineSpacing(0f, if (item.code) 1.18f else 1.34f)
        if (appliedSpannable !== spannable) {
            appliedSpannable = spannable
            setText(spannable, BufferType.SPANNABLE)
        }
        if (DebugProbes.enabled) {
            post { DebugProbes.refreshTextLayout(this) }
        }
    }

    override fun onDraw(canvas: AndroidCanvas) {
        val item = item
        val palette = palette
        if (item != null && palette != null) {
            val density = resources.displayMetrics.density
            val fullWidth = width.toFloat()
            if (item.code) {
                bgPaint.color = palette.codeBackgroundArgb
                canvas.drawRoundRect(
                    0f,
                    0f,
                    fullWidth,
                    height.toFloat(),
                    8f * density,
                    8f * density,
                    bgPaint,
                )
            }
            if (item.callout != null) {
                bgPaint.color = calloutBackground(item.callout.kind, palette).toArgbCompat()
                canvas.drawRoundRect(
                    0f,
                    0f,
                    fullWidth,
                    height.toFloat(),
                    8f * density,
                    8f * density,
                    bgPaint,
                )
                bgPaint.color = calloutAccent(item.callout.kind, palette).toArgbCompat()
                canvas.drawRect(0f, 0f, 3f * density, height.toFloat(), bgPaint)
            } else if (item.quote) {
                bgPaint.color = palette.ruleArgb
                canvas.drawRect(0f, 0f, 3f * density, height.toFloat(), bgPaint)
            }
            if (item.rule) {
                bgPaint.color = palette.ruleArgb
                canvas.drawRect(
                    0f,
                    height / 2f,
                    fullWidth,
                    height / 2f + max(1f, density),
                    bgPaint,
                )
            }
        }
        super.onDraw(canvas)
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        super.onLayout(changed, left, top, right, bottom)
        if (changed && DebugProbes.enabled) {
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

    private fun tagAt(x: Float, y: Float): TextTag? {
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
        return tags.firstOrNull { offset >= it.start && offset < it.end }
    }

    override fun onTextContextMenuItem(id: Int): Boolean {
        if (id == android.R.id.copy && mathLatex.isNotEmpty()) {
            val start = min(selectionStart, selectionEnd).coerceAtLeast(0)
            val end = max(selectionStart, selectionEnd).coerceAtMost(text.length)
            if (end > start && (start until end).any { it in mathLatex }) {
                val source = text
                val resolved = buildString(end - start) {
                    for (offset in start until end) {
                        val latex = mathLatex[offset]
                        if (latex != null && (source[offset] == '\uFFFC' || source[offset] == '\u200B')) {
                            append(latex)
                        } else {
                            append(source[offset])
                        }
                    }
                }
                context.getSystemService(ClipboardManager::class.java)
                    ?.setPrimaryClip(ClipData.newPlainText("Hortus", resolved))
                if (source is Spannable) {
                    Selection.setSelection(source, end)
                }
                clearFocus()
                return true
            }
        }
        return super.onTextContextMenuItem(id)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = event.x
                downY = event.y
                moved = false
            }
            MotionEvent.ACTION_MOVE -> {
                if (hypot(event.x - downX, event.y - downY) > touchSlop) moved = true
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
                tagAt(event.x, event.y)?.let { tag ->
                    onTag?.invoke(tag.tag)
                    return true
                }
            }
        }
        return handled
    }
}

@Composable
internal fun TextBlockView(
    item: TextItem,
    contentWidth: Int,
    bodySizePx: Float,
    palette: ReaderPalette,
    renderBridge: ItemRenderBridge,
    notePath: String,
    onLink: (TextLink) -> Unit,
    onTag: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val composeDensity = LocalDensity.current
    val density = composeDensity.density
    val blockScale = headingScale(item.headingLevel)
    val blockSize = bodySizePx * blockScale
    val mathResults = remember(item.key, blockSize) { mutableStateMapOf<String, MathRenderResult>() }
    val images = remember(item.key) { mutableStateMapOf<String, LoadedImage>() }
    val hasInlineMedia = item.inlineMath.isNotEmpty() || item.inlineImages.isNotEmpty()
    if (hasInlineMedia) {
        LaunchedEffect(item.key, item.text, item.inlineMath, item.inlineImages, blockSize, contentWidth, density) {
            val pending = item.inlineMath.filter { mathKey(it) !in mathResults }
            if (pending.isNotEmpty()) {
                val values = renderBridge.prepareMath(pending, blockSize)
                pending.forEachIndexed { index, formula ->
                    values.getOrNull(index)?.let { mathResults[mathKey(formula)] = it }
                }
            }
            val imageTarget = max(1, contentWidth - (24f * density).toInt())
            for (media in item.inlineImages) {
                val key = mediaKey(media)
                if (key in images) continue
                val requestedWidthPx = media.imageWidth?.takeIf { it > 0u }?.let { points ->
                    with(composeDensity) { points.toFloat().dp.roundToPx() }.coerceAtLeast(1)
                }
                images[key] = renderBridge.loadImage(
                    media.imagePath,
                    imageTarget,
                    requestedWidthPx,
                )
            }
        }
    }
    val mathByLocal = remember(item.key, item.inlineMath, mathResults.size) {
        if (item.inlineMath.isEmpty()) {
            emptyMap()
        } else {
            HashMap<Int, PreparedMath?>(item.inlineMath.size).apply {
                for (formula in item.inlineMath) {
                    val local = (formula.displayStart.toLong() - item.displayStart).toInt()
                    if (local in 0 until item.text.length) {
                        put(local, mathResults[mathKey(formula)]?.prepared)
                    }
                }
            }
        }
    }
    val mathLatex = remember(item.key, item.inlineMath) {
        if (item.inlineMath.isEmpty()) {
            emptyMap()
        } else {
            HashMap<Int, String>(item.inlineMath.size).apply {
                for (formula in item.inlineMath) {
                    val local = (formula.displayStart.toLong() - item.displayStart).toInt()
                    if (local in 0 until item.text.length) {
                        put(local, formula.displaySource.ifBlank { "\$${formula.latex}\$" })
                    }
                }
            }
        }
    }
    // 纯文本块不将 contentWidth 纳入 spannable 键，避免宽度微调引发无效重建。
    val spannableWidthKey = if (item.inlineImages.isNotEmpty()) contentWidth else 0
    val spannable = remember(
        item.key,
        item.text,
        spannableWidthKey,
        blockSize,
        palette,
        mathResults.size,
        images.size,
        density,
    ) {
        val preparedMath = if (item.inlineMath.isEmpty()) {
            emptyMap()
        } else {
            mathResults.mapValues { it.value.prepared }
        }
        buildTextSpannable(
            item = item,
            blockSize = blockSize,
            palette = palette,
            math = preparedMath,
            images = images,
            context = context,
        )
    }
    var textView by remember(item.key) { mutableStateOf<ReaderTextView?>(null) }
    val failures = if (item.inlineMath.isEmpty()) {
        emptyList()
    } else {
        item.inlineMath.mapNotNull { formula ->
            val result = mathResults[mathKey(formula)]
            if (result != null && result.prepared == null && result.reason != null) formula to result.reason else null
        }
    }
    Column(modifier.fillMaxWidth()) {
        AndroidView(
            factory = { context ->
                ReaderTextView(context).apply {
                    layoutParams = ViewGroup.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT,
                    )
                    if (DebugProbes.enabled) {
                        onLayoutReady = { DebugProbes.refreshTextLayout(this) }
                    }
                    textView = this
                }
            },
            update = { view ->
                textView = view
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
                    onTag = onTag,
                )
                if (DebugProbes.enabled) {
                    DebugProbes.refreshTextLayout(view)
                }
            },
            onReset = { view ->
                textView = null
                view.resetForReuse()
            },
            modifier = Modifier
                .fillMaxWidth()
                .debugTextHits(item.links.isNotEmpty() || item.tags.isNotEmpty()) {
                    textView?.let(DebugProbes::refreshTextHits)
                },
        )
        for ((formula, reason) in failures) {
            Text(
                "${formula.displaySource}: ${localizedRenderError(context, reason)}",
                style = MaterialTheme.typography.labelSmall,
                color = palette.error,
                modifier = Modifier.padding(start = 8.dp),
            )
        }
    }
    if (DebugProbes.enabled) {
        DisposableEffect(item.key) {
            onDispose { DebugProbes.clearTextHits("text:${item.key}") }
        }
    }
}
