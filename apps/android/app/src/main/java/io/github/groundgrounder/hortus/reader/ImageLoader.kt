// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.util.LruCache
import com.caverock.androidsvg.SVG
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileInputStream
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

internal data class LoadedImage(
    val bitmap: Bitmap?,
    val widthPx: Int,
    val heightPx: Int,
    val intrinsicWidth: Int,
    val intrinsicHeight: Int,
    val reason: String,
)

internal object ImageInspection {
    private val records = java.util.concurrent.ConcurrentHashMap<String, LoadedImage>()
    fun record(path: String, image: LoadedImage) { records[path] = image }
    fun snapshot(): List<Pair<String, LoadedImage>> = records.entries.map { it.key to it.value }
    fun clear() = records.clear()
}

internal object ImageLoader {
    private val cache = object : LruCache<String, LoadedImage>(64 * 1024 * 1024) {
        override fun sizeOf(key: String, value: LoadedImage): Int =
            value.bitmap?.allocationByteCount ?: 1
    }

    private fun stamp(path: String): String? {
        val file = File(path)
        return if (file.isFile && file.canRead()) "${file.lastModified()}:${file.length()}" else null
    }

    suspend fun load(
        path: String,
        targetWidthPx: Int,
        requestedWidthPx: Int? = null,
    ): LoadedImage = withContext(Dispatchers.IO) {
        MainThreadCounters.record("imageDecode") {
            loadBlocking(path, targetWidthPx, requestedWidthPx)
        }
    }

    private fun loadBlocking(path: String, targetWidthPx: Int, requestedWidthPx: Int?): LoadedImage {
        if (path.isBlank()) return LoadedImage(null, 0, 0, 0, 0, "image.missing")
        val stamp = stamp(path) ?: return LoadedImage(null, 0, 0, 0, 0, "image.missing")
        val target = max(1, min(targetWidthPx, requestedWidthPx ?: targetWidthPx))
        val key = "$path|$stamp|$target"
        cache.get(key)?.let { return it }
        val extension = path.substringAfterLast('.', "").lowercase()
        val loaded = if (extension == "svg") loadSvg(path, target) else loadBitmap(path, target)
        if (loaded.bitmap != null || loaded.reason.isNotEmpty()) cache.put(key, loaded)
        ImageInspection.record(path, loaded)
        return loaded
    }

    private fun loadSvg(path: String, target: Int): LoadedImage {
        return try {
            val svg = FileInputStream(path).use { SVG.getFromInputStream(it) }
            val width = svg.documentWidth
            val height = svg.documentHeight
            if (!width.isFinite() || !height.isFinite() || width <= 0f || height <= 0f) {
                return LoadedImage(null, 0, 0, 0, 0, "image.decode_failed")
            }
            val scale = target / width
            val outWidth = max(1, (width * scale).roundToInt())
            val outHeight = max(1, (height * scale).roundToInt())
            if (outWidth > 8192 || outHeight > 8192) {
                return LoadedImage(null, 0, 0, 0, 0, "image.too_large")
            }
            val bitmap = Bitmap.createBitmap(outWidth, outHeight, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            canvas.scale(outWidth / width, outHeight / height)
            svg.renderToCanvas(canvas)
            LoadedImage(
                bitmap,
                outWidth,
                outHeight,
                width.roundToInt(),
                height.roundToInt(),
                "",
            )
        } catch (_: Exception) {
            LoadedImage(null, 0, 0, 0, 0, "image.svg_unsupported")
        }
    }

    private fun loadBitmap(path: String, target: Int): LoadedImage {
        return try {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(path, bounds)
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
                return LoadedImage(null, 0, 0, 0, 0, "image.decode_failed")
            }
            var sample = 1
            while (bounds.outWidth / (sample * 2) >= target && bounds.outHeight / (sample * 2) >= 1) {
                sample *= 2
            }
            val options = BitmapFactory.Options().apply {
                inSampleSize = sample
                inPreferredConfig = Bitmap.Config.ARGB_8888
            }
            val decoded = BitmapFactory.decodeFile(path, options)
                ?: return LoadedImage(null, 0, 0, 0, 0, "image.decode_failed")
            val scale = min(1f, target.toFloat() / decoded.width)
            val outWidth = max(1, (decoded.width * scale).roundToInt())
            val outHeight = max(1, (decoded.height * scale).roundToInt())
            val bitmap = if (outWidth == decoded.width && outHeight == decoded.height) {
                decoded
            } else {
                Bitmap.createScaledBitmap(decoded, outWidth, outHeight, true).also {
                    if (it !== decoded) decoded.recycle()
                }
            }
            LoadedImage(
                bitmap,
                outWidth,
                outHeight,
                bounds.outWidth,
                bounds.outHeight,
                "",
            )
        } catch (_: Exception) {
            LoadedImage(null, 0, 0, 0, 0, "image.decode_failed")
        }
    }
}
