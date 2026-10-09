package org.veskra.dev.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// Deliberately NEUTRAL (near-grey) chrome and no dynamic color: saturated UI colors next to the
// canvas bias how artists judge their own colors (simultaneous contrast).
private val Dark = darkColorScheme(
    primary = Color(0xFFD0D4DC),
    onPrimary = Color(0xFF1B1E23),
    primaryContainer = Color(0xFF3A3F47),
    onPrimaryContainer = Color(0xFFE6E8EC),
    secondary = Color(0xFFB8BCC4),
    surface = Color(0xFF22252B),
    onSurface = Color(0xFFE6E8EC),
    surfaceVariant = Color(0xFF30343B),
    onSurfaceVariant = Color(0xFFC4C7CE),
    background = Color(0xFF1B1E23),
    onBackground = Color(0xFFE6E8EC),
    error = Color(0xFFFFB4AB),
)

private val Light = lightColorScheme(
    primary = Color(0xFF3A3F47),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFDDE0E6),
    onPrimaryContainer = Color(0xFF1B1E23),
    secondary = Color(0xFF545961),
    surface = Color(0xFFF4F5F7),
    onSurface = Color(0xFF1B1E23),
    surfaceVariant = Color(0xFFE2E4E8),
    onSurfaceVariant = Color(0xFF44474D),
    background = Color(0xFFF4F5F7),
    onBackground = Color(0xFF1B1E23),
)

@Composable
fun VeskraTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (dark) Dark else Light, content = content)
}
