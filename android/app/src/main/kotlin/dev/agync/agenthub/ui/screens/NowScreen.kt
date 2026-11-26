package dev.agync.agenthub.ui.screens

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.pulltorefresh.PullToRefreshDefaults
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.QueuedDecision
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.NowFilter
import dev.agync.agenthub.model.NowModel
import dev.agync.agenthub.ui.components.AskRow
import dev.agync.agenthub.ui.components.EventRow
import dev.agync.agenthub.ui.components.GlyphList
import dev.agync.agenthub.ui.components.HeroNote
import dev.agync.agenthub.ui.components.HubRow
import dev.agync.agenthub.ui.components.LedeText
import dev.agync.agenthub.ui.components.QueuedLine
import dev.agync.agenthub.ui.components.SessionRow
import dev.agync.agenthub.ui.components.agentSentence
import dev.agync.agenthub.ui.theme.Banner
import dev.agync.agenthub.ui.theme.statusBarPadding
import dev.agync.agenthub.ui.theme.ButtonKind
import dev.agync.agenthub.ui.theme.ButtonSize
import dev.agync.agenthub.ui.theme.CollapsedBar
import dev.agync.agenthub.ui.theme.Foot
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton
import dev.agync.agenthub.ui.theme.HubChip
import dev.agync.agenthub.ui.theme.HubIconButton
import dev.agync.agenthub.ui.theme.HubSwitcher
import dev.agync.agenthub.ui.theme.SectionLabel
import dev.agync.agenthub.ui.theme.SmallTopBar
import dev.agync.agenthub.ui.theme.TextAction
import dev.agync.agenthub.ui.theme.TitleBlock
import dev.agync.agenthub.ui.theme.ToolsRow
import dev.agync.agenthub.viewmodel.NowScreenKind
import dev.agync.agenthub.viewmodel.NowUi

/** What Now does with a tap, supplied by the host so a fingerprint or a tablet pane can sit in between. */
data class NowActions(
  val onOpenAsk: (Ask) -> Unit,
  val onVerb: (Ask, String) -> Unit,
  val onFilter: (NowFilter) -> Unit,
  val onRefresh: () -> Unit,
  val onSwitchHub: () -> Unit,
  val onSearch: () -> Unit,
  val onMore: () -> Unit,
  val onSessions: () -> Unit,
  val onMarkSeen: () -> Unit,
  val onCancelQueued: (String) -> Unit,
  val onScanFresh: () -> Unit,
  val onPairAgain: () -> Unit,
  val onDetails: () -> Unit,
  val onCopy: (String) -> Unit,
  val onOpenSession: (String) -> Unit,
  /** Removes a hub that no longer accepts this phone, with everything this phone kept for it. */
  val onForget: () -> Unit = {},
)

@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
@Composable
fun NowScreen(ui: NowUi, fmt: Fmt, actions: NowActions, modifier: Modifier = Modifier, refreshing: Boolean = false, selectedAskId: String? = null, bar: Boolean = true, initialScrollIndex: Int = 0) {
  val c = Hub.colors
  when (val k = ui.kind) {
    is NowScreenKind.Unreachable -> UnreachableScreen(ui, k, actions)
    is NowScreenKind.Unpaired -> UnpairedScreen(ui, k, actions)
    is NowScreenKind.TooOld -> TooOldScreen(ui, k, actions)
    is NowScreenKind.Loading -> LoadingScreen(ui, actions, bar)
    else -> {
      val state = rememberLazyListState(initialFirstVisibleItemIndex = initialScrollIndex)
      val collapsed by remember { derivedStateOf { state.firstVisibleItemIndex > 0 || state.firstVisibleItemScrollOffset > 120 } }
      Column(modifier.fillMaxSize().background(c.bg)) {
        if (bar) {
          if (collapsed) {
            CollapsedBar("Now") {
              HubIconButton(G.Search, "Search", actions.onSearch)
            }
          } else {
            Row(Modifier.fillMaxWidth().statusBarPadding().padding(horizontal = 4.dp).height(56.dp), verticalAlignment = Alignment.CenterVertically) {
              HubSwitcher(ui.hubName, actions.onSwitchHub)
              Box(Modifier.weight(1f))
              HubIconButton(G.Search, "Search", actions.onSearch)
            }
          }
        }
        // The list moves down as it is pulled, so the indicator has its own room above it and never covers the chips.
        val pull = rememberPullToRefreshState()
        PullToRefreshBox(
          refreshing, actions.onRefresh, Modifier.weight(1f), state = pull,
          indicator = { PullToRefreshDefaults.Indicator(pull, refreshing, Modifier.align(Alignment.TopCenter)) },
        ) {
          Box(Modifier.fillMaxSize().graphicsLayer { translationY = pull.distanceFraction * PULL_THRESHOLD.toPx() }) {
            NowList(ui, fmt, actions, state, selectedAskId, showTitle = bar)
          }
        }
      }
    }
  }
}

