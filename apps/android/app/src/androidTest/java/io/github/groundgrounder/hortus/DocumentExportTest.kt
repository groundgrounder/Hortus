// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.FileInputStream

@RunWith(AndroidJUnit4::class)
class DocumentExportTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    private fun shell(command: String): String {
        val descriptor = instrumentation.uiAutomation.executeShellCommand(command)
        return FileInputStream(descriptor.fileDescriptor).use { input ->
            input.readBytes().toString(Charsets.UTF_8)
        }
    }

    @Test
    fun savePngThroughCreateDocument() {
        shell("rm -f /sdcard/Download/m1-document-export.png")
        context.startActivity(
            Intent(context, DebugExportActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        val device = UiDevice.getInstance(instrumentation)
        val save = device.wait(
            Until.findObject(By.textContains("Save")),
            12_000,
        ) ?: device.wait(Until.findObject(By.textContains("保存")), 5_000)
        ?: device.wait(Until.findObject(By.descContains("Save")), 3_000)
        assertTrue("system document picker did not show a save action", save != null)
        save!!.click()
        device.waitForIdle()
        val deadline = System.currentTimeMillis() + 12_000
        var bytes = ""
        while (System.currentTimeMillis() < deadline) {
            bytes = shell("wc -c < /sdcard/Download/m1-document-export.png 2>/dev/null").trim()
            if (bytes.toLongOrNull()?.let { it > 0 } == true) break
            Thread.sleep(200)
        }
        val size = Regex("\\d+").find(bytes)?.value?.toLongOrNull() ?: 0L
        assertTrue("exported PNG is missing or empty: $bytes", size > 0)
        val header = shell("od -An -tx1 -N8 /sdcard/Download/m1-document-export.png").trim()
        assertTrue("exported file is not a PNG: $header", header.startsWith("89 50 4e 47"))
    }
}
