// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.render

import android.content.res.AssetManager
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.DashPathEffect
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.RectF
import android.graphics.Typeface
import android.graphics.text.TextRunShaper
import android.os.Build
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.core.MathEngine
import io.github.groundgrounder.hortus.core.MathFailure
import io.github.groundgrounder.hortus.core.MathGlyphMetrics
import io.github.groundgrounder.hortus.core.MathStyle
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.max

private const val FOREGROUND = Int.MIN_VALUE

internal fun mathInlinePadding(em: Float): Float = max(1f, em * 0.08f)

internal data class MathRenderResult(
    val prepared: PreparedMath?,
    val reason: String?,
    val metricsCodepoints: Int,
)

internal sealed interface MathShape {
    data class Glyph(
        val typeface: Typeface,
        val text: String,
        val x: Float,
        val baseline: Float,
        val textSize: Float,
        val color: Int,
    ) : MathShape

    data class Line(
        val x: Float,
        val y: Float,
        val width: Float,
        val thickness: Float,
        val dashed: Boolean,
        val color: Int,
    ) : MathShape

    data class Rect(val rect: RectF, val color: Int) : MathShape
    data class PathShape(val path: Path, val fill: Boolean, val color: Int) : MathShape
}

/**
 * 公式场景已在后台解析并选好字体；绘制只提交不可变几何，主线程不做测量或加载。
 * 在循环外复用局部绘制 Paint，消除逐字形/逐线段高频分配。
 */
internal class PreparedMath(
    val width: Float,
    val height: Float,
    val ascent: Float,
    val descent: Float,
    val shapes: List<MathShape>,
    val em: Float,
    val glyphCount: Int,
    val fallbackGlyphCount: Int,
    val unresolvedGlyphCount: Int,
    val estimatedBytes: Int,
) {
    fun draw(canvas: Canvas, originX: Float, originY: Float, foreground: Int) {
        fun resolve(color: Int): Int = if (color == FOREGROUND) foreground else color
        val glyphPaint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG).apply {
            isLinearText = true
            hinting = Paint.HINTING_OFF
        }
        val fillPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.FILL
        }
        val strokePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
        }
        var lastDashThickness = -1f
        var lastDashEffect: DashPathEffect? = null
        canvas.save()
        canvas.translate(originX, originY)
        for (shape in shapes) {
            when (shape) {
                is MathShape.Glyph -> {
                    glyphPaint.typeface = shape.typeface
                    glyphPaint.textSize = shape.textSize
                    glyphPaint.color = resolve(shape.color)
                    canvas.drawText(shape.text, shape.x, shape.baseline, glyphPaint)
                }
                is MathShape.Line -> {
                    val resolved = resolve(shape.color)
                    if (shape.dashed) {
                        val stroke = maxOf(shape.thickness, 0.5f)
                        if (lastDashThickness != stroke || lastDashEffect == null) {
                            val dash = 4 * stroke
                            lastDashEffect = DashPathEffect(floatArrayOf(dash, dash), 0f)
                            lastDashThickness = stroke
                        }
                        strokePaint.color = resolved
                        strokePaint.strokeWidth = stroke
                        strokePaint.pathEffect = lastDashEffect
                        canvas.drawLine(shape.x, shape.y, shape.x + shape.width, shape.y, strokePaint)
                    } else {
                        fillPaint.color = resolved
                        canvas.drawRect(
                            shape.x,
                            shape.y - shape.thickness / 2,
                            shape.x + shape.width,
                            shape.y + shape.thickness / 2,
                            fillPaint,
                        )
                    }
                }
                is MathShape.Rect -> {
                    fillPaint.color = resolve(shape.color)
                    canvas.drawRect(shape.rect, fillPaint)
                }
                is MathShape.PathShape -> {
                    if (shape.fill) {
                        fillPaint.color = resolve(shape.color)
                        canvas.drawPath(shape.path, fillPaint)
                    } else {
                        strokePaint.color = resolve(shape.color)
                        strokePaint.strokeWidth = 1.5f
                        strokePaint.pathEffect = null
                        canvas.drawPath(shape.path, strokePaint)
                    }
                }
            }
        }
        canvas.restore()
    }
}

