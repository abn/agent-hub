package dev.agync.agenthub.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.data.Refusal
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.AskStatus
import dev.agync.agenthub.model.AskText
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.ui.components.FactList
import dev.agync.agenthub.ui.components.FactValue
import dev.agync.agenthub.ui.components.HubRow
import dev.agync.agenthub.ui.components.QueuedLine
import dev.agync.agenthub.ui.theme.ActionBar
import dev.agync.agenthub.ui.theme.Banner
import dev.agync.agenthub.ui.theme.ButtonKind
import dev.agync.agenthub.ui.theme.ButtonSize
import dev.agync.agenthub.ui.theme.Dot
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton
import dev.agync.agenthub.ui.theme.HubCheckbox
import dev.agync.agenthub.ui.theme.HubField
import dev.agync.agenthub.ui.theme.HubIconButton
import dev.agync.agenthub.ui.theme.SectionLabel
import dev.agync.agenthub.ui.theme.SmallTopBar
import dev.agync.agenthub.ui.theme.StateWord
import dev.agync.agenthub.ui.theme.StatusPill
import dev.agync.agenthub.ui.theme.TextAction
import dev.agync.agenthub.ui.theme.ControlShape
import dev.agync.agenthub.viewmodel.ItemUi
import java.time.Instant

data class ItemActions(
  val onBack: () -> Unit,
  val onMore: () -> Unit,
  val onAnswer: (String) -> Unit,
  val onApprove: (String) -> Unit,
  val onDecline: (String) -> Unit,
  val onAdmit: (Boolean, String, Set<String>) -> Unit,
  val onSnooze: () -> Unit,
  val onDiscard: () -> Unit,
  val onEdit: (String) -> Unit,
  val onNext: () -> Unit,
  val onOpenSession: (String) -> Unit,
  val onOpenArtifact: (String) -> Unit,
  val onCopy: (String) -> Unit,
  val onResolve: () -> Unit = {},
  val onClear: () -> Unit = {},
  val onNote: (String) -> Unit = {},
  val onOpenBrain: (String, String) -> Unit = { _, _ -> },
)

/** The page title of an ask: its sentence, or its outcome once it is decided. */
fun itemTitle(ask: Ask, fmt: Fmt): String {
  val at = ask.decidedAt?.let(fmt::time)
  return when (ask.status) {
    AskStatus.Waiting -> if (ask.kind == AskKind.Joining) ask.sentence else ask.sentence
    AskStatus.Approved -> "Approved at $at: ${ask.sentence}"
    AskStatus.Declined -> "Declined at $at: ${ask.sentence}"
    AskStatus.Answered -> if (ask.kind == AskKind.Question) ask.sentence else "Answered at $at: ${ask.sentence}"
    AskStatus.Admitted -> "${ask.agent} joined at $at"
    AskStatus.TurnedAway -> "${ask.agent} was turned away at $at"
    AskStatus.ClosedAtDeadline -> "Closed at $at with no answer: ${ask.sentence}"
    AskStatus.Resolved -> "Resolved at $at: ${ask.sentence}"
    AskStatus.Cleared -> "Cleared at $at: ${ask.sentence}"
  }
}

private fun outcomeWord(s: AskStatus): String =
  when (s) {
    AskStatus.Waiting -> "Waiting on you"
    AskStatus.Approved -> "Approved"
    AskStatus.Declined -> "Declined"
    AskStatus.Answered -> "Answered"
    AskStatus.Admitted -> "Admitted"
    AskStatus.TurnedAway -> "Turned away"
    AskStatus.ClosedAtDeadline -> "Closed"
    AskStatus.Resolved -> "Resolved"
    AskStatus.Cleared -> "Cleared"
  }

@Composable
fun ItemScreen(ui: ItemUi, fmt: Fmt, now: Instant, actions: ItemActions, wide: Boolean = false, modifier: Modifier = Modifier, noteVersion: Int = 0, notice: String? = null) {
  val c = Hub.colors
  Column(modifier.fillMaxSize().background(c.bg)) {
    when (ui) {
      ItemUi.Loading -> SmallTopBar("", onBack = actions.onBack)
      ItemUi.Missing -> {
        SmallTopBar("", onBack = actions.onBack)
        Text("This ask is no longer on the hub.", style = Hub.type.body.copy(color = c.ink2), modifier = Modifier.padding(24.dp))
      }
      is ItemUi.Open -> OpenItem(ui, fmt, now, actions, wide, noteVersion, notice)
      is ItemUi.Queued -> QueuedItem(ui, fmt, actions)
      is ItemUi.Refused -> RefusedItem(ui, fmt, actions)
      is ItemUi.Resolved -> ResolvedItem(ui, fmt, actions)
    }
  }
}

