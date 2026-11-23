// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.res.Configuration
import android.graphics.BitmapFactory
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import io.github.groundgrounder.hortus.MainActivity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.regex.Pattern

internal object DebugReadingExports {
    suspend fun exportAll(activity: MainActivity, name: String, expectedNote: String? = null) {
        var waited = 0L
        while (waited < 12_000) {
            val current = activity.controller.state.content
            if (current != null && (expectedNote == null || current.path == expectedNote) &&
                !activity.controller.state.loading
            ) {
                break
            }
            delay(100)
            waited += 100
        }
        val content = activity.controller.state.content ?: return
        val dark = (activity.resources.configuration.uiMode and
            Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
        val repository = activity.controller.repository()
        val directory = checkNotNull(activity.getExternalFilesDir("exports")) { "external files unavailable" }
        directory.mkdirs()
        val results = JSONArray()
        for (item in content.diagramItems) {
            val collapsed = withContext(Dispatchers.IO) {
                repository.collapsedNodes(item.documentKey, item.ordinal)
            }
            val render = withContext(Dispatchers.IO) {
                repository.renderDiagram(
                    documentKey = item.documentKey,
                    engineText = item.engineText,
                    mmd = item.mmd,
                    ordinal = item.ordinal,
                    width = content.width.toDouble(),
                    dark = dark,
                    collapsed = collapsed,
                    revision = content.revision,
                )
            }
            val scene = render.scene
            if (scene == null) {
                results.put(JSONObject()
                    .put("ordinal", item.ordinal)
                    .put("type", item.diagramType)
                    .put("error", render.failure))
                continue
            }
            for (format in DiagramExportFormat.entries) {
                val fileName = DiagramExporter.defaultName(item.documentKey, item.ordinal, format)
                val bytes = withContext(Dispatchers.IO) {
                    when (format) {
                        DiagramExportFormat.Svg -> repository.exportSvg(
                            item.engineText,
                            item.mmd,
                            item.ordinal,
                            scene.width,
                            dark,
                            collapsed,
                        ).toByteArray(Charsets.UTF_8)
                        DiagramExportFormat.Png -> DiagramExporter.encodePng(scene, dark)
                        DiagramExportFormat.Pdf -> DiagramExporter.encodePdf(scene, dark)
                    }
                }
                val file = DiagramExporter.writeDebugFile(activity, fileName, bytes)
                val extra = JSONObject()
                    .put("sceneWidth", scene.width)
                    .put("sceneHeight", scene.height)
                    .put("dark", dark)
                when (format) {
                    DiagramExportFormat.Svg -> {
                        val text = bytes.toString(Charsets.UTF_8)
                        val matcher = Pattern.compile(
                            "viewBox\\s*=\\s*\"\\s*([-0-9.]+)[ ,]+([-0-9.]+)[ ,]+([-0-9.]+)[ ,]+([-0-9.]+)\\s*\"",
                        ).matcher(text)
                        if (matcher.find()) {
                            extra.put("viewBox", JSONArray(listOf(
                                matcher.group(1).toDouble(),
                                matcher.group(2).toDouble(),
                                matcher.group(3).toDouble(),
                                matcher.group(4).toDouble(),
                            )))
                        }
                        extra.put("hasScript", text.contains("<script", ignoreCase = true))
                        extra.put("hasForeignObject", text.contains("foreignObject", ignoreCase = true))
                        extra.put("validXmlStart", text.trimStart().startsWith("<?xml") || text.trimStart().startsWith("<svg"))
                    }
                    DiagramExportFormat.Png -> {
                        val options = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                        BitmapFactory.decodeFile(file.absolutePath, options)
                        extra.put("pngWidth", options.outWidth)
                        extra.put("pngHeight", options.outHeight)
                    }
                    DiagramExportFormat.Pdf -> {
                        extra.put("pdfHeader", bytes.copyOfRange(0, minOf(5, bytes.size)).toString(Charsets.US_ASCII))
                        val mediaBox = Regex("/MediaBox\\s*\\[\\s*([-0-9.]+)\\s+([-0-9.]+)\\s+([-0-9.]+)\\s+([-0-9.]+)\\s*]")
                            .find(bytes.toString(Charsets.ISO_8859_1))?.groupValues
                        if (mediaBox != null) {
                            extra.put("mediaBox", JSONArray(mediaBox.drop(1).map { it.toDouble() }))
                        }
                        try {
                            ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { descriptor ->
                                PdfRenderer(descriptor).use { renderer ->
                                    extra.put("pdfPages", renderer.pageCount)
                                    if (renderer.pageCount > 0) {
                                        renderer.openPage(0).use { page ->
                                            extra.put("pdfPageWidth", page.width)
                                            extra.put("pdfPageHeight", page.height)
                                        }
                                    }
                                }
                            }
                        } catch (error: Exception) {
                            extra.put("pdfError", error.message)
                        }
                    }
                }
                Inspection.recordExport(format.extension, fileName, bytes.size, file.absolutePath, extra)
                results.put(JSONObject()
                    .put("ordinal", item.ordinal)
                    .put("type", item.diagramType)
                    .put("format", format.extension)
                    .put("name", fileName)
                    .put("path", file.absolutePath)
                    .put("bytes", bytes.size)
                    .put("extra", extra))
            }
        }
        val report = JSONObject()
            .put("note", content.path)
            .put("title", content.title)
            .put("dark", dark)
            .put("exports", results)
        withContext(Dispatchers.IO) {
            val file = File(checkNotNull(activity.getExternalFilesDir(null)), name)
            val temporary = File(file.parentFile, ".$name.tmp")
            temporary.writeText(report.toString(2) + "\n")
            temporary.setReadable(true, false)
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
            )
        }
    }
}
