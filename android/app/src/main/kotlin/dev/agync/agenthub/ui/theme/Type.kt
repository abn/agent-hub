package dev.agync.agenthub.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.R

val PlexSans =
  FontFamily(
    Font(R.font.ibm_plex_sans_regular, FontWeight.Normal),
    Font(R.font.ibm_plex_sans_medium, FontWeight.Medium),
    Font(R.font.ibm_plex_sans_semibold, FontWeight.SemiBold),
    Font(R.font.ibm_plex_sans_semibold, FontWeight.Bold),
  )

val PlexMono =
  FontFamily(
    Font(R.font.ibm_plex_mono_regular, FontWeight.Normal),
    Font(R.font.ibm_plex_mono_medium, FontWeight.Medium),
    Font(R.font.ibm_plex_mono_medium, FontWeight.SemiBold),
  )

/**
 * The Android type scale: the web's scale moved up one step. 28 page, 22 section,
 * 18 lede, 16 body, 14 meta, 13 mono, 12 floor. Sizes are sp so they follow the
 * system font scale to 200% and rows grow to fit.
 */
@Immutable
data class HubType(
  val page: TextStyle,
  val title: TextStyle,
  val section: TextStyle,
  val lede: TextStyle,
  val body: TextStyle,
  val bodyStrong: TextStyle,
  val meta: TextStyle,
  val metaStrong: TextStyle,
  val mono: TextStyle,
  val monoSmall: TextStyle,
  val label: TextStyle,
  val button: TextStyle,
  val buttonLarge: TextStyle,
  val itemTitle: TextStyle,
  val sas: TextStyle,
  val codeInput: TextStyle,
)

private fun sans(size: Int, weight: FontWeight = FontWeight.Normal, line: Float = 1.45f) =
  TextStyle(fontFamily = PlexSans, fontSize = size.sp, fontWeight = weight, lineHeight = (size * line).sp)

private fun mono(size: Int, weight: FontWeight = FontWeight.Normal, line: Float = 1.5f) =
  TextStyle(fontFamily = PlexMono, fontSize = size.sp, fontWeight = weight, lineHeight = (size * line).sp)

val DefaultType =
  HubType(
    page = sans(28, FontWeight.SemiBold, 1.25f),
    title = sans(22, FontWeight.SemiBold, 1.3f),
    section = sans(20, FontWeight.SemiBold, 1.25f),
    lede = sans(18, FontWeight.Normal, 1.45f),
    body = sans(16, FontWeight.Normal, 1.45f),
    bodyStrong = sans(16, FontWeight.SemiBold, 1.4f),
    meta = sans(14, FontWeight.Normal, 1.4f),
    metaStrong = sans(14, FontWeight.Medium, 1.4f),
    mono = mono(13),
    monoSmall = mono(12),
    label = sans(12, FontWeight.SemiBold, 1.2f).copy(letterSpacing = 0.06.em),
    button = sans(14, FontWeight.Medium, 1.2f),
    buttonLarge = sans(16, FontWeight.Medium, 1.2f),
    itemTitle = sans(24, FontWeight.SemiBold, 1.3f),
    sas = mono(26, FontWeight.Medium, 1.3f).copy(letterSpacing = 0.04.em),
    codeInput = mono(20, FontWeight.Medium, 1.2f).copy(letterSpacing = 0.06.em),
  )

val LocalHubType = staticCompositionLocalOf { DefaultType }
