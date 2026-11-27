// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.reader.ui

import android.content.Context
import io.github.groundgrounder.hortus.R

/** 将内核/渲染器内部错误码映射为用户可读的本地化文案，内部码仅保留给 DebugProbes。 */
internal fun localizedRenderError(context: Context, rawReason: String): String {
    val code = rawReason.substringBefore(':').trim()
    val resId = when {
        code == "diagram.not_a_tree" -> R.string.render_error_diagram_not_tree
        code == "diagram.unsupported" -> R.string.render_error_diagram_unsupported
        code == "diagram.syntax" -> R.string.render_error_diagram_syntax
        code.startsWith("diagram.") -> R.string.render_error_diagram_failed
        code == "image.missing" || code == "image.not_file" -> R.string.render_error_image_missing
        code == "image.outside" || code == "image.invalid_path" -> R.string.render_error_image_outside
        code == "image.remote" -> R.string.render_error_image_remote
        code == "image.unsupported" || code == "image.svg_unsupported" -> R.string.render_error_image_unsupported
        code == "image.too_large" -> R.string.render_error_image_too_large
        code.startsWith("image.") -> R.string.render_error_image_decode
        code.startsWith("math.") || rawReason == "formula could not be rendered" -> R.string.render_error_math_failed
        else -> null
    }
    return if (resId != null) context.getString(resId) else rawReason
}
