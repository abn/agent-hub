package dev.agync.agenthub.ui.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubMark
import dev.agync.agenthub.ui.theme.hairlineBottom

/** The full-screen note: an optional glyph or mark, a 28 title, short paragraphs capped for reading. */
@Composable
fun HeroNote(
  title: String,
  modifier: Modifier = Modifier,
  glyph: String? = null,
  mark: Boolean = false,
  topPadding: androidx.compose.ui.unit.Dp = 8.dp,
  paragraphs: List<AnnotatedString> = emptyList(),
  small: Boolean = false,
) {
  val c = Hub.colors
  Column(modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, top = topPadding)) {
    if (mark) HubMark(size = 64.dp)
    if (glyph != null) Glyph(glyph, size = 32.dp, tint = c.ink2)
    Text(title, style = Hub.type.page.copy(color = c.ink), modifier = Modifier.padding(top = if (mark) 20.dp else 16.dp, bottom = 12.dp).semantics2())
    paragraphs.forEachIndexed { i, p ->
      Text(
        p,
        style = (if (small && i == paragraphs.lastIndex) Hub.type.meta else Hub.type.body.copy(lineHeight = androidx.compose.ui.unit.TextUnit(24.8f, androidx.compose.ui.unit.TextUnitType.Sp))).copy(color = c.ink2),
        modifier = Modifier.widthIn(max = 360.dp).padding(top = if (i == 0) 0.dp else 12.dp),
      )
    }
  }
}

private fun Modifier.semantics2(): Modifier = this.semantics { heading() }

/** A ruled list of glyph and text lines: the "things to check" and "it was used" lists. */
@Composable
fun GlyphList(items: List<Pair<String, AnnotatedString>>, modifier: Modifier = Modifier, horizontalPadding: androidx.compose.ui.unit.Dp = 24.dp) {
  val c = Hub.colors
  Column(modifier.fillMaxWidth().padding(horizontal = horizontalPadding)) {
    items.forEach { (g, text) ->
      Row(Modifier.fillMaxWidth().hairlineBottom(c.line).padding(vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.Top) {
        Glyph(g, tint = c.ink2, modifier = Modifier.padding(top = 1.dp))
        Text(text, style = Hub.type.body.copy(fontSize = androidx.compose.ui.unit.TextUnit(15f, androidx.compose.ui.unit.TextUnitType.Sp), color = c.ink), modifier = Modifier.weight(1f))
      }
    }
  }
}
