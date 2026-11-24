// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.graphics.fonts.Font
import android.graphics.fonts.SystemFonts

internal object AndroidFonts {
    fun discover(): List<Font> =
        SystemFonts.getAvailableFonts()
            .filter { it.file?.canRead() == true }
            .sortedWith(compareBy({ priority(it.file!!.name) }, { it.file!!.name }, { it.ttcIndex }))

    private fun priority(name: String): Int = when {
        name == "Roboto-Regular.ttf" -> 0
        name.startsWith("Roboto") && !name.startsWith("RobotoMono") -> 1
        name.startsWith("NotoSansCJK") || name.startsWith("NotoSansSC") -> 2
        name.contains("Mono") -> 3
        name.startsWith("NotoSans") -> 4
        else -> 5
    }
}
