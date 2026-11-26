package dev.agync.agenthub.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.AskText
import dev.agync.agenthub.model.EventKind
import dev.agync.agenthub.model.FeedEvent
import dev.agync.agenthub.model.Span
import dev.agync.agenthub.ui.theme.ButtonKind
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton
import dev.agync.agenthub.ui.theme.hairlineBottom

/** The lede: 18 sp, agent names bold, the rest line muted. */
@Composable
fun LedeText(spans: List<Span>, modifier: Modifier = Modifier) {
  val c = Hub.colors
  val text =
    buildAnnotatedString {
      for (s in spans) {
        withStyle(SpanStyle(fontWeight = if (s.bold) FontWeight.SemiBold else FontWeight.Normal, color = if (s.muted) c.ink3 else c.ink)) { append(s.text) }
      }
    }
  Text(text, style = Hub.type.lede, modifier = modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 16.dp))
}

/** "**agent** wants a decision: ..." with the agent in bold. */
fun agentSentence(agent: String, tail: String, c: dev.agync.agenthub.ui.theme.HubColors): AnnotatedString =
  buildAnnotatedString {
    withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = c.ink)) { append(agent) }
    append(" ")
    append(tail)
  }

/**
 * One row: a 24 glyph column, one gutter, flat under a hairline. [content] goes under the
 * sentence and the clock, spanning both: verbs, receipts, queued lines.
 */
@Composable
fun HubRow(
  modifier: Modifier = Modifier,
  glyph: @Composable () -> Unit,
  trailing: String? = null,
  wait: Boolean = false,
  selected: Boolean = false,
  onClick: (() -> Unit)? = null,
  clickLabel: String? = null,
  leadingInset: Dp = 0.dp,
  verticalPadding: Dp = 14.dp,
  head: @Composable () -> Unit,
  content: (@Composable () -> Unit)? = null,
) {
  val c = Hub.colors
  Box(
    modifier
      .fillMaxWidth()
      .background(if (selected) c.sunk else Color.Transparent)
      .hairlineBottom(c.line)
      .drawBehind {
        if (wait) drawRect(c.accent, size = androidx.compose.ui.geometry.Size(3.dp.toPx(), size.height))
        if (selected && !wait) drawRect(c.accent, size = androidx.compose.ui.geometry.Size(3.dp.toPx(), size.height))
      }
      .then(if (onClick != null) Modifier.clickable(role = Role.Button, onClickLabel = clickLabel, onClick = onClick) else Modifier),
  ) {
    Row(Modifier.fillMaxWidth().padding(start = 16.dp + leadingInset, end = 16.dp, top = verticalPadding, bottom = verticalPadding)) {
      Box(Modifier.width(24.dp).heightIn(min = 24.dp), contentAlignment = Alignment.Center) { glyph() }
      Column(Modifier.weight(1f).padding(start = 16.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
          Box(Modifier.weight(1f)) { head() }
          if (trailing != null) Text(trailing, style = Hub.type.mono.copy(color = c.ink2), modifier = Modifier.padding(top = 1.dp))
        }
        if (content != null) content()
      }
    }
  }
}

/** The mono "!" or "?" or glyph that marks a kind of ask. */
@Composable
fun KindMark(kind: AskKind, accent: Boolean) {
  val c = Hub.colors
  when (kind) {
    AskKind.Approval -> Text("!", style = Hub.type.mono.copy(fontSize = 18.sp, fontWeight = FontWeight.SemiBold, color = if (accent) c.accent else c.ink2), modifier = Modifier.clearAndSetSemantics {})
    AskKind.Question -> Text("?", style = Hub.type.mono.copy(fontSize = 18.sp, fontWeight = FontWeight.SemiBold, color = c.ink2), modifier = Modifier.clearAndSetSemantics {})
    AskKind.Joining -> Glyph(G.Laptop, tint = c.ink2)
    AskKind.Idea -> Glyph(G.Idea, tint = c.ink2)
  }
}

