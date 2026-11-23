// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.app.Instrumentation
import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent

internal object MotionPinch {
    private fun event(
        action: Int,
        downTime: Long,
        time: Long,
        points: List<Pair<Float, Float>>,
    ): MotionEvent {
        val properties = Array(points.size) { index ->
            MotionEvent.PointerProperties().apply {
                id = index
                toolType = MotionEvent.TOOL_TYPE_FINGER
            }
        }
        val coords = Array(points.size) { index ->
            MotionEvent.PointerCoords().apply {
                x = points[index].first
                y = points[index].second
                pressure = 1f
                size = 1f
            }
        }
        return MotionEvent.obtain(
            downTime,
            time,
            action,
            points.size,
            properties,
            coords,
            0,
            0,
            1f,
            1f,
            0,
            0,
            InputDevice.SOURCE_TOUCHSCREEN,
            0,
        )
    }

    fun pinch(
        instrumentation: Instrumentation,
        centerX: Float,
        centerY: Float,
        fromSpan: Float,
        toSpan: Float,
        steps: Int = 24,
    ) {
        val automation = instrumentation.uiAutomation
        val downTime = SystemClock.uptimeMillis()
        fun points(span: Float) = listOf(
            centerX - span / 2 to centerY,
            centerX + span / 2 to centerY,
        )
        var time = downTime
        val first = points(fromSpan)
        automation.injectInputEvent(
            event(MotionEvent.ACTION_DOWN, downTime, time, listOf(first[0])),
            true,
        )
        time += 16
        automation.injectInputEvent(
            event(
                MotionEvent.ACTION_POINTER_DOWN or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT),
                downTime,
                time,
                first,
            ),
            true,
        )
        for (index in 1..steps) {
            time += 16
            val span = fromSpan + (toSpan - fromSpan) * index / steps
            automation.injectInputEvent(
                event(MotionEvent.ACTION_MOVE, downTime, time, points(span)),
                true,
            )
            SystemClock.sleep(30)
        }
        time += 16
        val last = points(toSpan)
        automation.injectInputEvent(
            event(
                MotionEvent.ACTION_POINTER_UP or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT),
                downTime,
                time,
                last,
            ),
            true,
        )
        time += 16
        automation.injectInputEvent(
            event(MotionEvent.ACTION_UP, downTime, time, listOf(last[0])),
            true,
        )
        SystemClock.sleep(400)
    }

    fun doubleTap(instrumentation: Instrumentation, x: Float, y: Float) {
        val automation = instrumentation.uiAutomation
        val downTime = SystemClock.uptimeMillis()
        automation.injectInputEvent(
            event(MotionEvent.ACTION_DOWN, downTime, downTime, listOf(x to y)),
            true,
        )
        automation.injectInputEvent(
            event(MotionEvent.ACTION_UP, downTime, downTime + 40, listOf(x to y)),
            true,
        )
        SystemClock.sleep(70)
        val second = downTime + 140
        automation.injectInputEvent(
            event(MotionEvent.ACTION_DOWN, second, second, listOf(x to y)),
            true,
        )
        automation.injectInputEvent(
            event(MotionEvent.ACTION_UP, second, second + 40, listOf(x to y)),
            true,
        )
        SystemClock.sleep(400)
    }
}
