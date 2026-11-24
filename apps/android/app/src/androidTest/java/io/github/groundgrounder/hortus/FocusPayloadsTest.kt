// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.groundgrounder.hortus.focus.FocusPayload
import io.github.groundgrounder.hortus.focus.FocusPayloads
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class FocusPayloadsTest {
    @Before
    @After
    fun resetPayloads() {
        FocusPayloads.clear()
    }

    @Test
    fun evictsOldestPayloadFifoWhenCapacityExceeded() {
        val total = FocusPayloads.MAX_PAYLOADS + 4
        val tokens = (1..total).map { index ->
            FocusPayloads.put(
                FocusPayload(
                    vaultPath = "/vault",
                    notePath = "note-$index.md",
                    documentKey = "note-$index.md",
                    engineText = "graph TD; A$index-->B$index",
                    mmd = false,
                    ordinal = index,
                    revision = 1L,
                    dark = false,
                    collapsed = emptyList(),
                    title = "Diagram $index",
                    renderWidth = 400.0,
                ),
            )
        }
        for (evictedIndex in 0 until 4) {
            assertNull(
                "oldest payload ${evictedIndex + 1} should be evicted FIFO",
                FocusPayloads.get(tokens[evictedIndex]),
            )
        }
        for (keptIndex in 4 until total) {
            val entry = FocusPayloads.get(tokens[keptIndex])
            assertNotNull("recent payload ${keptIndex + 1} must be retained", entry)
            assertEquals("Diagram ${keptIndex + 1}", entry?.title)
        }
        val removed = FocusPayloads.remove(tokens.last())
        assertEquals("Diagram $total", removed?.title)
        assertNull(FocusPayloads.get(tokens.last()))
    }
}
