// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// 克制的自然绿灰配色：以文字明度、字重和留白建立层级，避免高饱和色块。
private val LightColors = lightColorScheme(
    primary = Color(0xFF2F5E4E),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFE3EFEA),
    onPrimaryContainer = Color(0xFF14352B),
    secondary = Color(0xFF52635B),
    onSecondary = Color(0xFFFFFFFF),
    background = Color(0xFFFAFBF9),
    onBackground = Color(0xFF1A1E1C),
    surface = Color(0xFFFAFBF9),
    onSurface = Color(0xFF1A1E1C),
    surfaceVariant = Color(0xFFF0F3F0),
    onSurfaceVariant = Color(0xFF4A5550),
    outline = Color(0xFFD5DDD8),
    outlineVariant = Color(0xFFE5EAE7),
    error = Color(0xFFB3261E),
    onError = Color(0xFFFFFFFF),
)

private val DarkColors = darkColorScheme(
    primary = Color(0xFF8CC5B0),
    onPrimary = Color(0xFF0E2B22),
    primaryContainer = Color(0xFF1E3C32),
    onPrimaryContainer = Color(0xFFD3EAE0),
    secondary = Color(0xFFAABBB2),
    onSecondary = Color(0xFF1B2B24),
    background = Color(0xFF131615),
    onBackground = Color(0xFFE4E9E6),
    surface = Color(0xFF131615),
    onSurface = Color(0xFFE4E9E6),
    surfaceVariant = Color(0xFF1C211F),
    onSurfaceVariant = Color(0xFFA2AEA8),
    outline = Color(0xFF333D38),
    outlineVariant = Color(0xFF252D29),
    error = Color(0xFFF2B8B5),
    onError = Color(0xFF601410),
)

@Composable
fun HortusTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = if (darkTheme) DarkColors else LightColors,
        content = content,
    )
}