@Composable
private fun TopBar(title: String, actions: ItemActions, wide: Boolean = false) {
  // In a detail pane there is nowhere to go back to: the list is beside it.
  SmallTopBar(title, titleMuted = true, onBack = if (wide) null else actions.onBack) { HubIconButton(G.Dot3, "More", actions.onMore) }
}

@Composable
private fun KindLine(kind: AskKind, trailing: @Composable () -> Unit) {
  val c = Hub.colors
  Row(Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
      if (kind == AskKind.Joining) Glyph(G.Laptop, size = 18.dp, tint = c.ink2)
      else if (kind == AskKind.Idea) Glyph(G.Idea, size = 18.dp, tint = c.ink2)
      else Text(AskText.kindMark(kind), style = Hub.type.mono.copy(fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.ink2))
      Text(AskText.kindLabel(kind), style = Hub.type.metaStrong.copy(fontSize = 13.sp, color = c.ink2))
    }
    trailing()
  }
}

@Composable
private fun TitleAndMeta(ask: Ask, fmt: Fmt, title: String = itemTitle(ask, fmt), sizeSp: Int = 24) {
  val c = Hub.colors
  Text(title, style = Hub.type.itemTitle.copy(fontSize = sizeSp.sp, lineHeight = (sizeSp * 1.3f).sp, color = c.ink), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 6.dp))
  if (ask.kind == AskKind.Joining) return
  Text(
    buildAnnotatedString {
      withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = c.ink)) { append(ask.agent) }
      if (ask.project != null) append(" in ${ask.project}")
      append(" \u00B7 ")
      withStyle(SpanStyle(fontFamily = dev.agync.agenthub.ui.theme.PlexMono, fontSize = 13.sp)) { append(listOfNotNull(ask.tool, ask.machine, fmt.time(ask.askedAt)).joinToString(" \u00B7 ")) }
    },
    style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(horizontal = 16.dp),
  )
}

/** "phone" or "tablet": the device a per-device snooze belongs to. */
@Composable
fun deviceWord(): String = if (LocalConfiguration.current.smallestScreenWidthDp >= 600) "tablet" else "phone"

