// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.app.Application

class HortusApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        CoreEnvironment.initialize(this)
    }
}
