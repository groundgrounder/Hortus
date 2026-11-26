// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.ClipboardManager
import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import io.github.groundgrounder.hortus.reader.HitRegistry
import org.json.JSONObject
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.FileInputStream

@RunWith(AndroidJUnit4::class)
class SelectableTextTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    private fun shell(command: String): String {
        val descriptor = instrumentation.uiAutomation.executeShellCommand(command)
        return FileInputStream(descriptor.fileDescriptor).use { input ->
            input.readBytes().toString(Charsets.UTF_8)
        }
    }

    private fun waitForNote(scenario: ActivityScenario<MainActivity>, path: String, timeout: Long = 12_000) {
        val deadline = System.currentTimeMillis() + timeout
        while (System.currentTimeMillis() < deadline) {
            var current: String? = null
            scenario.onActivity { current = it.viewModel.readerState.value.content?.path }
            if (current == path) return
            Thread.sleep(100)
        }
        throw AssertionError("note did not open: $path")
    }

    @Test
    fun selectCopyLinkAndScroll() {
        shell("appops set $PACKAGE_NAME MANAGE_EXTERNAL_STORAGE allow")
        val acceptedVault = File("/sdcard/Documents/HortusM1Vault")
        val fieldGuide = File(acceptedVault, "Markdown field guide.md").isFile
        val vault: File
        val note: String
        val word: String
        val linkLabel: String
        if (fieldGuide) {
            vault = acceptedVault
            note = "Markdown field guide.md"
            word = "This original note"
            linkLabel = "Study loop"
        } else {
            vault = File(context.getExternalFilesDir(null), "selection-vault").apply { mkdirs() }
            File(vault, "Start here.md").writeText("# Start here\n\nDestination note.\n")
            File(vault, "Selection.md").writeText(
                "# Selection\n\nplainword alpha beta gamma delta\n\nGo to [[Start here]] tomorrow.\n",
            )
            note = "Selection.md"
            word = "plainword"
            linkLabel = "Start here"
        }
        val intent = Intent(context, MainActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            putExtra(MainActivity.EXTRA_OPEN_NOTE, note)
            putExtra(MainActivity.EXTRA_OPEN_VAULT, vault.absolutePath)
        }
        val scenario = ActivityScenario.launch<MainActivity>(intent)
        waitForNote(scenario, note)
        instrumentation.waitForIdleSync()
        val device = UiDevice.getInstance(instrumentation)
        val text = device.wait(Until.findObject(By.textContains(word)), 8_000)
        assertNotNull("reader text view not found", text)
        val bounds = text!!.visibleBounds
        device.swipe(
            bounds.left + 90,
            bounds.top + bounds.height() / 2,
            bounds.left + 90,
            bounds.top + bounds.height() / 2,
            900,
        )
        val copy = device.wait(Until.findObject(By.textContains("复制")), 4_000)
            ?: device.wait(Until.findObject(By.textContains("Copy")), 3_000)
        assertNotNull("selection toolbar with copy did not appear", copy)
        copy!!.click()
        device.waitForIdle()
        Thread.sleep(400)
        val clipboard = context.getSystemService(ClipboardManager::class.java)
            .primaryClip?.getItemAt(0)?.text?.toString().orEmpty()
        assertTrue("clipboard is empty", clipboard.isNotBlank())
        assertTrue("clipboard exposes markdown markers: $clipboard", !clipboard.contains("**") && !clipboard.contains("[["))

        val sourcePath = note
        val link = HitRegistry.snapshot().firstOrNull {
            it.kind == "link" && it.label.contains(linkLabel)
        }
        assertNotNull("link hit area missing", link)
        device.click(
            (link!!.x + link.width / 2f).toInt(),
            (link.y + link.height / 2f).toInt(),
        )
        val targetPath = link!!.target ?: ""
        waitForNote(scenario, targetPath)
        var linkOpened = false
        scenario.onActivity { linkOpened = it.viewModel.readerState.value.content?.path == targetPath }
        assertTrue("link did not open target", linkOpened)
        scenario.onActivity { it.onBackPressedDispatcher.onBackPressed() }
        waitForNote(scenario, sourcePath)

        val height = context.resources.displayMetrics.heightPixels
        val width = context.resources.displayMetrics.widthPixels
        device.swipe(width / 2, (height * 0.75f).toInt(), width / 2, (height * 0.25f).toInt(), 12)
        device.waitForIdle()
        Thread.sleep(500)
        var survived = false
        scenario.onActivity { survived = it.viewModel.readerState.value.content != null }
        assertTrue("reader lost content after scrolling with selection", survived)

        val report = JSONObject()
            .put("fieldGuide", fieldGuide)
            .put("toolbarFound", true)
            .put("clipboard", clipboard)
            .put("clipboardLength", clipboard.length)
            .put("markers", clipboard.contains("**") || clipboard.contains("[[") || clipboard.contains("$$"))
            .put("linkOpened", linkOpened)
            .put("scrollSurvived", survived)
        val out = File("/sdcard/Download/hortus-m1-instrument").apply { mkdirs() }
        File(out, "selection.json").writeText(report.toString(2) + "\n")
        scenario.close()
    }

    @Test
    fun copySelectionWithInlineMathIncludesLatexSource() {
        shell("appops set $PACKAGE_NAME MANAGE_EXTERNAL_STORAGE allow")
        val vault = File(context.getExternalFilesDir(null), "math-copy-vault-${System.nanoTime()}").apply {
            mkdirs()
        }
        try {
            File(vault, "MathCopy.md").writeText(
                "# MathCopy\n\n中文计算 alpha \$a^2 + b^2 = c^2\$ beta 结束。\n",
            )
            val intent = Intent(context, MainActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                putExtra(MainActivity.EXTRA_OPEN_NOTE, "MathCopy.md")
                putExtra(MainActivity.EXTRA_OPEN_VAULT, vault.absolutePath)
            }
            val scenario = ActivityScenario.launch<MainActivity>(intent)
            try {
                waitForNote(scenario, "MathCopy.md")
                instrumentation.waitForIdleSync()
                val device = UiDevice.getInstance(instrumentation)
                val paragraph = device.wait(Until.findObject(By.textContains("中文计算")), 8_000)
                assertNotNull("paragraph with inline math not found", paragraph)
                val bounds = paragraph!!.visibleBounds
                device.swipe(
                    bounds.left + 80,
                    bounds.top + bounds.height() / 2,
                    bounds.left + 80,
                    bounds.top + bounds.height() / 2,
                    900,
                )
                val selectAll = device.wait(Until.findObject(By.textContains("全选")), 3_000)
                    ?: device.wait(Until.findObject(By.textContains("Select all")), 3_000)
                assertNotNull("select all action did not appear", selectAll)
                selectAll!!.click()
                device.waitForIdle()
                val copy = device.wait(Until.findObject(By.textContains("复制")), 4_000)
                    ?: device.wait(Until.findObject(By.textContains("Copy")), 3_000)
                assertNotNull("copy action did not appear", copy)
                copy!!.click()
                device.waitForIdle()
                Thread.sleep(400)
                val clipboard = context.getSystemService(ClipboardManager::class.java)
                    .primaryClip?.getItemAt(0)?.text?.toString().orEmpty()
                assertTrue("clipboard missing Chinese prefix: $clipboard", clipboard.contains("中文计算"))
                assertTrue("clipboard missing LaTeX source: $clipboard", clipboard.contains("\$a^2 + b^2 = c^2\$"))
                assertTrue(
                    "clipboard still contains placeholder: $clipboard",
                    !clipboard.contains('\uFFFC') && !clipboard.contains('\u200B'),
                )
            } finally {
                scenario.close()
            }
        } finally {
            vault.deleteRecursively()
        }
    }

    private companion object {
        const val PACKAGE_NAME = "io.github.groundgrounder.hortus"
    }
}
