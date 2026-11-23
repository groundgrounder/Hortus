// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

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

internal class SceneCounts {
    var roundedRect = 0
    var line = 0
    var graphPath = 0
    var graphShape = 0
    var labelBackground = 0
    var text = 0
    var arrow = 0
    var circle = 0
    var cross = 0
    var solid = 0
    var dotted = 0
    var thick = 0
    var otherMarker = 0
    var otherLine = 0
    var textSamples = 0
    var maxTextWidthError = 0.0
    var worstTextDiagram = ""
    var worstText = ""

    fun merge(other: SceneCounts) {
        roundedRect += other.roundedRect
        line += other.line
        graphPath += other.graphPath
        graphShape += other.graphShape
        labelBackground += other.labelBackground
        text += other.text
        arrow += other.arrow
        circle += other.circle
        cross += other.cross
        solid += other.solid
        dotted += other.dotted
        thick += other.thick
        otherMarker += other.otherMarker
        otherLine += other.otherLine
        textSamples += other.textSamples
        if (other.maxTextWidthError > maxTextWidthError) {
            maxTextWidthError = other.maxTextWidthError
            worstTextDiagram = other.worstTextDiagram
            worstText = other.worstText
        }
    }

    fun toJson(): org.json.JSONObject = org.json.JSONObject()
        .put("RoundedRect", roundedRect)
        .put("Line", line)
        .put("GraphPath", graphPath)
        .put("GraphShape", graphShape)
        .put("LabelBackground", labelBackground)
        .put("Text", text)
        .put("Arrow", arrow)
        .put("Circle", circle)
        .put("Cross", cross)
        .put("Solid", solid)
        .put("Dotted", dotted)
        .put("Thick", thick)
        .put("otherMarker", otherMarker)
        .put("otherLine", otherLine)
        .put("textSamples", textSamples)
        .put("maxTextWidthErrorPx", maxTextWidthError)
        .put("worstTextDiagram", worstTextDiagram)
        .put("worstText", worstText)
}

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
    private val scales = java.util.concurrent.ConcurrentHashMap<SceneTextKey, Float>()
    private val paints = java.util.concurrent.ConcurrentHashMap<String, Paint>()

    fun scaleX(span: SceneText, zoom: Float): Float {
        val key = SceneTextKey(
            span.text,
            span.fontFamily,
            span.fontWeight.toInt(),
            span.fontSize.toFloat(),
            zoom,
        )
        return scales.getOrPut(key) {
            val paint = basePaint(span, zoom, 0)
            val target = (span.width * zoom * 16.0).toFloat()
            repeat(5) {
                val raw = paint.measureText(span.text)
                if (raw <= 0f) return@repeat
                if (abs(target - raw) <= 0.01f) return@repeat
                paint.textScaleX *= target / raw
            }
            paint.textScaleX
        }
    }

    fun put(span: SceneText, zoom: Float, value: Float) {
        scales[SceneTextKey(span.text, span.fontFamily, span.fontWeight.toInt(), span.fontSize.toFloat(), zoom)] = value
    }

    fun paint(span: SceneText, zoom: Float, color: Int): Paint {
        // 先算横向比例再取画笔：scaleX 会复用同一支 Paint 测量，不能让它覆盖最终颜色。
        val scale = scaleX(span, zoom)
        val paint = basePaint(span, zoom, color)
        paint.textScaleX = scale
        return paint
    }

    private fun basePaint(span: SceneText, zoom: Float, color: Int): Paint {
        val key = "${span.fontFamily}|${span.fontWeight}|${span.fontSize}|${zoom}"
        val paint = paints.getOrPut(key) {
            Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG or Paint.LINEAR_TEXT_FLAG).apply {
                typeface = DiagramFonts.typeface(span.fontFamily, span.fontWeight.toInt())
                textSize = span.fontSize.toFloat() * zoom * 16f
                isLinearText = true
                isSubpixelText = true
                hinting = Paint.HINTING_OFF
                textLocale = DiagramFonts.localeFor(span.fontFamily)
            }
        }
        paint.color = color
        return paint
    }
}

internal object SceneRenderer {
    fun alpha(color: SceneColor): Int = (color.a * 255).toInt().coerceIn(0, 255)

    fun colorInt(color: SceneColor, foreground: Int): Int {
        val a = alpha(color)
        if (a == 0 && color.r == 0.0 && color.g == 0.0 && color.b == 0.0) return 0
        return Color.argb(a, (color.r * 255).toInt().coerceIn(0, 255),
            (color.g * 255).toInt().coerceIn(0, 255), (color.b * 255).toInt().coerceIn(0, 255))
    }