/** The list pane's header on a tablet: the hub switcher and search, no large title. */
@Composable
fun TabletListHeader(hubName: String, onSwitchHub: () -> Unit, onSearch: () -> Unit) {
  Row(Modifier.fillMaxWidth().statusBarPadding().height(64.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
    HubSwitcher(hubName, onSwitchHub)
    Box(Modifier.weight(1f))
    HubIconButton(G.Search, "Search", onSearch)
  }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun NowList(ui: NowUi, fmt: Fmt, actions: NowActions, state: LazyListState, selectedAskId: String?, showTitle: Boolean = true) {
  val m = ui.model
  val c = Hub.colors
  val all = ui.filter == NowFilter.All
  LazyColumn(Modifier.fillMaxSize(), state = state) {
    if (showTitle) item(key = "title") { TitleBlock("Now", ui.meta) }
    if (ui.offlineBanner != null) {
      item(key = "banner") {
        Banner(G.Offline, actions = {
          TextAction("Try again", actions.onRefresh)
          TextAction("Details", actions.onDetails)
        }) { Text(boldLead(ui.offlineBanner, "Cannot reach ${ui.hubName}.", c), style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 21.sp, color = c.ink)) }
      }
    }
    stickyHeader(key = "tools") {
      ToolsRow(Modifier.background(c.bg)) {
        HubChip("All", ui.filter == NowFilter.All, { actions.onFilter(NowFilter.All) }, count = m.allCount)
        HubChip("Needs you", ui.filter == NowFilter.NeedsYou, { actions.onFilter(NowFilter.NeedsYou) }, count = m.needsYou.size, countAccent = true)
        if (ui.sinceLabel == "Waiting to send") HubChip("Waiting to send", ui.filter == NowFilter.WaitingToSend, { actions.onFilter(NowFilter.WaitingToSend) }, count = m.waitingToSend.size)
        else HubChip("Since you looked", ui.filter == NowFilter.Since, { actions.onFilter(NowFilter.Since) }, count = m.since.size)
      }
    }
    if (all && ui.offlineBanner == null) item(key = "lede") { LedeText(m.lede) }
    if (all && m.needsYou.isEmpty() && ui.offlineBanner == null) {
      item(key = "empty-help") {
        Text(
          "When an agent asks a question or wants a decision, it lands here and on your phone's shade. You can leave the app closed.",
          style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2), modifier = Modifier.padding(horizontal = 16.dp),
        )
      }
    }
    if ((all || ui.filter == NowFilter.NeedsYou) && m.needsYou.isNotEmpty()) {
      item(key = "needs-label") { SectionLabel("Needs you", count = m.needsYou.size, accent = true) }
      items(m.needsYou, key = { "ask-" + it.id }) { ask ->
        AskRow(ask, fmt.stamp(ask.askedAt, ui.now), { actions.onOpenAsk(ask) }, { v -> actions.onVerb(ask, v) }, selected = ask.id == selectedAskId, receipts = receiptsLine(ask, fmt, ui), note = if (ui.offlineBanner != null && ask.kind == dev.agync.agenthub.model.AskKind.Approval) "A decision waits on this phone until the hub is back. If the ask closes first, it is not sent and you are told." else null)
      }
    }
    if ((all || ui.filter == NowFilter.NeedsYou) && m.canWait.isNotEmpty()) {
      item(key = "wait-label") { SectionLabel("Can wait", count = m.canWait.size) }
      items(m.canWait, key = { "wait-" + it.id }) { ask ->
        AskRow(ask, fmt.stamp(ask.askedAt, ui.now), { actions.onOpenAsk(ask) }, { v -> actions.onVerb(ask, v) }, selected = ask.id == selectedAskId, glyphOverride = G.Clock, sub = "Snoozed on this ${deviceWord()} until ${ui.snoozeUntil(ask, fmt)}")
      }
    }
    if ((all || ui.filter == NowFilter.WaitingToSend) && m.waitingToSend.isNotEmpty()) {
      item(key = "send-label") { SectionLabel("Waiting to send", count = m.waitingToSend.size) }
      items(m.waitingToSend, key = { "send-" + it.queued.askId }) { row ->
        QueuedAskRow(row.queued, row.ask, fmt, ui, actions)
      }
    }
    if (all && m.live.isNotEmpty()) {
      item(key = "live-label") { SectionLabel("Live", count = m.live.count { it.parent == null }, endText = "Sessions", onEnd = actions.onSessions) }
      items(m.live, key = { "live-" + it.id }) { s -> SessionRow(s, ui.now, fmt, onClick = { actions.onOpenSession(s.id) }) }
    }
    if ((all || ui.filter == NowFilter.Since) && m.since.isNotEmpty() && ui.sinceLabel == "Since you looked") {
      item(key = "since-label") { SectionLabel("Since you looked", count = m.since.size, endText = "Mark seen", onEnd = actions.onMarkSeen) }
      items(m.since, key = { "since-" + it.id }) { e -> EventRow(e, fmt.stamp(e.at, ui.now)) }
    }
    if (all && m.needsYou.isEmpty() && m.answeredToday.isNotEmpty()) {
      item(key = "earlier-label") { SectionLabel("Earlier today") }
      items(m.answeredToday, key = { "done-" + it.id }) { e -> EventRow(e, fmt.stamp(e.at, ui.now)) }
    }
    if (ui.refused.isNotEmpty() && all) {
      item(key = "refused-label") { SectionLabel("Not sent", count = ui.refused.size) }
      items(ui.refused, key = { "refused-" + it.askId }) { q -> NotSentRow(q, fmt, actions) }
    }
    item(key = "end") { Box(Modifier.padding(bottom = 24.dp)) }
  }
}

