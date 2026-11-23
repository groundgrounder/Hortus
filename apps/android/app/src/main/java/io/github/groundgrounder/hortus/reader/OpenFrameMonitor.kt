// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.os.Handler
import android.os.Looper
import android.view.FrameMetrics
import android.view.Window
import java.util.concurrent.atomic.AtomicLong

/** 用 FrameMetrics 记录一次打开窗口内的主线程帧时长（纳秒换算为毫秒）。 */
internal object OpenFrameMonitor {
    val maxFrameMs = AtomicLong(0)
    val rawMaxFrameMs = AtomicLong(0)
    private val main = Handler(Looper.getMainLooper())
    @Volatile private var running = false
    @Volatile private var installed = false
    @Volatile private var skipFrames = 0

    fun install(window: Window) {
        if (installed) return
        installed = true
        try {
            window.addOnFrameMetricsAvailableListener({ _, metrics, _ ->
                if (!running) return@addOnFrameMetricsAvailableListener
                val duration = (
                    metrics.getMetric(FrameMetrics.LAYOUT_MEASURE_DURATION) +
                        metrics.getMetric(FrameMetrics.DRAW_DURATION)
                    ) / 1_000_000.0
                if (duration.isFinite() && duration > 0.0) {
                    rawMaxFrameMs.updateAndGet { current -> maxOf(current, duration.toLong()) }
                    if (skipFrames > 0) {
                        skipFrames -= 1
                        return@addOnFrameMetricsAvailableListener
                    }
                    maxFrameMs.updateAndGet { current -> maxOf(current, duration.toLong()) }
                }
            }, main)
        } catch (_: RuntimeException) {
            installed = false
        }
    }

    fun begin() {
        maxFrameMs.set(0)
        rawMaxFrameMs.set(0)
        skipFrames = 10
        running = true
        main.removeCallbacksAndMessages("open-frame-monitor")
        main.postDelayed({ running = false }, 3_000)
    }

    fun end() {
        running = false
    }
}