@Composable
private fun ColumnScope.OpenItem(ui: ItemUi.Open, fmt: Fmt, now: Instant, a: ItemActions, wide: Boolean, noteVersion: Int, notice: String?) {
  val c = Hub.colors
  val ask = ui.ask
  val device = deviceWord()
  var note by rememberSaveable(ask.id) { mutableStateOf("") }
  var reply by rememberSaveable(ask.id, noteVersion) { mutableStateOf("") }
  // What the person ticked. Nothing is granted unless they tick it, and only a requested project can be ticked.
  var granted by remember(ask.id) { mutableStateOf(emptySet<String>()) }
  val name = ask.enrolment?.machine ?: ask.agent
  TopBar(if (ui.total > 1 || ui.position > 1) "${ui.position} of ${ui.total} need you" else AskText.kindLabel(ask.kind), a, wide)
  if (notice != null) Banner(G.Warn) { Text(notice, style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink)) }
  if (ui.offline) Banner(G.Offline) { Text("Offline. Your answer waits on this phone and sends when the hub is back.", style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink)) }
  val r = ask.receipts
  val facts =
    buildList<Pair<String, @Composable () -> Unit>> {
      r.session?.let { s -> add("Session" to { FactValue(r.sessionName ?: s, link = true, suffix = r.sessionEvents?.let { "\u00B7 $it events" }, onClick = { a.onOpenSession(s) }) }) }
      r.brainKey?.let { k -> add("Brain" to { FactValue(k, link = true, onClick = r.session?.let { s -> { a.onOpenBrain(s, k) } }) }) }
      r.artifact?.let { n -> add("Artifact" to { FactValue(n, link = true, suffix = r.artifactVersion?.let { "v$it" }, onClick = { a.onOpenArtifact(n) }) }) }
      r.eventId?.let { id ->
        add("Event" to {
          Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            FactValue(id)
            // The copy control is a glyph on the row that holds the string, not the string made into a link.
            HubIconButton(G.Copy, "Copy the event id", { a.onCopy(id) }, glyphSize = 18.dp)
          }
        })
      }
      if (ask.kind == AskKind.Approval && ask.onSilence != null) add((if (wide) "Deadline" else "If silent") to { FactValue(if (wide) ask.deadline?.let { "${fmt.time(it)}, ${ask.onSilence.lowercase().replace(Regex(" at \\d.*"), "")} if silent" } ?: ask.onSilence else ask.onSilence, sans = !wide) })
    }
  Row(Modifier.weight(1f).fillMaxWidth()) {
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      KindLine(ask.kind) {
        if (ui.snoozedUntil != null) Text("Snoozed on this $device until ${fmt.time(ui.snoozedUntil)}", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
        else StatusPill("Waiting on you")
      }
      TitleAndMeta(ask, fmt)
      if (ask.kind == AskKind.Joining) {
        Text("Asked at ${fmt.time(ask.askedAt)} \u00B7 pending", style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp))
      }
      when (ask.kind) {
        AskKind.Question -> if (!ask.prose.isNullOrBlank()) Text(ask.prose, style = Hub.type.body.copy(lineHeight = 24.8.sp, color = c.ink), modifier = Modifier.widthIn(max = 640.dp).padding(start = 16.dp, end = 16.dp, top = 16.dp))
        AskKind.Approval -> {
          SectionLabel("What approving does", Modifier.padding(top = 12.dp), note = "as ${ask.tool ?: ask.agent} wrote it", topPadding = 20.dp)
          Text(
            ask.prose.orEmpty(),
            style = Hub.type.body.copy(lineHeight = 24.8.sp, color = c.ink),
            modifier = Modifier.widthIn(max = 640.dp).padding(start = 16.dp, end = 16.dp, top = 12.dp).drawBehind { drawLine(c.lineStrong, Offset(1.dp.toPx(), 0f), Offset(1.dp.toPx(), size.height), 2.dp.toPx()) }.padding(start = 14.dp),
          )
        }
        AskKind.Joining -> Unit
        AskKind.Idea -> if (!ask.prose.isNullOrBlank()) Text(ask.prose, style = Hub.type.body.copy(lineHeight = 24.8.sp, color = c.ink2), modifier = Modifier.widthIn(max = 640.dp).padding(start = 16.dp, end = 16.dp, top = 16.dp))
      }
      if (ask.kind == AskKind.Joining) {
        val e = ask.enrolment
        if (e != null) FactList(
          buildList {
            e.firstAgent?.let { add("First agent" to { FactValue(it) }) }
            e.system?.let { add("System" to { FactValue(it) }) }
            e.from?.let { add("From" to { FactValue(it) }) }
            e.reason?.let { add("Reason" to { FactValue(it, sans = true) }) }
          },
          Modifier.padding(top = 16.dp), labelWidth = 104.dp,
        )
        Text("Approving covers every agent on this machine, under the name it asked for. They read and write every ordinary project.", style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp))
        e?.requestedProjects?.forEach { p ->
          val on = p in granted
          Row(
            Modifier.padding(top = 16.dp).fillMaxWidth().drawBehind {
              drawLine(c.line, Offset(0f, 0.5f), Offset(size.width, 0.5f), 1.dp.toPx())
              drawLine(c.line, Offset(0f, size.height - 0.5f), Offset(size.width, size.height - 0.5f), 1.dp.toPx())
            }.toggleable(on, role = Role.Checkbox, onValueChange = { granted = if (it) granted + p else granted - p }).heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 14.dp),
            horizontalArrangement = Arrangement.spacedBy(16.dp),
          ) {
            HubCheckbox(on, Modifier.padding(top = 2.dp))
            Column {
              Text("Grant $p", style = Hub.type.body.copy(fontWeight = FontWeight.Medium, color = c.ink))
              Text("It asked for $p. Without a grant it stays out.", style = Hub.type.meta.copy(color = c.ink2))
            }
          }
        }
      } else {
        if (!wide) FactList(facts, Modifier.padding(top = 20.dp), centered = setOf("Event"))
        if (ask.kind == AskKind.Question && ask.related.isNotEmpty()) {
          SectionLabel("Related", rule = false, topPadding = 20.dp)
          ask.related.forEach { rel ->
            HubRow(glyph = { Text("?", style = Hub.type.mono.copy(fontSize = 18.sp, color = c.ink2)) }, trailing = rel.whenLabel, head = {
              Text(buildAnnotatedString {
                withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = c.ink)) { append(rel.agent) }
                append(" asked: ${rel.sentence} ")
                withStyle(SpanStyle(color = c.ink3)) { append(rel.outcome) }
              }, style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink))
            })
          }
        }
        if (ui.next != null && ask.kind == AskKind.Approval) {
          Text(
            buildAnnotatedString {
              append("After you decide, the next ask opens: ")
              withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = c.ink)) { append(ui.next.agent) }
              append(" ${AskText.ledeTail(ui.next)}")
            },
            style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 16.dp),
          )
        }
        if (wide && ask.kind == AskKind.Approval) {
          HubField(note, { note = it }, Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 8.dp), hint = "Note for ${ask.tool ?: ask.agent}, optional", textStyle = Hub.type.meta.copy(fontSize = 15.sp), minHeight = 48.dp, description = "Note for ${ask.agent}")
        }
        if (wide && ask.kind == AskKind.Idea) {
          Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
            HubField(reply, { reply = it }, Modifier.weight(1f), hint = "Add a note", description = "Note on the idea", keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send), onAction = { if (reply.isNotBlank()) a.onNote(reply) })
            Box(Modifier.size(52.dp).border(1.dp, c.lineStrong, ControlShape), contentAlignment = Alignment.Center) {
              HubIconButton(G.Send, "Add the note to the idea", { if (reply.isNotBlank()) a.onNote(reply) })
            }
          }
        }
      }
    }
    if (wide && ask.kind != AskKind.Joining) {
      Column(Modifier.width(240.dp).fillMaxHeight().padding(start = 20.dp, top = 8.dp, end = 16.dp)) {
        facts.forEach { (k, v) ->
          Text(k.uppercase(), style = Hub.type.label.copy(color = c.ink2), modifier = Modifier.padding(top = 16.dp))
          Box(Modifier.padding(top = 4.dp)) { v() }
        }
      }
    }
  }
  ActionBar {
    val deadline = ask.deadline
    val dueText =
      if (ask.kind == AskKind.Joining) null
      else if (deadline != null) buildAnnotatedString {
        append("Due ")
        withStyle(SpanStyle(fontFamily = dev.agync.agenthub.ui.theme.PlexMono)) { append(fmt.time(deadline)) }
        append(", ${fmt.until(deadline, now)}")
      } else buildAnnotatedString { append("No deadline") }
    if (wide) {
      Row(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(dueText ?: AnnotatedString("Refusing turns the machine away. It gets no reason."), style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.weight(1f))
        if (ui.snoozedUntil == null && ask.kind != AskKind.Joining) TextAction("Snooze 1h on this $device", a.onSnooze)
        when (ask.kind) {
          AskKind.Approval -> {
            HubButton("Decline", { a.onDecline(note) }, Modifier.widthIn(min = 140.dp), size = ButtonSize.Large, contentDescription = "Decline ${ask.agent}: ${ask.sentence}")
            HubButton("Approve", { a.onApprove(note) }, Modifier.widthIn(min = 140.dp), kind = ButtonKind.Primary, size = ButtonSize.Large, contentDescription = "Approve ${ask.agent}: ${ask.sentence}")
          }
          AskKind.Question -> ask.answers.forEach { ans -> HubButton(ans, { a.onAnswer(ans) }, Modifier.widthIn(min = 140.dp), size = ButtonSize.Large, contentDescription = "Answer ${ask.agent}: $ans") }
          AskKind.Joining -> {
            HubButton("Refuse", { a.onAdmit(false, name, emptySet()) }, Modifier.widthIn(min = 140.dp), size = ButtonSize.Large)
            HubButton("Approve", { a.onAdmit(true, name, granted) }, Modifier.widthIn(min = 140.dp), kind = ButtonKind.Primary, size = ButtonSize.Large)
          }
          AskKind.Idea -> {
            HubButton("Clear", a.onClear, Modifier.widthIn(min = 140.dp), size = ButtonSize.Large, contentDescription = "Clear the idea: ${ask.sentence}")
            HubButton("Resolve", a.onResolve, Modifier.widthIn(min = 140.dp), kind = ButtonKind.Primary, size = ButtonSize.Large, contentDescription = "Resolve the idea: ${ask.sentence}")
          }
        }
      }
      return@ActionBar
    }
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
      if (ask.kind == AskKind.Joining) {
        Text("Refusing turns the machine away. It gets no reason.", style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.weight(1f))
      } else {
        Text(dueText ?: AnnotatedString(""), style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.weight(1f))
        if (ui.snoozedUntil == null) TextAction("Snooze 1h on this $device", a.onSnooze)
      }
    }
    when (ask.kind) {
      AskKind.Question -> {
        if (ask.answers.size in 1..3) Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
          ask.answers.forEach { ans -> HubButton(ans, { a.onAnswer(ans) }, Modifier.weight(1f), contentDescription = "Answer ${ask.agent}: $ans") }
        }
        Row(Modifier.fillMaxWidth().padding(bottom = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
          HubField(
            reply, { reply = it }, Modifier.weight(1f), hint = "Or write a reply", description = "Reply to ${ask.agent}",
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send), onAction = { if (reply.isNotBlank()) a.onAnswer(reply) },
          )
          Box(Modifier.size(52.dp).border(1.dp, c.lineStrong, ControlShape), contentAlignment = Alignment.Center) {
            HubIconButton(G.Send, "Send reply to ${ask.agent}", { if (reply.isNotBlank()) a.onAnswer(reply) })
          }
        }
      }
      AskKind.Approval -> {
        HubField(note, { note = it }, Modifier.fillMaxWidth().padding(bottom = 8.dp), hint = "Note for ${ask.tool ?: ask.agent}, optional", textStyle = Hub.type.meta.copy(fontSize = 15.sp), minHeight = 48.dp, description = "Note for ${ask.agent}")
        Row(Modifier.fillMaxWidth().padding(bottom = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
          HubButton("Decline", { a.onDecline(note) }, Modifier.weight(1f), size = ButtonSize.Large, contentDescription = "Decline ${ask.agent}: ${ask.sentence}")
          HubButton("Approve", { a.onApprove(note) }, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large, contentDescription = "Approve ${ask.agent}: ${ask.sentence}")
        }
      }
      AskKind.Idea -> {
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
          HubField(
            reply, { reply = it }, Modifier.weight(1f), hint = "Add a note", description = "Note on the idea",
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send), onAction = { if (reply.isNotBlank()) a.onNote(reply) },
          )
          Box(Modifier.size(52.dp).border(1.dp, c.lineStrong, ControlShape), contentAlignment = Alignment.Center) {
            HubIconButton(G.Send, "Add the note to the idea", { if (reply.isNotBlank()) a.onNote(reply) })
          }
        }
        Row(Modifier.fillMaxWidth().padding(bottom = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
          HubButton("Clear", a.onClear, Modifier.weight(1f), size = ButtonSize.Large, contentDescription = "Clear the idea: ${ask.sentence}")
          HubButton("Resolve", a.onResolve, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large, contentDescription = "Resolve the idea: ${ask.sentence}")
        }
      }
      AskKind.Joining -> Row(Modifier.fillMaxWidth().padding(bottom = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        HubButton("Refuse", { a.onAdmit(false, name, emptySet()) }, Modifier.weight(1f), size = ButtonSize.Large)
        HubButton("Approve", { a.onAdmit(true, name, granted) }, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large)
      }
    }
  }
}

