package dev.agync.agenthub.ui.theme

import android.app.Activity
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat

object Hub {
  val colors: HubColors
    @Composable get() = LocalHubColors.current

  val type: HubType
    @Composable get() = LocalHubType.current
}

/** How the app picks its theme: the system's by default. */
enum class ThemeMode { System, Light, Dark }

@Composable
fun HubTheme(mode: ThemeMode = ThemeMode.System, content: @Composable () -> Unit) {
  val dark =
    when (mode) {
      ThemeMode.System -> isSystemInDarkTheme()
      ThemeMode.Light -> false
      ThemeMode.Dark -> true
    }
  val colors = if (dark) DarkColors else LightColors
  val scheme =
    if (dark) {
      darkColorScheme(
        primary = colors.accent, onPrimary = colors.inv, background = colors.bg, onBackground = colors.ink,
        surface = colors.bg, onSurface = colors.ink, surfaceVariant = colors.sunk, onSurfaceVariant = colors.ink2,
        outline = colors.lineStrong, outlineVariant = colors.line, error = colors.danger,
      )
    } else {
      lightColorScheme(
        primary = colors.accent, onPrimary = colors.inv, background = colors.bg, onBackground = colors.ink,
        surface = colors.bg, onSurface = colors.ink, surfaceVariant = colors.sunk, onSurfaceVariant = colors.ink2,
        outline = colors.lineStrong, outlineVariant = colors.line, error = colors.danger,
      )
    }
  val view = LocalView.current
  if (!view.isInEditMode) {
    SideEffect {
      val window = (view.context as? Activity)?.window ?: return@SideEffect
      WindowCompat.getInsetsController(window, view).apply {
        isAppearanceLightStatusBars = !dark
        isAppearanceLightNavigationBars = !dark
      }
    }
  }
  CompositionLocalProvider(LocalHubColors provides colors, LocalHubType provides DefaultType) {
    MaterialTheme(colorScheme = scheme, content = content)
  }
}
