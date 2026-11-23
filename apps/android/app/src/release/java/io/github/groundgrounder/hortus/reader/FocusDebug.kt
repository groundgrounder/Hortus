// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import android.content.Context
import org.json.JSONObject

/** Release 不写全屏页状态文件。 */
internal object FocusDebug {
    fun write(context: Context, state: JSONObject) = Unit

    fun stateJson(): JSONObject = JSONObject()
}
