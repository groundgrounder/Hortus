// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.pdf.PdfDocument
import io.github.groundgrounder.hortus.core.SceneData
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.URI
import kotlin.math.ceil
import kotlin.math.roundToInt

enum class DiagramExportFormat(val extension: String, val mime: String) {
    Svg("svg", "image/svg+xml"),
    Png("png", "image/png"),
    Pdf("pdf", "application/pdf"),
}

object DiagramExporter {
    fun defaultName(notePath: String, ordinal: Int, format: DiagramExportFormat): String {
        val base = notePath.substringAfterLast('/').substringBeforeLast('.').trim()
        val name = base.ifEmpty { "Diagram" }
        return "$name-diagram-$ordinal.${format.extension}"
    }

    fun encodePng(scene: SceneData, dark: Boolean): ByteArray {
        val width = ceil(scene.width * 2).toInt().coerceAtLeast(1)
        val height = ceil(scene.height * 2).toInt().coerceAtLeast(1)
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        canvas.drawColor(Color.TRANSPARENT)
        SceneRenderer.draw(
            canvas,
            scene,
            if (dark) Color.WHITE else Color.BLACK,
            zoom = 2f,
        )
        val output = ByteArrayOutputStream()
        bitmap.compress(Bitmap.CompressFormat.PNG, 100, output)
        bitmap.recycle()
        return output.toByteArray()
    }

    fun encodePdf(scene: SceneData, dark: Boolean): ByteArray {
        val width = ceil(scene.width).toInt().coerceAtLeast(1)
        val height = ceil(scene.height).toInt().coerceAtLeast(1)
        val document = PdfDocument()
        val page = document.startPage(PdfDocument.PageInfo.Builder(width, height, 1).create())
        SceneRenderer.draw(
            page.canvas,
            scene,
            if (dark) Color.WHITE else Color.BLACK,
        )
        document.finishPage(page)
        val output = ByteArrayOutputStream()
        document.writeTo(output)
        document.close()
        return output.toByteArray()
    }

    internal suspend fun svg(
        repository: ReaderRepository,
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
    ): ByteArray = withContext(Dispatchers.IO) {
        repository.exportSvg(engineText, mmd, ordinal, width, dark, collapsed)
            .toByteArray(Charsets.UTF_8)
    }

    suspend fun png(scene: SceneData, dark: Boolean): ByteArray = withContext(Dispatchers.IO) {
        encodePng(scene, dark)
    }

    suspend fun pdf(scene: SceneData, dark: Boolean): ByteArray = withContext(Dispatchers.IO) {
        encodePdf(scene, dark)
    }

    fun write(context: Context, uri: android.net.Uri, bytes: ByteArray) {
        context.contentResolver.openOutputStream(uri, "w")?.use { it.write(bytes) }
            ?: throw IllegalStateException("output stream unavailable")
    }

    fun writeDebugFile(context: Context, name: String, bytes: ByteArray): File {
        val directory = checkNotNull(context.getExternalFilesDir("exports")) { "external files unavailable" }
        directory.mkdirs()
        val file = File(directory, name)
        val temporary = File(directory, ".$name.tmp")
        temporary.writeBytes(bytes)
        check(temporary.setReadable(true, false)) { "cannot make export readable" }
        try {
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
                java.nio.file.StandardCopyOption.ATOMIC_MOVE,
            )
        } catch (_: Exception) {
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
            )
        }
        return file
    }
}

data class FocusPayload(
    val vaultPath: String,
    val notePath: String,
    val documentKey: String,
    val engineText: String,
    val mmd: Boolean,
    val ordinal: Int,
    val revision: Long,
    val dark: Boolean,
    val collapsed: List<String>,
    val title: String,
    val renderWidth: Double,
)

object FocusPayloads {
    private var next = 1
    private val payloads = HashMap<Int, FocusPayload>()

    fun put(payload: FocusPayload): Int {
        synchronized(this) {
            val token = next++
            payloads[token] = payload
            if (payloads.size > 8) payloads.keys.firstOrNull()?.let(payloads::remove)
            return token
        }
    }

    fun get(token: Int): FocusPayload? = synchronized(this) { payloads[token] }

    fun remove(token: Int) = synchronized(this) { payloads.remove(token) }
}

object FocusStateRegistry {
    @Volatile var lastScale: Float = 0f
    @Volatile var lastOffsetX: Float = 0f
    @Volatile var lastOffsetY: Float = 0f
    @Volatile var sceneWidth: Float = 0f
    @Volatile var sceneHeight: Float = 0f
    @Volatile var largestFontPx: Float = 0f
    @Volatile var baseLargestFontPx: Float = 0f
    @Volatile var pointerEvents: Int = 0
    @Volatile var gestureDistance: Float = 0f
}