@Composable
private fun ThreadBlock(who: String, text: String, dashed: Boolean = false, trailing: (@Composable () -> Unit)? = null) {
  val c = Hub.colors
  Column(
    Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp).fillMaxWidth().drawBehind {
      drawLine(c.line, Offset(1.dp.toPx(), 0f), Offset(1.dp.toPx(), size.height), 2.dp.toPx(), pathEffect = if (dashed) PathEffect.dashPathEffect(floatArrayOf(6f, 6f)) else null)
    }.padding(start = 14.dp, top = 4.dp, bottom = 4.dp),
  ) {
    Row(verticalAlignment = Alignment.CenterVertically) {
      Text(who, style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.weight(1f))
      trailing?.invoke()
    }
    Text(text, style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink), modifier = Modifier.padding(top = 2.dp))
  }
}

@Composable
private fun ColumnScope.QueuedItem(ui: ItemUi.Queued, fmt: Fmt, a: ItemActions) {
  val c = Hub.colors
  TopBar(AskText.kindLabel(ui.ask.kind), a)
  Banner(G.Offline) { Text(buildAnnotatedString { append("Offline. Your answer waits on this phone and sends when "); withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append("the hub") }; append(" is back.") }, style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink)) }
  Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
    Row(Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
      Text(AskText.kindMark(ui.ask.kind) + " " + AskText.kindLabel(ui.ask.kind), style = Hub.type.metaStrong.copy(fontSize = 13.sp, color = c.ink2))
      QueuedLine("Waiting to send")
    }
    TitleAndMeta(ui.ask, fmt)
    ui.ask.prose?.let { Text(it, style = Hub.type.body.copy(lineHeight = 24.8.sp, color = c.ink), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp)) }
    ThreadBlock("You · ${fmt.time(ui.queued.queuedAt)}, not sent yet", ui.queued.display)
    Text(
      "If the ask closes before the hub is back, because it was answered elsewhere or its deadline passed, the hub refuses the answer. This phone then keeps your text and tells you.",
      style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp),
    )
  }
  ActionBar {
    Row(Modifier.fillMaxWidth().padding(top = 12.dp, bottom = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
      HubButton("Cancel answer", a.onDiscard, Modifier.weight(1f), size = ButtonSize.Large)
      HubButton("Edit", { a.onEdit(ui.queued.text.orEmpty()) }, Modifier.weight(1f), size = ButtonSize.Large)
    }
  }
}

