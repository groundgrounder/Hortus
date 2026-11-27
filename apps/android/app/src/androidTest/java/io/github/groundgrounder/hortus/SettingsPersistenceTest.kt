// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.groundgrounder.hortus.app.HortusServices
import io.github.groundgrounder.hortus.core.settings.DEFAULT_READER_BODY_SP
import io.github.groundgrounder.hortus.core.settings.LaunchExtras
import io.github.groundgrounder.hortus.core.settings.ThemeMode
import kotlinx.coroutines.Job
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class SettingsPersistenceTest {
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
    fun settingsPersistAndClearCachePreservesConfigAndVault() {
        instrumentation.uiAutomation.executeShellCommand(
            "appops set ${context.packageName} MANAGE_EXTERNAL_STORAGE allow",
        )
        val settingsFile = File(File(context.filesDir, "config"), "android-settings.json")
        val backup = if (settingsFile.isFile) settingsFile.readText() else null
        val vault = File(context.getExternalFilesDir(null), "settings-vault-${System.nanoTime()}").apply {
            mkdirs()
        }
        try {
            val noteBody = "# Settings Test\n\nParagraph with \$x^2\$ inline math.\n"
            val noteFile = File(vault, "Settings.md").apply { writeText(noteBody) }

            val intent = Intent(context, MainActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                putExtra(LaunchExtras.EXTRA_OPEN_VAULT, vault.absolutePath)
                putExtra(LaunchExtras.EXTRA_OPEN_NOTE, "Settings.md")
            }
            val scenario = ActivityScenario.launch<MainActivity>(intent)
            try {
                waitForNote(scenario, "Settings.md")
                var initialPx = 0f
                var themeJob: Job? = null
                var fontJob: Job? = null
                scenario.onActivity { activity ->
                    initialPx = activity.viewModel.bodySizePx.value
                    themeJob = activity.viewModel.setThemeMode(ThemeMode.DARK)
                    fontJob = activity.viewModel.setFontSizeSp(21f)
                }
                runBlocking {
                    themeJob?.join()
                    fontJob?.join()
                }
                instrumentation.waitForIdleSync()

                var updatedPx = 0f
                scenario.onActivity { activity ->
                    updatedPx = activity.viewModel.bodySizePx.value
                    assertEquals(ThemeMode.DARK, activity.viewModel.appSettings.value.themeMode)
                    assertEquals(21f, activity.viewModel.appSettings.value.fontSizeSp, 0.01f)
                    assertEquals(updatedPx, activity.viewModel.readerState.value.content?.bodySize ?: 0f, 0.5f)
                }
                assertTrue("bodySizePx did not grow ($initialPx -> $updatedPx)", updatedPx > initialPx)
                assertTrue("android-settings.json missing", settingsFile.isFile)

                val exportCache = File(context.cacheDir, "diagram-exports").apply { mkdirs() }
                val dummyExport = File(exportCache, "dummy.png").apply { writeBytes(byteArrayOf(1, 2, 3)) }
                var clearJob: Job? = null
                scenario.onActivity { activity ->
                    clearJob = activity.viewModel.clearCaches()
                }
                runBlocking { clearJob?.join() }
                assertFalse("diagram export cache was not cleared", dummyExport.exists())
                assertTrue("settings file was removed by clearCaches", settingsFile.isFile)
                assertEquals(noteBody, noteFile.readText())
                assertEquals(listOf("Settings.md"), vault.list()?.toList())

                scenario.recreate()
                instrumentation.waitForIdleSync()
                scenario.onActivity { activity ->
                    assertEquals(ThemeMode.DARK, activity.viewModel.appSettings.value.themeMode)
                    assertEquals(21f, activity.viewModel.appSettings.value.fontSizeSp, 0.01f)
                }
            } finally {
                scenario.close()
            }
        } finally {
            runBlocking {
                val services = HortusServices.get(context)
                services.settingsRepository.setThemeMode(ThemeMode.SYSTEM)
                services.settingsRepository.setFontSizeSp(DEFAULT_READER_BODY_SP)
            }
            if (backup != null) {
                settingsFile.parentFile?.mkdirs()
                settingsFile.writeText(backup)
            } else {
                settingsFile.delete()
            }
            vault.deleteRecursively()
        }
    }
}