/** One ask on Now: its sentence, clock, every verb it has, and its receipts line. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AskRow(
  ask: Ask,
  clock: String,
  onOpen: () -> Unit,
  onVerb: (String) -> Unit,
  modifier: Modifier = Modifier,
  selected: Boolean = false,
  sub: String? = null,
  note: String? = null,
  receipts: String? = null,
  glyphOverride: String? = null,
) {
  val c = Hub.colors
  val wait = ask.kind == AskKind.Approval && ask.waiting && sub == null
  val verbs = dev.agync.agenthub.model.NowBuilder.verbs(ask)
  HubRow(
    modifier = modifier,
    glyph = { if (glyphOverride != null) Glyph(glyphOverride, tint = c.ink2) else KindMark(ask.kind, accent = wait) },
    trailing = clock,
    wait = wait,
    selected = selected,
    onClick = onOpen,
    clickLabel = "Open ${ask.agent}",
    head = {
      Column {
        Text(agentSentence(ask.agent, AskText.rowTail(ask), c), style = Hub.type.body.copy(color = c.ink))
        if (sub != null) Text(sub, style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(top = 2.dp))
      }
    },
  ) {
    FlowRow(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
      verbs.forEachIndexed { i, v ->
        val primary = ask.kind == AskKind.Approval && i == 0
        HubButton(v, { onVerb(v) }, kind = if (primary) ButtonKind.Primary else ButtonKind.Default, contentDescription = verbDescription(ask, v))
      }
    }
    if (receipts != null) Text(receipts, style = Hub.type.mono.copy(color = c.ink3), modifier = Modifier.padding(top = 4.dp))
    if (note != null) Text(note, style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(top = 4.dp))
  }
}

private fun verbDescription(ask: Ask, verb: String) =
  when (ask.kind) {
    AskKind.Approval -> "$verb ${ask.agent}: ${ask.sentence}"
    AskKind.Question -> "Answer ${ask.agent}: $verb"
    AskKind.Joining -> "$verb ${ask.agent}"
    AskKind.Idea -> "$verb the idea: ${ask.sentence}"
  }

/** A feed event row: glyph by kind, bold actor, sentence, clock. */
@Composable
fun EventRow(event: FeedEvent, clock: String, modifier: Modifier = Modifier, who: String? = null, onClick: (() -> Unit)? = null, trailingContent: (@Composable () -> Unit)? = null) {
  val c = Hub.colors
  HubRow(
    modifier = modifier,
    glyph = {
      when (event.kind) {
        EventKind.Asked -> Text("?", style = Hub.type.mono.copy(fontSize = 18.sp, fontWeight = FontWeight.SemiBold, color = if (event.waitingOnYou) c.accent else c.ink2))
        EventKind.Published -> Glyph(G.Doc, tint = c.ink2)
        EventKind.Noted -> Text("·", style = Hub.type.mono.copy(fontSize = 18.sp, fontWeight = FontWeight.SemiBold, color = c.ink2))
        EventKind.Started -> Glyph(G.Sub, tint = c.ink2)
        EventKind.Finished, EventKind.Answered, EventKind.Approved -> Glyph(G.Check, tint = c.ink2)
        EventKind.Captured -> Glyph(G.Idea, tint = c.ink2)
      }
    },
    trailing = clock,
    wait = event.waitingOnYou,
    onClick = onClick,
    head = { Text(agentSentence(event.who, event.text, c), style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink)) },
  ) {
    if (who != null) Text(who, style = Hub.type.mono.copy(color = c.ink3), modifier = Modifier.padding(top = 2.dp))
    trailingContent?.invoke()
  }
}

/** The two-column facts list under an item: label, then value, hairlines above and below. */
@Composable
fun FactList(rows: List<Pair<String, @Composable () -> Unit>>, modifier: Modifier = Modifier, labelWidth: Dp = 88.dp, centered: Set<String> = emptySet()) {
  val c = Hub.colors
  Column(
    modifier.fillMaxWidth().drawBehind {
      drawLine(c.line, Offset(0f, 0.5f), Offset(size.width, 0.5f), 1.dp.toPx())
      drawLine(c.line, Offset(0f, size.height - 0.5f), Offset(size.width, size.height - 0.5f), 1.dp.toPx())
    }.padding(horizontal = 16.dp, vertical = 14.dp),
    verticalArrangement = Arrangement.spacedBy(10.dp),
  ) {
    rows.forEach { (k, v) ->
      // A row that holds a 48 dp control is taller than its text: its label sits level with the value, not above it.
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = if (k in centered) Alignment.CenterVertically else Alignment.Top) {
        Text(k, style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.width(labelWidth))
        Box(Modifier.weight(1f)) { v() }
      }
    }
  }
}

/** A value in a fact list: mono, underlined when it opens something. */
@Composable
fun FactValue(text: String, link: Boolean = false, suffix: String? = null, sans: Boolean = false, onClick: (() -> Unit)? = null) {
  val c = Hub.colors
  val base = if (sans) Hub.type.meta.copy(color = c.ink) else Hub.type.mono.copy(fontSize = 13.sp, lineHeight = 21.sp, color = c.ink)
  val text2 =
    buildAnnotatedString {
      if (link) withStyle(SpanStyle(textDecoration = androidx.compose.ui.text.style.TextDecoration.Underline)) { append(text) } else append(text)
      if (suffix != null) append(" $suffix")
    }
  Text(text2, style = base, modifier = if (onClick != null) Modifier.heightIn(min = 24.dp).clickable(role = Role.Button, onClick = onClick) else Modifier)
}

/** A settings row: 56 high under a coarse pointer, glyph, title with a line under it, an end slot. */
@Composable
fun SettingsRow(
  glyph: String,
  title: String,
  modifier: Modifier = Modifier,
  detail: AnnotatedString? = null,
  onClick: (() -> Unit)? = null,
  glyphDescription: String? = null,
  end: (@Composable RowScope.() -> Unit)? = null,
) {
  val c = Hub.colors
  Row(
    modifier.fillMaxWidth().hairlineBottom(c.line)
      .then(if (onClick != null) Modifier.clickable(role = Role.Button, onClick = onClick) else Modifier)
      .heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
    horizontalArrangement = Arrangement.spacedBy(16.dp),
  ) {
    Glyph(glyph, tint = c.ink2, description = glyphDescription)
    Column(Modifier.weight(1f)) {
      Text(title, style = Hub.type.body.copy(fontWeight = FontWeight.Medium, color = c.ink))
      if (detail != null) Text(detail, style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
    }
    if (end != null) end()
  }
}

fun detail(text: String, c: dev.agync.agenthub.ui.theme.HubColors, accentTail: String? = null): AnnotatedString =
  buildAnnotatedString {
    append(text)
    if (accentTail != null) withStyle(SpanStyle(color = c.accent, fontFamily = dev.agync.agenthub.ui.theme.PlexMono)) { append(accentTail) }
  }

/** The quiet "queued" line: an upload glyph and words. */
@Composable
fun QueuedLine(text: String, modifier: Modifier = Modifier) {
  Row(modifier, horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
    Glyph(G.Queue, size = 16.dp, tint = Hub.colors.ink2)
    Text(text, style = Hub.type.meta.copy(fontSize = 13.sp, fontWeight = FontWeight.Medium, color = Hub.colors.ink2))
  }
}
