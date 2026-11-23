// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.graphics.Color
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.groundgrounder.hortus.core.SceneColor
import io.github.groundgrounder.hortus.core.ScenePoint
import io.github.groundgrounder.hortus.core.SceneText
import io.github.groundgrounder.hortus.reader.SceneTextMetrics
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SceneTextPaintTest {
    @Test
    fun paintKeepsColorWhenScaleIsComputed() {
        val span = SceneText(
            text = "Hortus",
            baseline = ScenePoint(0.0, 10.0),
            width = 50.0,
            fontSize = 20.0,
            fontWeight = 400u.toUShort(),
            fontFamily = "Roboto",
            color = SceneColor(0.0, 0.0, 0.0, 1.0),
        )
        val paint = SceneTextMetrics.paint(span, 2f, Color.BLACK)
        assertEquals("drawing paint must keep its resolved color", Color.BLACK, paint.color)
        assertTrue("text scale must be positive", paint.textScaleX > 0f)
    }
}