private fun NowUi.snoozeUntil(ask: Ask, fmt: Fmt): String = fmt.time(snoozes[ask.id] ?: now)

private fun receiptsLine(ask: Ask, fmt: Fmt, ui: NowUi): String? {
  val e = ask.enrolment
  val parts =
    if (e != null) listOfNotNull(e.firstAgent, e.system, e.from)
    else listOfNotNull(ask.tool, ask.machine, ask.project, ask.deadline?.let { "due ${fmt.time(it)}" })
  return parts.takeIf { it.isNotEmpty() }?.joinToString(" · ")
}

@Composable
private fun QueuedAskRow(q: QueuedDecision, ask: Ask?, fmt: Fmt, ui: NowUi, actions: NowActions) {
  val c = Hub.colors
  HubRow(
    glyph = { Text(if (ask?.kind == dev.agync.agenthub.model.AskKind.Approval) "!" else "?", style = Hub.type.mono.copy(fontSize = 18.sp, color = c.ink2)) },
    trailing = ask?.let { fmt.stamp(it.askedAt, ui.now) },
    onClick = ask?.let { a -> { actions.onOpenAsk(a) } },
    head = {
      Column {
        Text(agentSentence(q.agent, ask?.let { dev.agync.agenthub.model.AskText.rowTail(it) } ?: "asks: ${q.sentence}", c), style = Hub.type.body.copy(color = c.ink))
        QueuedLine("Your answer, ${fmt.time(q.queuedAt)}: ${q.display}", Modifier.padding(top = 8.dp))
      }
    },
  ) {
    HubButton("Cancel answer", { actions.onCancelQueued(q.askId) }, Modifier.padding(top = 8.dp), size = ButtonSize.Small)
  }
}

@Composable
internal fun NotSentRow(q: QueuedDecision, fmt: Fmt, actions: NowActions) {
  val c = Hub.colors
  HubRow(
    glyph = { Text("?", style = Hub.type.mono.copy(fontSize = 18.sp, color = c.ink2)) },
    head = {
      Row(verticalAlignment = Alignment.Top) {
        Column(Modifier.weight(1f)) {
          Text(buildAnnotatedStringTo(q.agent, fmt.time(q.queuedAt), c), style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink))
          Text(q.display, style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(top = 2.dp))
        }
        HubIconButton(G.Copy, "Copy the answer", { actions.onCopy(q.display) }, glyphSize = 20.dp, modifier = Modifier.offset(x = 12.dp, y = (-12).dp))
      }
    },
    trailing = null,
  ) {}
}

private fun buildAnnotatedStringTo(agent: String, time: String, c: dev.agync.agenthub.ui.theme.HubColors): AnnotatedString =
  androidx.compose.ui.text.buildAnnotatedString {
    append("To ")
    withStyle(androidx.compose.ui.text.SpanStyle(fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, color = c.ink)) { append(agent) }
    append(", $time")
  }

private fun boldLead(text: String, lead: String, c: dev.agync.agenthub.ui.theme.HubColors): AnnotatedString =
  androidx.compose.ui.text.buildAnnotatedString {
    if (text.startsWith(lead)) {
      withStyle(androidx.compose.ui.text.SpanStyle(fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)) { append(lead) }
      append(text.removePrefix(lead))
    } else append(text)
    @Suppress("UNUSED_EXPRESSION") c
  }

@Composable
private fun SwitcherBar(hubName: String, actions: NowActions) {
  Row(Modifier.fillMaxWidth().statusBarPadding().padding(horizontal = 4.dp).height(64.dp), verticalAlignment = Alignment.CenterVertically) {
    HubSwitcher(hubName, actions.onSwitchHub)
    Box(Modifier.weight(1f))
    HubIconButton(G.Search, "Search", actions.onSearch)
  }
}

