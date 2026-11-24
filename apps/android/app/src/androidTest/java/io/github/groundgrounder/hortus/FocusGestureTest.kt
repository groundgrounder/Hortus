// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.groundgrounder.hortus.reader.DiagramFocusActivity
import io.github.groundgrounder.hortus.reader.FocusPayload
import io.github.groundgrounder.hortus.reader.FocusPayloads
import io.github.groundgrounder.hortus.reader.FocusStateRegistry
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
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
        instrumentation.uiAutomation.executeShellCommand(
            "appops set io.github.groundgrounder.hortus MANAGE_EXTERNAL_STORAGE allow",
        )
        vault = File(context.getExternalFilesDir(null), "focus-vault").apply { mkdirs() }
        File(vault, "diagram.md").writeText("# Diagram\n")
        runBlocking {
            val core = CoreEnvironment.createWorkspaceCore(context)
            try {
                core.rememberDiagramFocus(vault.absolutePath, "diagram.md", 1u, 0.0, 0.0, 0.0)
            } finally {
                core.close()
            }
        }
        FocusStateRegistry.lastScale = 0f
        FocusStateRegistry.lastOffsetX = 0f
        FocusStateRegistry.lastOffsetY = 0f
        FocusStateRegistry.sceneWidth = 0f
        FocusStateRegistry.sceneHeight = 0f
        FocusStateRegistry.sceneFontSize = 0f
        FocusStateRegistry.unitScale = 0f
        FocusStateRegistry.drawnFontSize = 0f
        FocusStateRegistry.collapsed = emptyList()
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

    private fun writeReport(name: String, extra: JSONObject = JSONObject()) {
        val directory = File("/sdcard/Download/hortus-m1-instrument").apply { mkdirs() }
        val report = JSONObject()
            .put("scale", FocusStateRegistry.lastScale)
            .put("offsetX", FocusStateRegistry.lastOffsetX)
            .put("offsetY", FocusStateRegistry.lastOffsetY)
            .put("fitScale", FocusStateRegistry.fitScale)
            .put("sceneFontSize", FocusStateRegistry.sceneFontSize)
            .put("unitScale", FocusStateRegistry.unitScale)
            .put("drawnFontSize", FocusStateRegistry.drawnFontSize)
            .put("extra", extra)
        File(directory, name).writeText(report.toString(2) + "\n")
    }

    @Test
    fun pinchPanDoubleTapAndPersistence() {
        var scenario = launch()
        instrumentation.waitForIdleSync()
        val width = context.resources.displayMetrics.widthPixels
        val height = context.resources.displayMetrics.heightPixels
        val cx = width / 2f
        val cy = height / 2f
        val fit = FocusStateRegistry.fitScale
        assertTrue("fit scale must be positive", fit > 0f)
        assertEquals("initial view uses fit", fit, FocusStateRegistry.lastScale, 0.02f)

        MotionPinch.pinch(instrumentation, cx, cy, 120f, 900f)
        MotionPinch.pinch(instrumentation, cx, cy, 160f, 1000f)
        assertEquals("pinch reaches exactly 400%", 4f, FocusStateRegistry.lastScale, 0.01f)
        assertTrue("scene font size recorded", FocusStateRegistry.sceneFontSize > 0f)
        assertTrue("unit scale recorded", FocusStateRegistry.unitScale > 0f)
        val unitScale = FocusStateRegistry.unitScale
        assertEquals(
            "drawn font size equals scene font size × 4 × unit scale",
            FocusStateRegistry.sceneFontSize * 4f * unitScale,
            FocusStateRegistry.drawnFontSize,
            0.05f,
        )
        runCatching {
            androidx.test.uiautomator.UiDevice.getInstance(instrumentation).takeScreenshot(
                File("/sdcard/Download/hortus-m1-instrument/focus-400.png"),
            )
        }
        writeReport("focus-400.json")

        MotionPinch.pinch(instrumentation, cx, cy, 900f, 60f)
        assertEquals("pinch in reaches fit", fit, FocusStateRegistry.lastScale, 0.02f)

        val beforePanX = FocusStateRegistry.lastOffsetX
        val beforePanY = FocusStateRegistry.lastOffsetY
        instrumentation.uiAutomation.executeShellCommand(
            "input swipe $cx $cy ${cx + 220} ${cy + 160} 250",
        )
        Thread.sleep(400)
        assertTrue(
            "pan changes offsets",
            kotlin.math.abs(FocusStateRegistry.lastOffsetX - beforePanX) > 1f ||
                kotlin.math.abs(FocusStateRegistry.lastOffsetY - beforePanY) > 1f,
        )

        MotionPinch.doubleTap(instrumentation, cx, cy)
        val toggled = FocusStateRegistry.lastScale
        assertTrue(
            "double tap switches between fit and 100%: $toggled",
            kotlin.math.abs(toggled - focusScale(target = 1f)) < 0.05f ||
                kotlin.math.abs(toggled - fit) < 0.05f,
        )

        MotionPinch.pinch(instrumentation, cx, cy, 140f, 1000f)
        Thread.sleep(300)
        val persistedScale = FocusStateRegistry.lastScale
        val persistedX = FocusStateRegistry.lastOffsetX
        val persistedY = FocusStateRegistry.lastOffsetY
        assertEquals("pinch restored 400% before persistence check", 4f, persistedScale, 0.01f)
        scenario.close()
        Thread.sleep(1_200)
        FocusStateRegistry.lastScale = 0f
        scenario = launch()
        instrumentation.waitForIdleSync()
        assertEquals("scale restored", persistedScale, FocusStateRegistry.lastScale, 0.02f)
        assertEquals("offset x restored", persistedX, FocusStateRegistry.lastOffsetX, 2f)
        assertEquals("offset y restored", persistedY, FocusStateRegistry.lastOffsetY, 2f)
        writeReport("focus-restored.json")
        scenario.close()
    }

    private fun focusScale(target: Float): Float = target.coerceAtLeast(FocusStateRegistry.fitScale)
}
