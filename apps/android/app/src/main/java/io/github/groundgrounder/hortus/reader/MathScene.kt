// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.res.AssetManager
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.RectF
import android.graphics.Typeface
import android.graphics.text.TextRunShaper
import android.os.Build
import io.github.groundgrounder.hortus.core.MathFailure
import io.github.groundgrounder.hortus.core.MathGlyphMetrics
import io.github.groundgrounder.hortus.core.MathStyle
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap

private const val FOREGROUND = Int.MIN_VALUE

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
        canvas.save()
        canvas.translate(originX, originY)
        for (shape in shapes) {
            when (shape) {
                is MathShape.Glyph -> {
                    val paint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG).apply {
                        typeface = shape.typeface
                        textSize = shape.textSize
                        isLinearText = true
                        hinting = Paint.HINTING_OFF
                        color = resolve(shape.color)
                    }
                    canvas.drawText(shape.text, shape.x, shape.baseline, paint)
                }
                is MathShape.Line -> {
                    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                        color = resolve(shape.color)
                        strokeWidth = maxOf(shape.thickness, 0.5f)
                        style = Paint.Style.FILL_AND_STROKE
                    }
                    if (shape.dashed) {
                        paint.style = Paint.Style.STROKE
                        val dash = 4 * maxOf(shape.thickness, 0.5f)
                        paint.pathEffect = android.graphics.DashPathEffect(floatArrayOf(dash, dash), 0f)
                        canvas.drawLine(shape.x, shape.y, shape.x + shape.width, shape.y, paint)
                    } else {
                        canvas.drawRect(
                            shape.x,
                            shape.y - shape.thickness / 2,
                            shape.x + shape.width,
                            shape.y + shape.thickness / 2,
                            Paint(Paint.ANTI_ALIAS_FLAG).apply { color = resolve(shape.color) },
                        )
                    }
                }
                is MathShape.Rect -> canvas.drawRect(
                    shape.rect,
                    Paint(Paint.ANTI_ALIAS_FLAG).apply { color = resolve(shape.color) },
                )
                is MathShape.PathShape -> {
                    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                        color = resolve(shape.color)
                        style = if (shape.fill) Paint.Style.FILL else Paint.Style.STROKE
                        strokeWidth = 1.5f
                    }
                    canvas.drawPath(shape.path, paint)
                }
            }
        }
        canvas.restore()
    }
}

private data class MathCacheKey(val latex: String, val style: MathStyle, val emBits: Long)
private data class Metrik(val codepoint: Int, val width: Double, val height: Double, val depth: Double)

internal class MathSceneRenderer(
    private val assets: AssetManager,
    private val engine: io.github.groundgrounder.hortus.core.MathEngine,
) {
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
        return MainThreadCounters.record("mathRender") {
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
                val reason = error.toString()
                synchronized(lock) { failed[key] = reason }
                MathRenderResult(null, reason, 0)
            } catch (error: Exception) {
                val reason = error.message ?: error.javaClass.simpleName
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
                    if (!Paint(Paint.ANTI_ALIAS_FLAG).apply { typeface = face }.hasGlyph(text)) {
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
        val height = (scene.optDouble("height", 0.0) * em).toFloat()
        val depth = (scene.optDouble("depth", 0.0) * em).toFloat()
        return PreparedMath(
            width = width,
            height = height,
            ascent = height,
            descent = depth,
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
}
