// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.macrobenchmark

import android.content.ComponentName
import android.content.Intent
import android.os.SystemClock
import androidx.benchmark.macro.MacrobenchmarkScope
import androidx.test.uiautomator.UiDevice

internal const val PACKAGE = "io.github.groundgrounder.hortus"
internal const val VAULT = "/sdcard/Documents/HortusM1Vault"
internal const val FIELD_GUIDE = "Markdown field guide.md"
internal const val LARGE_NOTE = "Synthetic/Large note.md"
internal const val README = "README.md"

internal fun noteIntent(note: String): Intent =
    Intent(Intent.ACTION_MAIN).apply {
        addCategory(Intent.CATEGORY_LAUNCHER)
        component = ComponentName(PACKAGE, "$PACKAGE.MainActivity")
        putExtra("io.github.groundgrounder.hortus.OPEN_NOTE", note)
        putExtra("io.github.groundgrounder.hortus.OPEN_VAULT", VAULT)
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }

internal fun MacrobenchmarkScope.open(note: String) {
    startActivityAndWait(noteIntent(note))
    device.waitForIdle()
}

internal fun MacrobenchmarkScope.fastSwipe(down: Boolean) {
    val width = device.displayWidth
    val height = device.displayHeight
    val top = (height * 0.22f).toInt()
    val bottom = (height * 0.82f).toInt()
    // 120 ms 快速滑动；与“设置”应用基线的滑动参数完全相同。
    device.executeShellCommand(
        "input swipe ${width / 2} ${if (down) bottom else top} ${width / 2} ${if (down) top else bottom} 120",
    )
}

internal fun settle(milliseconds: Long = 3_000) {
    SystemClock.sleep(milliseconds)
}
