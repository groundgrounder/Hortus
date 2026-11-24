// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb

internal const val READER_BODY_SP = 17f

internal data class ReaderPalette(
    val text: Color,
    val muted: Color,
    val accent: Color,
    val codeBackground: Color,
    val highlight: Color,
    val tagBackground: Color,
    val quoteBackground: Color,
    val calloutBackground: Color,
    val rule: Color,
    val error: Color,
    val surface: Color,
) {
    val textArgb: Int get() = text.toArgb()
    val mutedArgb: Int get() = muted.toArgb()
    val accentArgb: Int get() = accent.toArgb()
    val errorArgb: Int get() = error.toArgb()
    val codeBackgroundArgb: Int get() = codeBackground.toArgb()
    val ruleArgb: Int get() = rule.toArgb()
}

@Composable
internal fun readerPalette(): ReaderPalette {
    val scheme = MaterialTheme.colorScheme
    return ReaderPalette(
        text = scheme.onSurface,
        muted = scheme.onSurfaceVariant,
        accent = scheme.primary,
        codeBackground = scheme.surfaceVariant,
        highlight = Color(0xFFFFE082).copy(alpha = 0.38f),
        tagBackground = scheme.primary.copy(alpha = 0.12f),
        quoteBackground = scheme.surfaceVariant.copy(alpha = 0.55f),
        calloutBackground = scheme.primary.copy(alpha = 0.08f),
        rule = scheme.outlineVariant,
        error = scheme.error,
        surface = scheme.surface,
    )
}

internal fun calloutAccent(kind: String, palette: ReaderPalette): Color = when (kind) {
    "warning", "caution", "attention" -> Color(0xFFC77C02)
    "danger", "error", "bug", "failure" -> palette.error
    "success", "check", "done" -> Color(0xFF2E7D4F)
    "tip", "hint", "important" -> Color(0xFF2F6B8A)
    "question", "help", "faq" -> Color(0xFF7A5AA6)
    "example" -> Color(0xFF8A5A2F)
    else -> palette.accent
}

internal fun calloutBackground(kind: String, palette: ReaderPalette): Color =
    calloutAccent(kind, palette).copy(alpha = 0.10f)
