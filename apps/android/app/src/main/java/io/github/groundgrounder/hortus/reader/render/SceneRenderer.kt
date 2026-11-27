// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.render

import android.graphics.Canvas
import android.graphics.Color
import android.graphics.DashPathEffect
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.Typeface
import io.github.groundgrounder.hortus.core.SceneColor
import io.github.groundgrounder.hortus.core.SceneData
import io.github.groundgrounder.hortus.core.SceneHit
import io.github.groundgrounder.hortus.core.ScenePoint
import io.github.groundgrounder.hortus.core.ScenePrimitive
import io.github.groundgrounder.hortus.core.SceneRect
import io.github.groundgrounder.hortus.core.SceneText
import java.util.Locale
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.round

internal object DiagramFonts {
    private val cache = java.util.concurrent.ConcurrentHashMap<String, Typeface>()

    fun typeface(family: String, weight: Int, italic: Boolean = false): Typeface {
        val key = "$family|$weight|$italic"
        return cache.getOrPut(key) {
            val name = when (family) {
                "System Font", ".AppleSystemUIFont", "SF Pro", "SF Pro Text", "" -> "sans-serif"
                else -> family
            }
            val base = try {
                Typeface.create(name, if (weight >= 600) Typeface.BOLD else Typeface.NORMAL)
            } catch (_: RuntimeException) {
                if (weight >= 600) Typeface.DEFAULT_BOLD else Typeface.DEFAULT
            }
            if (italic) Typeface.create(base, Typeface.ITALIC) else base
        }
    }

    fun localeFor(family: String): Locale =
        if (family.contains("CJK") || family.contains("Noto Sans SC") || family.contains("PingFang")) {
            Locale.SIMPLIFIED_CHINESE
        } else {
            Locale.ENGLISH
        }
}

private data class SceneTextKey(
    val text: String,
    val family: String,
    val weight: Int,
    val size: Float,
    val zoom: Float,
)

internal object SceneTextMetrics {
    internal const val MAX_SCALES = 2048
    internal const val MAX_PAINTS = 64

    private val scalesLock = Any()
    private val paintsLock = Any()

    private val scales = object : LinkedHashMap<SceneTextKey, Float>(256, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<SceneTextKey, Float>): Boolean =
            size > MAX_SCALES
    }
    private val paints = object : LinkedHashMap<String, Paint>(32, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Paint>): Boolean =
            size > MAX_PAINTS
    }

    private fun quantizedZoom(zoom: Float): Float =
        if (zoom > 0f && zoom.isFinite()) (round(zoom * 1000f) / 1000f).coerceAtLeast(0.001f) else zoom

    fun scaleX(span: SceneText, zoom: Float): Float {
        val zoomKey = quantizedZoom(zoom)
        val key = SceneTextKey(
            span.text,
            span.fontFamily,
            span.fontWeight.toInt(),
            span.fontSize.toFloat(),
            zoomKey,
        )
        synchronized(scalesLock) {
            scales[key]?.let { return it }
        }
        val paint = Paint(templatePaint(span, zoomKey))
        val target = (span.width * zoomKey * 16.0).toFloat()
        repeat(5) {
            val raw = paint.measureText(span.text)
            if (raw <= 0f) return@repeat
            if (abs(target - raw) <= 0.01f) return@repeat
            paint.textScaleX *= target / raw
        }
        val computed = paint.textScaleX
        synchronized(scalesLock) {
            scales[key] = computed
        }
        return computed
    }

    fun prewarmScene(scene: SceneData, zoom: Float) {
        if (zoom <= 0f || !zoom.isFinite()) return
        for (primitive in scene.primitives) {
            if (primitive is ScenePrimitive.Text && primitive.span.fontSize > 0.0) {
                scaleX(primitive.span, zoom)
            }
        }
    }

    fun put(span: SceneText, zoom: Float, value: Float) {
        val key = SceneTextKey(
            span.text,
            span.fontFamily,
            span.fontWeight.toInt(),
            span.fontSize.toFloat(),
            quantizedZoom(zoom),
        )
        synchronized(scalesLock) {
            scales[key] = value
        }
    }

    /** 返回独立 Paint 副本，避免主线程与后台线程竞争修改同一个共享 Paint。 */
    fun paint(span: SceneText, zoom: Float, color: Int): Paint {
        val scale = scaleX(span, zoom)
        val template = templatePaint(span, quantizedZoom(zoom))
        return Paint(template).apply {
            textSize = span.fontSize.toFloat() * zoom * 16f
            textScaleX = scale
            this.color = color
        }
    }

    /** 复用调用方局部 Paint（仅单次 draw 内部线程局部使用），消除逐文本图元分配。 */
    fun configurePaint(target: Paint, span: SceneText, zoom: Float, color: Int) {
        val scale = scaleX(span, zoom)
        target.set(templatePaint(span, quantizedZoom(zoom)))
        target.textSize = span.fontSize.toFloat() * zoom * 16f
        target.textScaleX = scale
        target.color = color
    }

    private fun templatePaint(span: SceneText, zoom: Float): Paint {
        val zoomKey = quantizedZoom(zoom)
        val key = "${span.fontFamily}|${span.fontWeight}|${span.fontSize}|$zoomKey"
        synchronized(paintsLock) {
            paints[key]?.let { return it }
        }
        val created = Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG or Paint.LINEAR_TEXT_FLAG).apply {
            typeface = DiagramFonts.typeface(span.fontFamily, span.fontWeight.toInt())
            textSize = span.fontSize.toFloat() * zoomKey * 16f
            isLinearText = true
            isSubpixelText = true
            hinting = Paint.HINTING_OFF
            textLocale = DiagramFonts.localeFor(span.fontFamily)
        }
        synchronized(paintsLock) {
            return paints[key] ?: created.also { paints[key] = it }
        }
    }

    internal fun cacheSizes(): Pair<Int, Int> {
        val s = synchronized(scalesLock) { scales.size }
        val p = synchronized(paintsLock) { paints.size }
        return s to p
    }
}

