// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.app.Application
import io.github.groundgrounder.hortus.app.HortusServices
import io.github.groundgrounder.hortus.core.CoreEnvironment

class HortusApplication : Application() {
    internal lateinit var services: HortusServices
        private set

    override fun onCreate() {
        super.onCreate()
        DebugProbes.init(this)
        CoreEnvironment.initialize(this)
        services = HortusServices(this)
    }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        if (::services.isInitialized) {
            services.trimMemory(level)
        }
    }

    override fun onTerminate() {
        if (::services.isInitialized) {
            services.close()
        }
        super.onTerminate()
    }
}
