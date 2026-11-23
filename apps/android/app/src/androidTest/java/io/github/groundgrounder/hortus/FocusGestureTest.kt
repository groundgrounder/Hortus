// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.test.uiautomator.UiDevice
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.groundgrounder.hortus.reader.DiagramFocusActivity
import io.github.groundgrounder.hortus.reader.FocusPayload
import io.github.groundgrounder.hortus.reader.FocusPayloads
import io.github.groundgrounder.hortus.reader.FocusStateRegistry
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class FocusGestureTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private lateinit var vault: File

    @Before
    fun setUp() {
        vault = File(context.getExternalFilesDir(null), "focus-vault").apply { mkdirs() }
        File(vault, "diagram.md").writeText("# Diagram\n")
        FocusStateRegistry.lastScale = 0f
        FocusStateRegistry.lastOffsetX = 0f
        FocusStateRegistry.lastOffsetY = 0f
        FocusStateRegistry.sceneWidth = 0f
        FocusStateRegistry.sceneHeight = 0f
    }

    private fun launch(): ActivityScenario<DiagramFocusActivity> {
        val payload = FocusPayload(
            vaultPath = vault.absolutePath,
            notePath = "diagram.md",
            documentKey = "focus-test",
            engineText = "flowchart LR\n  A[Alpha] --> B[Beta]\n  B --> C[Gamma]\n",
            mmd = true,
            ordinal = 1,
            revision = 1,
            dark = false,
            collapsed = emptyList(),
            title = "Focus test",
            renderWidth = 680.0,
        )
        val token = FocusPayloads.put(payload)
        val scenario = ActivityScenario.launch<DiagramFocusActivity>(DiagramFocusActivity.intent(context, token))
        waitForScene()
        return scenario
    }

    private fun waitForScene() {
        val deadline = System.currentTimeMillis() + 8_000
        while (System.currentTimeMillis() < deadline) {
            if (FocusStateRegistry.lastScale > 0f) return
            Thread.sleep(80)
        }
        assertTrue("focus scene did not render", FocusStateRegistry.lastScale > 0f)
    }

    @Test
    fun pinchPanDoubleTapAndPersistence() {
        var scenario = launch()
        instrumentation.waitForIdleSync()
        val width = context.resources.displayMetrics.widthPixels
        val height = context.resources.displayMetrics.heightPixels
        val cx = width / 2f
        val cy = height / 2f
        val initialScale = FocusStateRegistry.lastScale
        assertTrue("initial fit scale must be positive", initialScale > 0f)

        MotionPinch.pinch(instrumentation, cx, cy, 120f, 900f)
        assertTrue("pinch out should exceed 300%: ${FocusStateRegistry.lastScale}",
            FocusStateRegistry.lastScale >= 3f)

        MotionPinch.pinch(instrumentation, cx, cy, 900f, 80f)
        assertTrue("pinch in should reach 50% or less: ${FocusStateRegistry.lastScale}",
            FocusStateRegistry.lastScale <= 0.5f)

        val beforePanX = FocusStateRegistry.lastOffsetX
        val beforePanY = FocusStateRegistry.lastOffsetY
        instrumentation.uiAutomation.executeShellCommand(
            "input swipe $cx $cy ${cx + 220} ${cy + 160} 250",
        )
        Thread.sleep(400)
        assertTrue(
            "pan should change offsets",
            kotlin.math.abs(FocusStateRegistry.lastOffsetX - beforePanX) > 1f ||
                kotlin.math.abs(FocusStateRegistry.lastOffsetY - beforePanY) > 1f,
        )

        MotionPinch.doubleTap(instrumentation, cx, cy)
        val toggled = FocusStateRegistry.lastScale
        assertTrue("double tap should switch between fit and 100%: $toggled",
            kotlin.math.abs(toggled - 1f) < 0.05f || kotlin.math.abs(toggled - initialScale) < 0.05f)

        MotionPinch.pinch(instrumentation, cx, cy, 120f, 900f)
        MotionPinch.pinch(instrumentation, cx, cy, 200f, 1200f)
        Thread.sleep(300)
        val scale = FocusStateRegistry.lastScale
        assertTrue("scale should be at least 300%: $scale", scale >= 3f)
        if (scale >= 4f) {
            runCatching {
                UiDevice.getInstance(instrumentation).takeScreenshot(
                    java.io.File(context.getExternalFilesDir(null), "m1-focus-400.png"),
                )
            }
            val base = FocusStateRegistry.baseLargestFontPx
            val drawn = FocusStateRegistry.largestFontPx
            assertTrue("vector text must scale with zoom", base > 0f)
            assertTrue("drawn font size should be base × 4: $drawn", kotlin.math.abs(drawn - base * 4f) < 0.05f)
        }

        val persistedScale = FocusStateRegistry.lastScale
        val persistedX = FocusStateRegistry.lastOffsetX
        val persistedY = FocusStateRegistry.lastOffsetY
        scenario.close()
        Thread.sleep(1_200)
        FocusStateRegistry.lastScale = 0f
        scenario = launch()
        instrumentation.waitForIdleSync()
        assertEquals("scale restored", persistedScale, FocusStateRegistry.lastScale, 0.02f)
        assertEquals("offset x restored", persistedX, FocusStateRegistry.lastOffsetX, 2f)
        assertEquals("offset y restored", persistedY, FocusStateRegistry.lastOffsetY, 2f)
        scenario.close()
    }
}