private fun mathFailureDetail(error: MathFailure): String = when (error) {
    is MathFailure.Parse -> if (error.detail.isBlank()) "math.parse" else "math.parse: ${error.detail}"
    is MathFailure.ResourceLimit -> if (error.detail.isBlank()) "math.resource_limit" else "math.resource_limit: ${error.detail}"
    is MathFailure.Panic -> if (error.detail.isBlank()) "math.panic" else "math.panic: ${error.detail}"
    is MathFailure.InvalidLayout -> if (error.detail.isBlank()) "math.invalid_layout" else "math.invalid_layout: ${error.detail}"
}

private data class MathCacheKey(val latex: String, val style: MathStyle, val emBits: Long)
private data class Metrik(val codepoint: Int, val width: Double, val height: Double, val depth: Double)

internal class MathSceneRenderer(
    private val assets: AssetManager,
    private val engine: MathEngine,
) : AutoCloseable {
    private val faces = ConcurrentHashMap<String, Typeface>()
    private val fallbackFaces = ConcurrentHashMap<Int, Typeface>()
    private val metrics = ConcurrentHashMap<Int, Metrik>()
    private val cache = object : LinkedHashMap<MathCacheKey, PreparedMath>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<MathCacheKey, PreparedMath>): Boolean =
            size > 160
    }
    private val failed = object : LinkedHashMap<MathCacheKey, String>(32, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<MathCacheKey, String>): Boolean =
            size > 64
    }
    private val lock = Any()
    private var measuredCodepoints = 0
    val metricsCodepointCount: Int get() = measuredCodepoints

    private fun face(key: String): Typeface = faces.getOrPut(key) {
        val name = "Fonts/KaTeX/KaTeX_$key.ttf"
        try {
            Typeface.createFromAsset(assets, name)
        } catch (_: RuntimeException) {
            Typeface.DEFAULT
        }
    }

    private fun fallbackFace(codepoint: Int, text: String, size: Float): Typeface {
        fallbackFaces[codepoint]?.let { return it }
        var face = Typeface.DEFAULT
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            try {
                val probe = Paint(Paint.ANTI_ALIAS_FLAG).apply { textSize = size }
                val run = TextRunShaper.shapeTextRun(text, 0, text.length, 0, text.length, 0f, 0f, false, probe)
                if (run.glyphCount() > 0) {
                    val font = run.getFont(0)
                    val file = font.file
                    if (file != null) {
                        face = Typeface.Builder(file)
                            .setTtcIndex(font.ttcIndex)
                            .build()
                    }
                }
            } catch (_: RuntimeException) {
                face = Typeface.DEFAULT
            }
        }
        fallbackFaces[codepoint] = face
        return face
    }

    private fun measure(codepoint: Int, size: Float): Metrik {
        metrics[codepoint]?.let { return it }
        val text = String(Character.toChars(codepoint))
        val face = fallbackFace(codepoint, text, size)
        val paint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG).apply {
            typeface = face
            textSize = size
            isLinearText = true
            hinting = Paint.HINTING_OFF
        }
        val width = paint.measureText(text)
        val bounds = Rect()
        paint.getTextBounds(text, 0, text.length, bounds)
        val value = Metrik(
            codepoint,
            (width / size).toDouble(),
            (maxOf(0f, -bounds.top.toFloat()) / size).toDouble(),
            (maxOf(0f, bounds.bottom.toFloat()) / size).toDouble(),
        )
        metrics[codepoint] = value
        measuredCodepoints += 1
        return value
    }

    fun render(latex: String, style: MathStyle, em: Float): MathRenderResult {
        if (latex.isBlank()) {
            return MathRenderResult(null, null, 0)
        }
        val key = MathCacheKey(latex, style, em.toBits().toLong())
        synchronized(lock) {
            cache[key]?.let { return MathRenderResult(it, null, 0) }
            failed[key]?.let { return MathRenderResult(null, it, 0) }
        }
        return DebugProbes.traceMainThread("mathRender") {
            try {
                var bytes = engine.render(latex, style)
                var measured = 0
                var scene = JSONObject(String(bytes, 8, bytes.size - 8, Charsets.UTF_8))
                val missing = scene.optJSONArray("missing_metrics")
                if (missing != null && missing.length() > 0) {
                    val updates = ArrayList<MathGlyphMetrics>(missing.length())
                    val fallbackScales = HashMap<Int, Double>()
                    val items = scene.optJSONArray("items")
                    if (items != null) {
                        for (index in 0 until items.length()) {
                            val item = items.optJSONObject(index) ?: continue
                            if (item.optString("kind") != "Glyph" || !item.optBoolean("fallback")) continue
                            val codepoint = item.optInt("codepoint", -1)
                            if (codepoint >= 0) fallbackScales[codepoint] = item.optDouble("scale", 1.0)
                        }
                    }
                    for (index in 0 until missing.length()) {
                        val codepoint = missing.optInt(index, -1)
                        if (codepoint < 0) continue
                        val scale = fallbackScales[codepoint] ?: 1.0
                        val value = measure(codepoint, (em * scale).toFloat())
                        updates += MathGlyphMetrics(
                            codepoint.toUInt(),
                            value.width,
                            value.height,
                            value.depth,
                        )
                    }
                    if (updates.isNotEmpty()) {
                        engine.setMetrics(updates)
                        measured = updates.size
                        bytes = engine.render(latex, style)
                        scene = JSONObject(String(bytes, 8, bytes.size - 8, Charsets.UTF_8))
                    }
                }
                val prepared = prepare(scene, em)
                synchronized(lock) { cache[key] = prepared }
                MathRenderResult(prepared, null, measured)
            } catch (error: MathFailure) {
                val reason = mathFailureDetail(error)
                synchronized(lock) { failed[key] = reason }
                MathRenderResult(null, reason, 0)
            } catch (error: Exception) {
                val detail = error.message?.takeIf { it.isNotBlank() }
                val reason = if (detail != null) "math.render_failed: $detail" else "math.render_failed"
                synchronized(lock) { failed[key] = reason }
                MathRenderResult(null, reason, 0)
            }
        }
    }

    private fun prepare(scene: JSONObject, em: Float): PreparedMath {
        val items = scene.optJSONArray("items") ?: throw IllegalArgumentException("formula scene has no items")
        val shapes = ArrayList<MathShape>(items.length())
        var glyphs = 0
        var fallbacks = 0
        var unresolved = 0
        val glyphCheckPaint = Paint(Paint.ANTI_ALIAS_FLAG)
        for (index in 0 until items.length()) {
            val item = items.getJSONObject(index)
            val x = (item.optDouble("x", 0.0) * em).toFloat()
            val y = (item.optDouble("y", 0.0) * em).toFloat()
            val color = colorOf(item.optJSONObject("color"))
            when (item.optString("kind")) {
                "Glyph" -> {
                    val codepoint = item.optInt("codepoint", -1)
                    if (codepoint < 0) {
                        unresolved += 1
                        continue
                    }
                    val scale = item.optDouble("scale", 1.0).toFloat()
                    val text = String(Character.toChars(codepoint))
                    val useFallback = item.optBoolean("fallback")
                    val face = if (useFallback) {
                        fallbackFace(codepoint, text, em * scale)
                    } else {
                        face(item.optString("font"))
                    }
                    if (useFallback) {
                        fallbacks += 1
                    }
                    glyphCheckPaint.typeface = face
                    if (!glyphCheckPaint.hasGlyph(text)) {
                        unresolved += 1
                    }
                    shapes += MathShape.Glyph(face, text, x, y, em * scale, color)
                    glyphs += 1
                }
                "Line" -> shapes += MathShape.Line(
                    x,
                    y,
                    (item.optDouble("width", 0.0) * em).toFloat(),
                    (item.optDouble("thickness", 0.0) * em).toFloat(),
                    item.optBoolean("dashed"),
                    color,
                )
                "Rect" -> shapes += MathShape.Rect(
                    RectF(x, y, x + (item.optDouble("width", 0.0) * em).toFloat(), y + (item.optDouble("height", 0.0) * em).toFloat()),
                    color,
                )
                "Path" -> {
                    val path = Path()
                    val commands = item.optJSONArray("commands")
                    if (commands != null) {
                        for (commandIndex in 0 until commands.length()) {
                            val command = commands.getJSONObject(commandIndex)
                            fun px(value: Double?) = x + ((value ?: 0.0) * em).toFloat()
                            fun py(value: Double?) = y + ((value ?: 0.0) * em).toFloat()
                            when (command.optString("kind")) {
                                "MoveTo" -> path.moveTo(px(command.optDouble("x", 0.0)), py(command.optDouble("y", 0.0)))
                                "LineTo" -> path.lineTo(px(command.optDouble("x", 0.0)), py(command.optDouble("y", 0.0)))
                                "CubicTo" -> path.cubicTo(
                                    px(command.optDouble("x1", 0.0)),
                                    py(command.optDouble("y1", 0.0)),
                                    px(command.optDouble("x2", 0.0)),
                                    py(command.optDouble("y2", 0.0)),
                                    px(command.optDouble("x", 0.0)),
                                    py(command.optDouble("y", 0.0)),
                                )
                                "QuadTo" -> path.quadTo(
                                    px(command.optDouble("x1", 0.0)),
                                    py(command.optDouble("y1", 0.0)),
                                    px(command.optDouble("x", 0.0)),
                                    py(command.optDouble("y", 0.0)),
                                )
                                "Close" -> path.close()
                            }
                        }
                    }
                    shapes += MathShape.PathShape(path, item.optBoolean("fill"), color)
                }
            }
        }
        val width = (scene.optDouble("width", 0.0) * em).toFloat()
        // 场景的 height 是基线以上的上升，depth 是基线以下的下降；y 坐标从公式顶部向下。
        val ascent = (scene.optDouble("height", 0.0) * em).toFloat()
        val descent = (scene.optDouble("depth", 0.0) * em).toFloat()
        return PreparedMath(
            width = width,
            height = ascent + descent,
            ascent = ascent,
            descent = descent,
            shapes = shapes,
            em = em,
            glyphCount = glyphs,
            fallbackGlyphCount = fallbacks,
            unresolvedGlyphCount = unresolved,
            estimatedBytes = shapes.size * 64 + glyphs * 16,
        )
    }

    private fun colorOf(color: JSONObject?): Int {
        if (color == null) return FOREGROUND
        val foreground = color.optBoolean("foreground", false)
        val rgba = color.optJSONArray("rgba")
        if (foreground || rgba == null || rgba.length() < 4) return FOREGROUND
        return Color.argb(
            (rgba.optDouble(3) * 255).toInt().coerceIn(0, 255),
            (rgba.optDouble(0) * 255).toInt().coerceIn(0, 255),
            (rgba.optDouble(1) * 255).toInt().coerceIn(0, 255),
            (rgba.optDouble(2) * 255).toInt().coerceIn(0, 255),
        )
    }

    fun clearCache() {
        synchronized(lock) {
            cache.clear()
            failed.clear()
        }
    }

    override fun close() {
        clearCache()
        try {
            engine.close()
        } catch (_: Exception) {
            Unit
        }
    }
}
