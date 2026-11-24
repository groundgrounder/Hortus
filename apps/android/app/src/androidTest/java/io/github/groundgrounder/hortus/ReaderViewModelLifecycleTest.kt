// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.groundgrounder.hortus.core.settings.LaunchExtras
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class ReaderViewModelLifecycleTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

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
    fun recreatePreservesViewModelAndContent() {
        instrumentation.uiAutomation.executeShellCommand(
            "appops set ${context.packageName} MANAGE_EXTERNAL_STORAGE allow",
        )
        val vault = File(context.getExternalFilesDir(null), "vm-lifecycle-${System.nanoTime()}").apply {
            mkdirs()
        }
        try {
            File(vault, "Alpha.md").writeText("# Alpha\n\nBody content for recreation.\n")
            val intent = Intent(context, MainActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                putExtra(LaunchExtras.EXTRA_OPEN_VAULT, vault.absolutePath)
                putExtra(LaunchExtras.EXTRA_OPEN_NOTE, "Alpha.md")
            }
            val scenario = ActivityScenario.launch<MainActivity>(intent)
            try {
                waitForNote(scenario, "Alpha.md")
                var vmBefore: ReaderViewModel? = null
                var revBefore = -1L
                scenario.onActivity { activity ->
                    vmBefore = activity.viewModel
                    revBefore = activity.viewModel.readerState.value.content?.revision ?: -1L
                }
                assertNotNull(vmBefore)
                assertTrue(revBefore >= 0L)

                scenario.recreate()
                instrumentation.waitForIdleSync()

                var vmAfter: ReaderViewModel? = null
                var noteAfter: String? = null
                var revAfter = -1L
                scenario.onActivity { activity ->
                    vmAfter = activity.viewModel
                    noteAfter = activity.viewModel.readerState.value.content?.path
                    revAfter = activity.viewModel.readerState.value.content?.revision ?: -1L
                }
                assertSame("ViewModel must survive configuration recreation", vmBefore, vmAfter)
                assertEquals("Alpha.md", noteAfter)
                assertEquals(revBefore, revAfter)
            } finally {
                scenario.close()
            }
        } finally {
            vault.deleteRecursively()
        }
    }
}
