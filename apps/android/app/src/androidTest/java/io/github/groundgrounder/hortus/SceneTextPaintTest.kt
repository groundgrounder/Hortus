// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus

import android.graphics.Color
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.groundgrounder.hortus.core.SceneColor
import io.github.groundgrounder.hortus.core.ScenePoint
import io.github.groundgrounder.hortus.core.SceneText
import io.github.groundgrounder.hortus.reader.render.SceneTextMetrics
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

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

    @Test
    fun paintCacheIsBoundedAndThreadSafe() {
        val palette = intArrayOf(Color.BLACK, Color.RED, Color.BLUE, Color.DKGRAY, Color.MAGENTA)
        val pool = Executors.newFixedThreadPool(8)
        try {
            val tasks = (0 until 8).map { worker ->
                Callable {
                    for (step in 0 until 600) {
                        val size = 10.0 + ((worker * 600 + step) % 300) * 0.1
                        val weight = (300 + ((worker + step) % 5) * 100).toUShort()
                        val family = if ((worker + step) % 2 == 0) "Roboto" else "serif"
                        val color = palette[(worker + step) % palette.size]
                        val span = SceneText(
                            text = "Item-${worker}-${step}",
                            baseline = ScenePoint(0.0, 12.0),
                            width = 64.0 + (step % 17),
                            fontSize = size,
                            fontWeight = weight,
                            fontFamily = family,
                            color = SceneColor(0.0, 0.0, 0.0, 1.0),
                        )
                        val paint = SceneTextMetrics.paint(span, 1.5f + (worker % 3) * 0.25f, color)
                        assertEquals("concurrent paint must retain caller color", color, paint.color)
                        assertTrue("concurrent textScaleX must stay positive", paint.textScaleX > 0f)
                    }
                }
            }
            val futures = pool.invokeAll(tasks, 30, TimeUnit.SECONDS)
            for (future in futures) {
                future.get()
            }
        } finally {
            pool.shutdownNow()
        }
        val (scales, paints) = SceneTextMetrics.cacheSizes()
        assertTrue("scale cache must stay bounded at <= 2048, got $scales", scales in 1..2048)
        assertTrue("paint cache must stay bounded at <= 64, got $paints", paints in 1..64)
    }
}
