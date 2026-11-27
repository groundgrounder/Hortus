// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.macrobenchmark

import androidx.benchmark.macro.junit4.BaselineProfileRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class BaselineProfileGenerator {
    @get:Rule
    val rule = BaselineProfileRule()

    @Before
    fun prepareDevice() {
        InstrumentationRegistry.getInstrumentation().uiAutomation
            .executeShellCommand("appops set $PACKAGE MANAGE_EXTERNAL_STORAGE allow")
    }

    @Test
    fun generate() = rule.collect(
        packageName = PACKAGE,
        includeInStartupProfile = true,
    ) {
        pressHome()
        open(README)
        settle(800)
        open(FIELD_GUIDE)
        settle(800)
        repeat(2) {
            fastSwipe(down = true)
            fastSwipe(down = false)
        }
        settle(500)
    }
}
