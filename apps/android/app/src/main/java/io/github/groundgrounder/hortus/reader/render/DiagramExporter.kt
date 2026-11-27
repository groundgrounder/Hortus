// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.render

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.pdf.PdfDocument
import android.net.Uri
import androidx.core.content.FileProvider
import io.github.groundgrounder.hortus.core.SceneData
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import kotlin.math.ceil

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

    suspend fun png(scene: SceneData, dark: Boolean): ByteArray = withContext(Dispatchers.IO) {
        encodePng(scene, dark)
    }

    suspend fun pdf(scene: SceneData, dark: Boolean): ByteArray = withContext(Dispatchers.IO) {
        encodePdf(scene, dark)
    }

    fun write(context: Context, uri: Uri, bytes: ByteArray) {
        context.contentResolver.openOutputStream(uri, "w")?.use { it.write(bytes) }
            ?: throw IllegalStateException("output stream unavailable")
    }

    /** 分享用的临时文件上限；超出时按最后修改时间删除最旧的文件。 */
    const val SHARE_CACHE_MAX_BYTES = 24L * 1024 * 1024
    const val SHARE_CACHE_MAX_FILES = 32

    fun shareDirectory(context: Context): File = File(context.cacheDir, "diagram-exports")

    fun shareFile(context: Context, name: String, bytes: ByteArray): File {
        val directory = shareDirectory(context)
        directory.mkdirs()
        val file = File(directory, name)
        val temporary = File(directory, ".$name.tmp")
        temporary.writeBytes(bytes)
        try {
            Files.move(
                temporary.toPath(),
                file.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
                StandardCopyOption.ATOMIC_MOVE,
            )
        } catch (_: Exception) {
            Files.move(
                temporary.toPath(),
                file.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
            )
        }
        cleanupShareCache(context)
        return file
    }

    fun cleanupShareCache(context: Context) {
        val directory = shareDirectory(context)
        val files = directory.listFiles()?.filter { it.isFile }?.sortedBy { it.lastModified() } ?: return
        var total = files.sumOf { it.length() }
        var count = files.size
        for (file in files) {
            if (total <= SHARE_CACHE_MAX_BYTES && count <= SHARE_CACHE_MAX_FILES) break
            total -= file.length()
            count -= 1
            file.delete()
        }
    }

    fun shareUri(context: Context, file: File): Uri =
        FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", file)

    fun shareIntent(context: Context, file: File, format: DiagramExportFormat, title: String): Intent =
        Intent(Intent.ACTION_SEND).apply {
            type = format.mime
            putExtra(Intent.EXTRA_STREAM, shareUri(context, file))
            putExtra(Intent.EXTRA_TITLE, title)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }

    fun readBack(context: Context, uri: Uri): ByteArray? = try {
        context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
    } catch (_: Exception) {
        null
    }
}
