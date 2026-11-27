// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.macrobenchmark

import androidx.benchmark.macro.BaselineProfileMode
import androidx.benchmark.macro.CompilationMode
import androidx.benchmark.macro.FrameTimingMetric
import androidx.benchmark.macro.junit4.MacrobenchmarkRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class OpenNoteBenchmark {
    @get:Rule
    val rule = MacrobenchmarkRule()

    @Before
    fun prepareDevice() {
        val device = InstrumentationRegistry.getInstrumentation().uiAutomation
        device.executeShellCommand("appops set $PACKAGE MANAGE_EXTERNAL_STORAGE allow")
        settle(1_000)
    }

    @Test
    fun openFieldGuideInProcessBaselineProfile() = measure(CompilationMode.Partial(BaselineProfileMode.Require))

    @Test
    fun openFieldGuideInProcessNoProfile() = measure(CompilationMode.None())

    private fun measure(mode: CompilationMode) = rule.measureRepeated(
        packageName = PACKAGE,
        metrics = listOf(FrameTimingMetric()),
        compilationMode = mode,
        iterations = 5,
        setupBlock = {
            open(README)
            settle(3_200)
        },
    ) {
        open(FIELD_GUIDE)
    }
}