/** The hub has not answered yet, so there is nothing true to say about what waits. */
@Composable
private fun LoadingScreen(ui: NowUi, actions: NowActions, bar: Boolean) {
  Column(Modifier.fillMaxSize().background(Hub.colors.bg)) {
    if (bar) SwitcherBar(ui.hubName, actions)
    TitleBlock("Now", "Reaching ${ui.hubName}")
  }
}

@Composable
private fun UnreachableScreen(ui: NowUi, k: NowScreenKind.Unreachable, actions: NowActions) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize().background(c.bg)) {
    SwitcherBar(ui.hubName, actions)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      HeroNote(
        "Cannot reach ${ui.hubName}", glyph = G.Offline, topPadding = 16.dp,
        paragraphs = listOf(AnnotatedString("This phone tried every way it knows to reach the hub at ${k.attemptedAt}, and none answered." + (k.lastConnected?.let { " Last connected $it." } ?: ""))),
      )
      Column(Modifier.padding(start = 24.dp, end = 24.dp, top = 20.dp)) {
        SectionLabel("Things to check", Modifier.padding(0.dp), topPadding = 0.dp)
      }
      GlyphList(
        listOf(
          G.Wifi to AnnotatedString("Is this phone online? Wi-Fi or mobile data is enough."),
          G.Hub to AnnotatedString("Is the hub running? On the node, the service should be active."),
          G.Swap to AnnotatedString("Did the hub move to another network? Scan a fresh code from the hub's Settings, Phones, Add a phone. This phone stays paired and keeps its place."),
        ),
      )
      if (ui.unsent.isNotEmpty()) {
        SectionLabel("Waiting to send", count = ui.unsent.size)
        ui.unsent.forEach { q -> NotSentRow(q, Fmt(), actions) }
      }
    }
    Foot {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        HubButton("Scan a fresh code", actions.onScanFresh, Modifier.weight(1f), size = ButtonSize.Large, glyph = G.Qr)
        HubButton("Try again", actions.onRefresh, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large, glyph = G.Refresh)
      }
    }
  }
}

@Composable
private fun UnpairedScreen(ui: NowUi, k: NowScreenKind.Unpaired, actions: NowActions) {
  val c = Hub.colors
  val fmt = remember { Fmt() }
  Column(Modifier.fillMaxSize().background(c.bg)) {
    SwitcherBar(ui.hubName, actions)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      HeroNote(
        "This phone was unpaired", glyph = G.Key, topPadding = 16.dp,
        paragraphs = listOf(
          AnnotatedString("${ui.hubName} no longer accepts ${k.deviceName}. It was removed under the hub's Settings, Phones."),
          AnnotatedString("What this phone kept for ${ui.hubName} is cleared: its cached asks and remembered passwords."),
        ),
      )
      Row(Modifier.padding(start = 12.dp)) { TextAction("Forget this hub", actions.onForget) }
      val unsent = ui.unsent + ui.refused
      if (unsent.isNotEmpty()) {
        SectionLabel("Not sent", Modifier.padding(top = 4.dp), count = unsent.size, topPadding = 24.dp)
        unsent.forEach { q -> NotSentRow(q, fmt, actions) }
      }
    }
    Foot {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        HubButton("Switch hub", actions.onSwitchHub, Modifier.weight(1f), size = ButtonSize.Large)
        HubButton("Pair again", actions.onPairAgain, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large, glyph = G.Qr)
      }
    }
  }
}

@Composable
private fun TooOldScreen(ui: NowUi, k: NowScreenKind.TooOld, actions: NowActions) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize().background(c.bg)) {
    SwitcherBar(ui.hubName, actions)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      HeroNote(
        "${ui.hubName} needs an update", glyph = G.Refresh, topPadding = 16.dp,
        paragraphs = listOf(AnnotatedString("This phone is paired and can reach the hub, but the hub is older than this app can read. Update the hub on its node; this phone connects by itself once it is done.")),
      )
      dev.agync.agenthub.ui.components.FactList(
        buildList {
          add("Hub runs" to { dev.agync.agenthub.ui.components.FactValue(k.hubVersion) })
          add("App needs" to { dev.agync.agenthub.ui.components.FactValue(k.needs, suffix = "or later") })
          if (k.via != null) add("Connected" to { dev.agync.agenthub.ui.components.FactValue("via ${k.via}", sans = true) })
        },
        Modifier.padding(top = 24.dp), labelWidth = 120.dp,
      )
    }
    Foot {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        HubButton("Switch hub", actions.onSwitchHub, Modifier.weight(1f), size = ButtonSize.Large)
        HubButton("Check again", actions.onRefresh, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large, glyph = G.Refresh)
      }
    }
  }
}

/** How far the list travels at a full pull: the indicator's own threshold. */
private val PULL_THRESHOLD = PullToRefreshDefaults.PositionalThreshold
