package dev.agync.agenthub.ui.components

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.SessionState
import dev.agync.agenthub.model.SessionSummary
import dev.agync.agenthub.ui.theme.Dot
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.StateWord
import java.time.Instant

/**
 * A session in a list. A subagent is indented under its parent and shows its short name in bold.
 * [facts] adds events and size; [participants] adds the mono "claude, laptop" lines.
 */
@Composable
fun SessionRow(
  session: SessionSummary,
  now: Instant,
  fmt: Fmt,
  modifier: Modifier = Modifier,
  onClick: (() -> Unit)? = null,
  facts: Boolean = false,
  participants: Boolean = false,
  tail: String? = null,
) {
  val c = Hub.colors
  val child = session.parent != null
  val shortName = session.name.substringAfter('/')
  val title =
    if (child) buildAnnotatedString {
      append(session.name.substringBefore('/') + "/")
      withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = c.ink)) { append(shortName) }
    } else buildAnnotatedString { withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = c.ink)) { append(session.name) } }
  val word = when (session.state) { SessionState.Live -> "live"; SessionState.Idle -> "idle"; SessionState.Ended -> "ended" }
  val dot = when (session.state) { SessionState.Live -> Dot.Live; SessionState.Idle -> Dot.Idle; SessionState.Ended -> Dot.Filled }
  val rest =
    buildString {
      if (facts) append("· ${session.events} events" + if (session.bytes >= 50_000) " · ${Fmt.bytes(session.bytes)}" else "")
      else if (!child && session.participants.isNotEmpty() && session.state == SessionState.Live) {
        val p = session.participants.first()
        append("· ${p.tool} · ${p.machine}")
      }
      if (tail != null) append(tail)
    }.ifEmpty { null }
  HubRow(
    modifier = modifier,
    glyph = { Glyph(if (child) G.Sub else G.Branch, size = if (child) 20.dp else 24.dp, tint = c.ink2) },
    trailing = fmt.ago(session.lastActivity, now),
    onClick = onClick,
    clickLabel = "Open ${session.name}",
    leadingInset = if (child) 40.dp else 0.dp,
    verticalPadding = if (facts) 12.dp else 14.dp,
    head = {
      Column {
        Text(title, style = Hub.type.body.copy(fontSize = if (child) 15.sp else 16.sp, color = c.ink2))
        StateWord(word, dot, Modifier.padding(top = 2.dp), rest = rest)
      }
    },
  ) {
    if (participants && session.participants.isNotEmpty()) {
      Box(Modifier.padding(top = 2.dp)) {
        Text(session.participants.joinToString("\n") { "${it.tool} · ${it.machine}" }, style = Hub.type.mono.copy(color = c.ink2))
      }
    }
  }
}
