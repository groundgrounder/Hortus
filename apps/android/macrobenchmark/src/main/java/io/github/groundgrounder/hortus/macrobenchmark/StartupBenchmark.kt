// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.macrobenchmark

import androidx.benchmark.macro.BaselineProfileMode
import androidx.benchmark.macro.CompilationMode
import androidx.benchmark.macro.StartupMode
import androidx.benchmark.macro.StartupTimingMetric
import androidx.benchmark.macro.junit4.MacrobenchmarkRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class StartupBenchmark {
    @get:Rule
    val rule = MacrobenchmarkRule()

    @Before
    fun prepareDevice() {
        val device = InstrumentationRegistry.getInstrumentation().uiAutomation
        device.executeShellCommand("appops set $PACKAGE MANAGE_EXTERNAL_STORAGE allow")
        settle(1_000)
    }

    @Test
    fun coldStartWithNoteBaselineProfile() = measure(CompilationMode.Partial(BaselineProfileMode.Require))

    @Test
    fun coldStartWithNoteNoProfile() = measure(CompilationMode.None())

    private fun measure(mode: CompilationMode) = rule.measureRepeated(
        packageName = PACKAGE,
        metrics = listOf(StartupTimingMetric()),
        compilationMode = mode,
        startupMode = StartupMode.COLD,
        iterations = 5,
        setupBlock = {
            pressHome()
            settle(800)
        },
    ) {
        startActivityAndWait(noteIntent(FIELD_GUIDE))
        device.waitForIdle()
        // 等待 reportFullyDrawn 落到跟踪窗口内，timeToFullDisplayMs 才有值。
        settle(1_500)
    }
}
