// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.groundgrounder.hortus.reader.DiagramExporter
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class ShareCacheTest {
    private companion object {
        const val PACKAGE_NAME = "io.github.groundgrounder.hortus"
    }

    private val context = InstrumentationRegistry.getInstrumentation().targetContext

    @Test
    fun repeatedSharesStayWithinCachePolicy() {
        InstrumentationRegistry.getInstrumentation().uiAutomation.executeShellCommand(
            "appops set $PACKAGE_NAME MANAGE_EXTERNAL_STORAGE allow",
        )
        val directory = DiagramExporter.shareDirectory(context)
        directory.deleteRecursively()
        val unit = ByteArray(1024 * 1024) { (it % 251).toByte() }
        repeat(20) { index ->
            DiagramExporter.shareFile(context, "diagram-$index.png", unit)
        }
        val files = directory.listFiles()?.filter { it.isFile }.orEmpty()
        val total = files.sumOf { it.length() }
        assertTrue(
            "share cache bytes $total exceed ${DiagramExporter.SHARE_CACHE_MAX_BYTES}",
            total <= DiagramExporter.SHARE_CACHE_MAX_BYTES,
        )
        assertTrue(
            "share cache files ${files.size} exceed ${DiagramExporter.SHARE_CACHE_MAX_FILES}",
            files.size <= DiagramExporter.SHARE_CACHE_MAX_FILES,
        )
        val checked = DiagramExporter.shareFile(context, "diagram-check.png", unit)
        val uri = DiagramExporter.shareUri(context, checked)
        val readBack = DiagramExporter.readBack(context, uri)
        assertTrue("content URI must return the same bytes", readBack?.contentEquals(unit) == true)
        val report = JSONObject()
            .put("files", files.size)
            .put("bytes", total)
            .put("maxBytes", DiagramExporter.SHARE_CACHE_MAX_BYTES)
            .put("maxFiles", DiagramExporter.SHARE_CACHE_MAX_FILES)
            .put("uri", uri.toString())
            .put("contentBytes", readBack?.size ?: -1)
            .put("contentMatches", readBack?.contentEquals(unit) == true)
        val out = File("/sdcard/Download/hortus-m1-instrument").apply { mkdirs() }
        File(out, "share-cache.json").writeText(report.toString(2) + "\n")
    }
}