@Composable
private fun ColumnScope.RefusedItem(ui: ItemUi.Refused, fmt: Fmt, a: ItemActions) {
  val c = Hub.colors
  TopBar(AskText.kindLabel(ui.ask.kind), a)
  val why =
    when (val r = ui.refusal) {
      is Refusal.AnsweredElsewhere -> "This ask was answered at ${fmt.time(r.at)}, before this phone was back online."
      is Refusal.ClosedAtDeadline -> "This ask closed at ${fmt.time(r.at)} with no answer, before this phone was back online."
    }
  Banner(G.Warn) { Text(buildAnnotatedString { withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append("Your answer was not sent.") }; append(" $why") }, style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink)) }
  Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
    Row(Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
      Text(AskText.kindMark(ui.ask.kind) + " " + AskText.kindLabel(ui.ask.kind), style = Hub.type.metaStrong.copy(fontSize = 13.sp, color = c.ink2))
      StateWord(if (ui.refusal is Refusal.ClosedAtDeadline) "Closed" else "Answered", Dot.Filled)
    }
    TitleAndMeta(ui.ask, fmt, title = ui.ask.sentence)
    val r = ui.refusal
    if (r is Refusal.AnsweredElsewhere && r.answer != null) ThreadBlock("Answered · ${fmt.time(r.at)}", r.answer)
    ThreadBlock("Yours, not sent · ${fmt.time(ui.queued.queuedAt)}", ui.queued.display, dashed = true) {
      HubIconButton(G.Copy, "Copy your answer", { a.onCopy(ui.queued.display) }, glyphSize = 20.dp)
    }
    Text("If an ask closes at its deadline instead, this line says so: “It closed at 18:00 with no answer.”", style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp))
  }
  ActionBar {
    Row(Modifier.fillMaxWidth().padding(top = 12.dp, bottom = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
      HubButton("Discard my answer", a.onDiscard, Modifier.weight(1f), size = ButtonSize.Large)
      ui.ask.receipts.session?.let { s -> HubButton("Open the session", { a.onOpenSession(s) }, Modifier.weight(1f), size = ButtonSize.Large) }
    }
  }
}