internal object SceneRenderer {
    private class DrawScratch {
        val clipF = RectF()
        val fillPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.FILL }
        val strokePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
        val pathPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeJoin = Paint.Join.ROUND
            strokeCap = Paint.Cap.ROUND
        }
        val linePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { strokeCap = Paint.Cap.ROUND }
        val markerPaint = Paint(Paint.ANTI_ALIAS_FLAG)
        val textPaint = Paint()
        val rect = RectF()
        val textBounds = RectF()
        val innerOval = RectF()
        val path = Path()
        val detailPath = Path()
        val markerPath = Path()
        var lastDashZoom = -1f
        var lastDashEffect: DashPathEffect? = null

        fun reset(clip: android.graphics.Rect) {
            clipF.set(clip)
        }

        fun dashEffect(zoom: Float): DashPathEffect {
            if (lastDashZoom != zoom || lastDashEffect == null) {
                val dash = 4f * zoom
                lastDashEffect = DashPathEffect(floatArrayOf(dash, dash), 0f)
                lastDashZoom = zoom
            }
            return lastDashEffect!!
        }
    }

    private val threadScratch = ThreadLocal.withInitial { DrawScratch() }

    fun alpha(color: SceneColor): Int = (color.a * 255).toInt().coerceIn(0, 255)

    fun colorInt(color: SceneColor, foreground: Int): Int {
        val a = alpha(color)
        if (a == 0 && color.r == 0.0 && color.g == 0.0 && color.b == 0.0) return 0
        return Color.argb(
            a,
            (color.r * 255).toInt().coerceIn(0, 255),
            (color.g * 255).toInt().coerceIn(0, 255),
            (color.b * 255).toInt().coerceIn(0, 255),
        )
    }

    fun draw(
        canvas: Canvas,
        scene: SceneData,
        foreground: Int,
        zoom: Float = 1f,
        translateX: Float = 0f,
        translateY: Float = 0f,
        includeText: Boolean = true,
    ) {
        val clip = canvas.clipBounds
        val scratch = threadScratch.get()!!.apply { reset(clip) }
        canvas.save()
        canvas.translate(translateX, translateY)
        for (primitive in scene.primitives) {
            when (primitive) {
                is ScenePrimitive.RoundedRect -> {
                    primitive.rect.writeTo(scratch.rect, zoom)
                    if (RectF.intersects(scratch.rect, scratch.clipF)) {
                        scratch.fillPaint.color = colorInt(primitive.color, foreground)
                        val r = primitive.radius.toFloat() * zoom
                        canvas.drawRoundRect(scratch.rect, r, r, scratch.fillPaint)
                    }
                }
                is ScenePrimitive.Line -> {
                    scratch.linePaint.color = colorInt(primitive.color, foreground)
                    scratch.linePaint.strokeWidth = primitive.width.toFloat() * zoom
                    canvas.drawLine(
                        primitive.from.x.toFloat() * zoom,
                        primitive.from.y.toFloat() * zoom,
                        primitive.to.x.toFloat() * zoom,
                        primitive.to.y.toFloat() * zoom,
                        scratch.linePaint,
                    )
                }
                is ScenePrimitive.GraphPath -> {
                    val points = primitive.points
                    if (points.isNotEmpty()) {
                        writeRoundedPath(scratch.path, points, primitive.cornerRadius, zoom)
                        scratch.pathPaint.color = colorInt(primitive.color, foreground)
                        scratch.pathPaint.strokeWidth = primitive.width.toFloat() * zoom
                        scratch.pathPaint.pathEffect = if (primitive.line == "Dotted") {
                            scratch.dashEffect(zoom)
                        } else {
                            null
                        }
                        canvas.drawPath(scratch.path, scratch.pathPaint)
                        marker(primitive.start, points, true, primitive.color, foreground, zoom, canvas, scratch)
                        marker(primitive.end, points, false, primitive.color, foreground, zoom, canvas, scratch)
                    }
                }
                is ScenePrimitive.GraphShape -> {
                    primitive.rect.writeTo(scratch.rect, zoom)
                    if (primitive.outline.isEmpty()) {
                        writeStandardShape(scratch.path, primitive.shape, scratch.rect, scratch.innerOval)
                    } else {
                        writePath(scratch.path, primitive.outline, zoom)
                        scratch.path.close()
                    }
                    if (alpha(primitive.fill) > 0 || primitive.fill.r + primitive.fill.g + primitive.fill.b > 0.0) {
                        scratch.fillPaint.color = colorInt(primitive.fill, foreground)
                        canvas.drawPath(scratch.path, scratch.fillPaint)
                    }
                    scratch.strokePaint.color = colorInt(primitive.stroke, foreground)
                    scratch.strokePaint.strokeWidth = primitive.strokeWidth.toFloat() * zoom
                    scratch.strokePaint.strokeJoin = Paint.Join.ROUND
                    canvas.drawPath(scratch.path, scratch.strokePaint)
                    if (primitive.details.isNotEmpty()) {
                        for (detail in primitive.details) {
                            writePath(scratch.detailPath, detail, zoom)
                            canvas.drawPath(scratch.detailPath, scratch.strokePaint)
                        }
                    }
                }
                is ScenePrimitive.LabelBackground -> {
                    primitive.rect.writeTo(scratch.rect, zoom)
                    scratch.fillPaint.color = colorInt(primitive.color, foreground)
                    canvas.drawRect(scratch.rect, scratch.fillPaint)
                }
                is ScenePrimitive.Text -> {
                    if (!includeText) continue
                    drawText(canvas, primitive.span, foreground, zoom, scratch)
                }
            }
        }
        canvas.restore()
    }

    private fun drawText(
        canvas: Canvas,
        span: SceneText,
        foreground: Int,
        zoom: Float,
        scratch: DrawScratch,
    ) {
        val supersample = 16f
        val size = span.fontSize.toFloat() * zoom
        if (size <= 0f) return
        val x = span.baseline.x.toFloat() * zoom
        val y = span.baseline.y.toFloat() * zoom
        scratch.textBounds.set(
            x - 2f,
            y - size * 1.5f,
            x + (span.width * zoom).toFloat() + 4f,
            y + size * 0.6f,
        )
        if (!RectF.intersects(scratch.textBounds, scratch.clipF)) return
        SceneTextMetrics.configurePaint(scratch.textPaint, span, zoom, colorInt(span.color, foreground))
        canvas.save()
        canvas.translate(x, y)
        canvas.scale(1f / supersample, 1f / supersample)
        canvas.drawText(span.text, 0f, 0f, scratch.textPaint)
        canvas.restore()
    }

    private fun marker(
        kind: String,
        points: List<ScenePoint>,
        atStart: Boolean,
        sceneColor: SceneColor,
        foreground: Int,
        zoom: Float,
        canvas: Canvas,
        scratch: DrawScratch,
    ) {
        if (kind == "None" || points.size < 2) return
        val tip = if (atStart) points.first() else points.last()
        val next = if (atStart) points[1] else points[points.size - 2]
        val dx = tip.x - next.x
        val dy = tip.y - next.y
        val length = hypot(dx, dy)
        if (length <= 0.0) return
        val ux = dx / length
        val uy = dy / length
        scratch.markerPaint.color = colorInt(sceneColor, foreground)
        scratch.markerPaint.strokeWidth = 1.5f * zoom
        when (kind) {
            "Arrow" -> {
                val path = scratch.markerPath
                path.rewind()
                path.moveTo(tip.x.toFloat() * zoom, tip.y.toFloat() * zoom)
                path.lineTo(
                    (tip.x - ux * 9 - uy * 4).toFloat() * zoom,
                    (tip.y - uy * 9 + ux * 4).toFloat() * zoom,
                )
                path.lineTo(
                    (tip.x - ux * 9 + uy * 4).toFloat() * zoom,
                    (tip.y - uy * 9 - ux * 4).toFloat() * zoom,
                )
                path.close()
                scratch.markerPaint.style = Paint.Style.FILL
                canvas.drawPath(path, scratch.markerPaint)
            }
            "Circle" -> {
                val cx = (tip.x - ux * 4.25).toFloat() * zoom
                val cy = (tip.y - uy * 4.25).toFloat() * zoom
                scratch.markerPaint.style = Paint.Style.STROKE
                canvas.drawCircle(cx, cy, 3.5f * zoom, scratch.markerPaint)
            }
            "Cross" -> {
                val cx = (tip.x - ux * 5).toFloat() * zoom
                val cy = (tip.y - uy * 5).toFloat() * zoom
                scratch.markerPaint.style = Paint.Style.STROKE
                drawCrossLine(canvas, cx, cy, ux, uy, -1.0, zoom, scratch.markerPaint)
                drawCrossLine(canvas, cx, cy, ux, uy, 1.0, zoom, scratch.markerPaint)
            }
        }
    }

    private fun drawCrossLine(
        canvas: Canvas,
        cx: Float,
        cy: Float,
        ux: Double,
        uy: Double,
        sign: Double,
        zoom: Float,
        paint: Paint,
    ) {
        canvas.drawLine(
            (cx - ux * 3 - uy * 3 * sign).toFloat() * zoom,
            (cy - uy * 3 + ux * 3 * sign).toFloat() * zoom,
            (cx + ux * 3 + uy * 3 * sign).toFloat() * zoom,
            (cy + uy * 3 - ux * 3 * sign).toFloat() * zoom,
            paint,
        )
    }

    private fun writeStandardShape(path: Path, shape: String, rect: RectF, innerOval: RectF) {
        path.rewind()
        when (shape) {
            "Stadium" -> {
                val radius = min(rect.width(), rect.height()) / 2
                path.addRoundRect(rect, radius, radius, Path.Direction.CW)
            }
            "Circle", "DoubleCircle" -> {
                path.addOval(rect, Path.Direction.CW)
                if (shape == "DoubleCircle") {
                    innerOval.set(rect.left + 2.5f, rect.top + 2.5f, rect.right - 2.5f, rect.bottom - 2.5f)
                    path.addOval(innerOval, Path.Direction.CW)
                }
            }
            else -> {
                val r = min(10f, rect.height() / 2)
                path.addRoundRect(rect, r, r, Path.Direction.CW)
            }
        }
    }

    private fun writePath(result: Path, points: List<ScenePoint>, zoom: Float) {
        result.rewind()
        val first = points.firstOrNull() ?: return
        result.moveTo(first.x.toFloat() * zoom, first.y.toFloat() * zoom)
        for (i in 1 until points.size) {
            val point = points[i]
            result.lineTo(point.x.toFloat() * zoom, point.y.toFloat() * zoom)
        }
    }

    private fun writeRoundedPath(result: Path, points: List<ScenePoint>, radius: Double, zoom: Float) {
        result.rewind()
        val first = points.firstOrNull() ?: return
        result.moveTo(first.x.toFloat() * zoom, first.y.toFloat() * zoom)
        if (points.size > 2) {
            for (index in 1 until points.size - 1) {
                val a = points[index - 1]
                val b = points[index]
                val c = points[index + 1]
                val incoming = hypot(b.x - a.x, b.y - a.y)
                val outgoing = hypot(c.x - b.x, c.y - b.y)
                val r = minOf(radius, incoming / 2, outgoing / 2)
                if (r > 0.001 && incoming > 0.0 && outgoing > 0.0) {
                    val startX = b.x + (a.x - b.x) * r / incoming
                    val startY = b.y + (a.y - b.y) * r / incoming
                    val endX = b.x + (c.x - b.x) * r / outgoing
                    val endY = b.y + (c.y - b.y) * r / outgoing
                    result.lineTo(startX.toFloat() * zoom, startY.toFloat() * zoom)
                    result.quadTo(
                        b.x.toFloat() * zoom,
                        b.y.toFloat() * zoom,
                        endX.toFloat() * zoom,
                        endY.toFloat() * zoom,
                    )
                } else {
                    result.lineTo(b.x.toFloat() * zoom, b.y.toFloat() * zoom)
                }
            }
        }
        val last = points.last()
        result.lineTo(last.x.toFloat() * zoom, last.y.toFloat() * zoom)
    }

    private fun SceneRect.writeTo(out: RectF, zoom: Float) {
        out.set(
            x.toFloat() * zoom,
            y.toFloat() * zoom,
            (x + width).toFloat() * zoom,
            (y + height).toFloat() * zoom,
        )
    }

    fun hitAt(
        scene: SceneData,
        x: Float,
        y: Float,
        zoom: Float = 1f,
        translateX: Float = 0f,
        translateY: Float = 0f,
    ): SceneHit? {
        val sceneX = (x - translateX) / zoom
        val sceneY = (y - translateY) / zoom
        return scene.hitRegions.lastOrNull { hit ->
            sceneX >= hit.rect.x &&
                sceneX <= hit.rect.x + hit.rect.width &&
                sceneY >= hit.rect.y &&
                sceneY <= hit.rect.y + hit.rect.height
        }
    }
}
