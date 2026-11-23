// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.app.Application
import android.content.pm.ApplicationInfo
import io.github.groundgrounder.hortus.reader.Inspection

class HortusApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        // 验收数据只在可调试的构建里收集；Release 与 benchmark 不承担这份开销。
        Inspection.recordingEnabled =
            (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
        CoreEnvironment.initialize(this)
    }
}