    fun draw(
        canvas: Canvas,
        scene: SceneData,
        foreground: Int,
        zoom: Float = 1f,
        translateX: Float = 0f,
        translateY: Float = 0f,
        includeText: Boolean = true,
        counts: SceneCounts? = null,
        diagramKey: String = "",
    ) {
        val clip = canvas.clipBounds
        canvas.save()
        canvas.translate(translateX, translateY)
        for (primitive in scene.primitives) {
            when (primitive) {
                is ScenePrimitive.RoundedRect -> {
                    counts?.let { it.roundedRect += 1 }
                    val rect = primitive.rect.toRectF(zoom)
                    if (rect.intersects(clip.left.toFloat(), clip.top.toFloat(), clip.right.toFloat(), clip.bottom.toFloat())) {
                        canvas.drawRoundRect(rect, primitive.radius.toFloat() * zoom,
                            primitive.radius.toFloat() * zoom,
                            Paint(Paint.ANTI_ALIAS_FLAG).apply { color = colorInt(primitive.color, foreground) })
                    }
                }
                is ScenePrimitive.Line -> {
                    counts?.let { it.line += 1 }
                    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                        color = colorInt(primitive.color, foreground)
                        strokeWidth = primitive.width.toFloat() * zoom
                        strokeCap = Paint.Cap.ROUND
                    }
                    canvas.drawLine(
                        primitive.from.x.toFloat() * zoom,
                        primitive.from.y.toFloat() * zoom,
                        primitive.to.x.toFloat() * zoom,
                        primitive.to.y.toFloat() * zoom,
                        paint,
                    )
                }
                is ScenePrimitive.GraphPath -> {
                    counts?.let { it.graphPath += 1 }
                    when (primitive.line) {
                        "Dotted" -> counts?.let { it.dotted += 1 }
                        "Thick" -> counts?.let { it.thick += 1 }
                        "Solid" -> counts?.let { it.solid += 1 }
                        else -> counts?.let { it.otherLine += 1 }
                    }
                    val points = primitive.points
                    if (points.isNotEmpty()) {
                        val path = roundedPath(points, primitive.cornerRadius, zoom)
                        val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                            color = colorInt(primitive.color, foreground)
                            strokeWidth = primitive.width.toFloat() * zoom
                            style = Paint.Style.STROKE
                            strokeJoin = Paint.Join.ROUND
                            strokeCap = Paint.Cap.ROUND
                            if (primitive.line == "Dotted") {
                                val dash = 4f * zoom
                                pathEffect = DashPathEffect(floatArrayOf(dash, dash), 0f)
                            }
                        }
                        canvas.drawPath(path, paint)
                        marker(primitive.start, points, true, primitive.color, foreground, zoom, canvas, counts)
                        marker(primitive.end, points, false, primitive.color, foreground, zoom, canvas, counts)
                    }
                }
                is ScenePrimitive.GraphShape -> {
                    counts?.let { it.graphShape += 1 }
                    val rect = primitive.rect.toRectF(zoom)
                    val body = if (primitive.outline.isEmpty()) {
                        standardShape(primitive.shape, rect)
                    } else {
                        path(primitive.outline, zoom).also { it.close() }
                    }
                    val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                        color = colorInt(primitive.fill, foreground)
                        style = Paint.Style.FILL
                    }
                    if (alpha(primitive.fill) > 0 || primitive.fill.r + primitive.fill.g + primitive.fill.b > 0.0) {
                        canvas.drawPath(body, fill)
                    }
                    val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                        color = colorInt(primitive.stroke, foreground)
                        strokeWidth = primitive.strokeWidth.toFloat() * zoom
                        style = Paint.Style.STROKE
                        strokeJoin = Paint.Join.ROUND
                    }
                    canvas.drawPath(body, stroke)
                    if (primitive.details.isNotEmpty()) {
                        for (detail in primitive.details) {
                            canvas.drawPath(path(detail, zoom), stroke)
                        }
                    }
                }
                is ScenePrimitive.LabelBackground -> {
                    counts?.let { it.labelBackground += 1 }
                    canvas.drawRect(
                        primitive.rect.toRectF(zoom),
                        Paint(Paint.ANTI_ALIAS_FLAG).apply { color = colorInt(primitive.color, foreground) },
                    )
                }
                is ScenePrimitive.Text -> {
                    if (!includeText) continue
                    counts?.let { it.text += 1 }
                    drawText(canvas, primitive.span, foreground, zoom, counts, diagramKey, clip)
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
        counts: SceneCounts?,
        diagramKey: String,
        clip: android.graphics.Rect,
    ) {
        val supersample = 16f
        val size = span.fontSize.toFloat() * zoom
        if (size <= 0f) return
        val paint = SceneTextMetrics.paint(span, zoom, colorInt(span.color, foreground))
        val measured = paint.measureText(span.text).toDouble() / 16.0 / zoom
        if (counts != null && span.width > 0.0) {
            val error = abs(measured - span.width)
            counts.textSamples += 1
            if (error > counts.maxTextWidthError) {
                counts.maxTextWidthError = error
                counts.worstTextDiagram = diagramKey
                counts.worstText = span.text.take(80)
            }
        }
        val x = span.baseline.x.toFloat() * zoom
        val y = span.baseline.y.toFloat() * zoom
        val bounds = RectF(
            x - 2f,
            y - size * 1.5f,
            x + (span.width * zoom).toFloat() + 4f,
            y + size * 0.6f,
        )
        if (!RectF.intersects(bounds, RectF(clip))) return
        canvas.save()
        canvas.translate(x, y)
        canvas.scale(1f / supersample, 1f / supersample)
        canvas.drawText(span.text, 0f, 0f, paint)
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
        counts: SceneCounts?,
    ) {
        if (kind == "None") return
        when (kind) {
            "Arrow" -> counts?.let { it.arrow += 1 }
            "Circle" -> counts?.let { it.circle += 1 }
            "Cross" -> counts?.let { it.cross += 1 }
            else -> counts?.let { it.otherMarker += 1 }
        }
        if (points.size < 2) return
        val tip = if (atStart) points.first() else points.last()
        val next = if (atStart) points[1] else points[points.size - 2]
        val dx = tip.x - next.x
        val dy = tip.y - next.y
        val length = hypot(dx, dy)
        if (length <= 0.0) return
        val ux = dx / length
        val uy = dy / length
        val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = colorInt(sceneColor, foreground)
            strokeWidth = 1.5f * zoom
        }
        when (kind) {
            "Arrow" -> {
                val path = Path()
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
                canvas.drawPath(path, paint.apply { style = Paint.Style.FILL })
            }
            "Circle" -> {
                val cx = (tip.x - ux * 4.25).toFloat() * zoom
                val cy = (tip.y - uy * 4.25).toFloat() * zoom
                canvas.drawCircle(cx, cy, 3.5f * zoom, paint.apply { style = Paint.Style.STROKE })
            }
            "Cross" -> {
                val cx = (tip.x - ux * 5).toFloat() * zoom
                val cy = (tip.y - uy * 5).toFloat() * zoom
                paint.style = Paint.Style.STROKE
                for (sign in listOf(-1.0, 1.0)) {
                    canvas.drawLine(
                        (cx - ux * 3 - uy * 3 * sign).toFloat() * zoom,
                        (cy - uy * 3 + ux * 3 * sign).toFloat() * zoom,
                        (cx + ux * 3 + uy * 3 * sign).toFloat() * zoom,
                        (cy + uy * 3 - ux * 3 * sign).toFloat() * zoom,
                        paint,
                    )
                }
            }
        }
    }

    private fun standardShape(shape: String, rect: RectF): Path {
        val path = Path()
        when (shape) {
            "Stadium" -> {
                val radius = min(rect.width(), rect.height()) / 2
                path.addRoundRect(rect, radius, radius, Path.Direction.CW)
            }
            "Circle", "DoubleCircle" -> {
                path.addOval(rect, Path.Direction.CW)
                if (shape == "DoubleCircle") {
                    path.addOval(
                        RectF(rect.left + 2.5f, rect.top + 2.5f, rect.right - 2.5f, rect.bottom - 2.5f),
                        Path.Direction.CW,
                    )
                }
            }
            else -> path.addRoundRect(rect, min(10f, rect.height() / 2), min(10f, rect.height() / 2), Path.Direction.CW)
        }
        return path
    }

    private fun path(points: List<ScenePoint>, zoom: Float): Path {
        val result = Path()
        val first = points.firstOrNull() ?: return result
        result.moveTo(first.x.toFloat() * zoom, first.y.toFloat() * zoom)
        for (point in points.drop(1)) {
            result.lineTo(point.x.toFloat() * zoom, point.y.toFloat() * zoom)
        }
        return result
    }

    private fun roundedPath(points: List<ScenePoint>, radius: Double, zoom: Float): Path {
        val result = Path()
        val first = points.firstOrNull() ?: return result
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
        return result
    }

    fun SceneRect.toRectF(zoom: Float): RectF = RectF(
        x.toFloat() * zoom,
        y.toFloat() * zoom,
        (x + width).toFloat() * zoom,
        (y + height).toFloat() * zoom,
    )

    fun hitAt(scene: SceneData, x: Float, y: Float, zoom: Float = 1f, translateX: Float = 0f, translateY: Float = 0f): SceneHit? {
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
