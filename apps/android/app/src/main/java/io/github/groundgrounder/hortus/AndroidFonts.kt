// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.graphics.Paint
import android.graphics.Typeface
import android.graphics.fonts.Font
import android.graphics.fonts.SystemFonts
import android.graphics.text.TextRunShaper
import android.os.Build
import io.github.groundgrounder.hortus.core.DiagramEngine
import io.github.groundgrounder.hortus.core.ScenePrimitive
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale
import kotlin.math.abs

internal object AndroidFonts {
    fun discover(): List<Font> =
        SystemFonts.getAvailableFonts()
            .filter { it.file?.canRead() == true }
            .sortedWith(compareBy({ priority(it.file!!.name) }, { it.file!!.name }, { it.ttcIndex }))

    fun compareWidths(fonts: List<Font>, report: JSONObject) {
        val samples = JSONArray()
        report.put("widthSamples", samples)
        val regular = fonts.filter { it.style.weight == 400 && it.style.slant == 0 }
        val english = checkNotNull(regular.firstOrNull { it.file!!.name.startsWith("Roboto") }) {
            "Regular Roboto system font could not be located"
        }
        val chinese = checkNotNull(chineseFont(regular)) {
            "Regular Noto Sans CJK system font could not be located"
        }

        var maxDiff = 0.0
        for ((font, texts) in listOf(
            english to listOf("Hortus Diagram 2026", "AVATAR + 0123456789"),
            chinese to listOf("笔记阅读", "图表与公式"),
        )) {
            val file = checkNotNull(font.file)
            DiagramEngine.withFontPaths(listOf(file.path)).use { engine ->
                for (text in texts) {
                    val scene = engine.render(
                        "flowchart LR\n  A -->|$text| B",
                        true, 1u, 680.0, false, emptyList(),
                    )
                    val spans = scene.primitives.filterIsInstance<ScenePrimitive.Text>()
                    val span = checkNotNull(spans.singleOrNull { it.span.text == text }?.span) {
                        "No single Rust span for $text: ${spans.map { it.span.text }}"
                    }
                    val paint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG).apply {
                        textSize = span.fontSize.toFloat()
                        textLocale = if (font == chinese) Locale.SIMPLIFIED_CHINESE else Locale.ENGLISH
                        hinting = Paint.HINTING_OFF
                        isLinearText = true
                        typeface = Typeface.Builder(file)
                            .setTtcIndex(font.ttcIndex)
                            .setFontVariationSettings("'wght' ${span.fontWeight}, 'opsz' ${span.fontSize}")
                            .build()
                    }
                    check(paint.typeface != null && paint.hasGlyph(text.first().toString())) {
                        "Android could not shape the probe text"
                    }
                    val androidWidth = paint.measureText(text).toDouble()
                    check(androidWidth.isFinite() && androidWidth > 0 &&
                        span.width.isFinite() && span.width > 0)
                    val diff = abs(span.width - androidWidth)
                    maxDiff = maxOf(maxDiff, diff)
                    samples.put(JSONObject()
                        .put("text", text)
                        .put("file", file.path)
                        .put("androidTtcIndex", font.ttcIndex)
                        .put("rustFontFamily", span.fontFamily)
                        .put("fontSizePx", span.fontSize)
                        .put("fontWeight", span.fontWeight.toInt())
                        .put("rustWidthPx", span.width)
                        .put("androidWidthPx", androidWidth)
                        .put("absDiffPx", diff))
                }
            }
        }
        report.put("maxAbsDiffPx", maxDiff)
    }

    private fun chineseFont(regular: List<Font>): Font? {
        val candidates = regular.filter {
            val name = it.file!!.name
            name.startsWith("NotoSansCJK") || name.startsWith("NotoSansSC")
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val paint = Paint().apply { textLocale = Locale.SIMPLIFIED_CHINESE }
            val glyphs = TextRunShaper.shapeTextRun("笔", 0, 1, 0, 1, 0f, 0f, false, paint)
            if (glyphs.glyphCount() > 0) {
                val shaped = glyphs.getFont(0)
                candidates.firstOrNull { it.file == shaped.file && it.ttcIndex == shaped.ttcIndex }
                    ?.let { return it }
            }
        }
        // Android 的标准 Noto Sans CJK 集合中，索引 2 为简体中文字体面。
        return candidates.firstOrNull { it.file!!.name.startsWith("NotoSansSC") }
            ?: candidates.firstOrNull { it.ttcIndex == 2 }
            ?: candidates.firstOrNull { it.localeList.getFirstMatch(arrayOf("zh-Hans")) != null }
            ?: candidates.firstOrNull()
    }

    private fun priority(name: String): Int = when {
        name == "Roboto-Regular.ttf" -> 0
        name.startsWith("Roboto") && !name.startsWith("RobotoMono") -> 1
        name.startsWith("NotoSansCJK") || name.startsWith("NotoSansSC") -> 2
        name.contains("Mono") -> 3
        name.startsWith("NotoSans") -> 4
        else -> 5
    }
}
