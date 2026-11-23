// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent

/** Release 构建不提供任何调试入口。 */
internal object DebugHooks {
    fun onActivityCreate(activity: MainActivity, intent: Intent) = Unit
    fun onNewIntent(activity: MainActivity, intent: Intent) = Unit
}
