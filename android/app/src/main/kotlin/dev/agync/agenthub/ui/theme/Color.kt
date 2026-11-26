package dev.agync.agenthub.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/**
 * The brand tokens of the web app, unchanged, as the Android board uses them.
 * Copper (`accent`) means one thing: this needs you. `danger` is deletion only.
 */
@Immutable
data class HubColors(
  val bg: Color,
  val sunk: Color,
  val raised: Color,
  val line: Color,
  val lineStrong: Color,
  val ink: Color,
  val ink2: Color,
  val ink3: Color,
  val inv: Color,
  val accent: Color,
  val accentBg: Color,
  val ok: Color,
  val danger: Color,
  val dangerBg: Color,
  val scrim: Color,
  val knowledge: Color,
  val isDark: Boolean,
)

val LightColors =
  HubColors(
    bg = Color(0xFFF3F5F2),
    sunk = Color(0xFFE8ECE7),
    raised = Color(0xFFFBFCFA),
    line = Color(0xFFD3DAD2),
    lineStrong = Color(0xFF66726A),
    ink = Color(0xFF151B17),
    ink2 = Color(0xFF3B4640),
    ink3 = Color(0xFF56625A),
    inv = Color(0xFFF3F5F2),
    accent = Color(0xFF84461D),
    accentBg = Color(0xFFF1E4D8),
    ok = Color(0xFF2C6A3D),
    danger = Color(0xFFA3154A),
    dangerBg = Color(0xFFF7E3EA),
    scrim = Color(0x52151B17),
    knowledge = Color(0xFF5E6A1F),
    isDark = false,
  )

val DarkColors =
  HubColors(
    bg = Color(0xFF111513),
    sunk = Color(0xFF1A1F1C),
    raised = Color(0xFF161B18),
    line = Color(0xFF29312C),
    lineStrong = Color(0xFF7A867E),
    ink = Color(0xFFE4EAE5),
    ink2 = Color(0xFFB5BFB8),
    ink3 = Color(0xFF8E9991),
    inv = Color(0xFF111513),
    accent = Color(0xFFE3A06F),
    accentBg = Color(0xFF2E241C),
    ok = Color(0xFF7DC490),
    danger = Color(0xFFFF7099),
    dangerBg = Color(0xFF3A1824),
    scrim = Color(0x8C000000),
    knowledge = Color(0xFFB5C26A),
    isDark = true,
  )

val LocalHubColors = staticCompositionLocalOf { LightColors }