@Composable
private fun ColumnScope.ResolvedItem(ui: ItemUi.Resolved, fmt: Fmt, a: ItemActions) {
  val c = Hub.colors
  val ask = ui.ask
  TopBar(AskText.kindLabel(ask.kind), a)
  Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
    KindLine(ask.kind) { StateWord(outcomeWord(ask.status), Dot.Filled) }
    TitleAndMeta(ask, fmt)
    Text("This was decided on another device before you opened it. Nothing is left to do here. The notification is gone from the shade.", style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp))
    FactList(
      buildList {
        ask.decidedAt?.let { add("Decided" to { FactValue("${fmt.time(it)} · ${outcomeWord(ask.status).lowercase()}") }) }
        ask.receipts.session?.let { s -> add("Session" to { FactValue(s, link = true, suffix = ask.receipts.sessionEvents?.let { "· $it events" }, onClick = { a.onOpenSession(s) }) }) }
        ask.receipts.eventId?.let { add("Event" to { FactValue(it, link = true) }) }
      },
      Modifier.padding(top = 20.dp),
    )
  }
  ActionBar {
    Row(Modifier.fillMaxWidth().padding(top = 12.dp, bottom = 4.dp)) {
      if (ui.stillNeedYou > 0) Text("${ui.stillNeedYou} still need you", style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.heightIn(min = 36.dp))
    }
    HubButton(if (ui.stillNeedYou > 0) "Next ask" else "Back to Now", a.onNext, Modifier.fillMaxWidth(), kind = ButtonKind.Primary, size = ButtonSize.Large)
  }
}
