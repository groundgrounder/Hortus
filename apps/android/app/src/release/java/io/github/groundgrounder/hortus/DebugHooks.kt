// SPDX-License-Identifier: GPL-3.0-only
@file:Suppress("NOTHING_TO_INLINE")
package io.github.groundgrounder.hortus

import android.content.Intent

/** Release 构建不提供任何调试入口。 */
internal object DebugHooks {
    inline fun handlesLaunch(intent: Intent): Boolean = false
    inline fun onActivityCreate(activity: MainActivity, intent: Intent) = Unit
    inline fun onNewIntent(activity: MainActivity, intent: Intent) = Unit
}
