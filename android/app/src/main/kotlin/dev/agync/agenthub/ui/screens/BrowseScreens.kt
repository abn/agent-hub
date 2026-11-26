package dev.agync.agenthub.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.data.HubState
import dev.agync.agenthub.model.EventKind
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.ProjectSummary
import dev.agync.agenthub.model.SessionState
import dev.agync.agenthub.model.SessionSummary
import dev.agync.agenthub.ui.components.EventRow
import dev.agync.agenthub.ui.components.HubRow
import dev.agync.agenthub.ui.components.SessionRow
import dev.agync.agenthub.ui.theme.ButtonKind
import dev.agync.agenthub.ui.theme.ButtonSize
import dev.agync.agenthub.ui.theme.CollapsedBar
import dev.agync.agenthub.ui.theme.Dot
import dev.agync.agenthub.ui.theme.Foot
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton
import dev.agync.agenthub.ui.theme.HubChip
import dev.agync.agenthub.ui.theme.HubIconButton
import dev.agync.agenthub.ui.theme.HubSwitcher
import dev.agync.agenthub.ui.theme.HubTabs
import dev.agync.agenthub.ui.theme.SectionLabel
import dev.agync.agenthub.ui.theme.SmallTopBar
import dev.agync.agenthub.ui.theme.StateWord
import dev.agync.agenthub.ui.theme.StatusPill
import dev.agync.agenthub.ui.theme.TitleBlock
import dev.agync.agenthub.ui.theme.ToolsRow
import dev.agync.agenthub.ui.theme.hairlineBottom
import dev.agync.agenthub.ui.theme.statusBarPadding
import dev.agync.agenthub.viewmodel.Load
import dev.agync.agenthub.viewmodel.ProjectViewModel
import dev.agync.agenthub.viewmodel.SessionFilter
import dev.agync.agenthub.viewmodel.SessionViewModel
import java.time.Instant

/** A top-level screen's header: hub switcher and two actions, then the large title and its meta line. */
@Composable
fun TopLevelHeader(title: String, meta: String, hubName: String, onSwitchHub: () -> Unit, actions: @Composable androidx.compose.foundation.layout.RowScope.() -> Unit) {
  Column(Modifier.fillMaxWidth()) {
    Row(Modifier.fillMaxWidth().statusBarPadding().height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
      HubSwitcher(hubName, onSwitchHub)
      Box(Modifier.weight(1f))
      actions()
    }
    TitleBlock(title, meta)
  }
}

@Composable
fun ProjectsScreen(
  projects: Load<List<ProjectSummary>>,
  state: HubState,
  waiting: Map<String, Pair<Int, Int>>,
  spaces: Int,
  fmt: Fmt,
  now: Instant,
  hubName: String,
  onSwitchHub: () -> Unit,
  onOpen: (String) -> Unit,
  onSearch: () -> Unit,
  onNew: () -> Unit,
  onRetry: () -> Unit,
  modifier: Modifier = Modifier,
) {
  val c = Hub.colors
  var tab by rememberSaveable { mutableIntStateOf(0) }
  Column(modifier.fillMaxSize().background(c.bg)) {
    TopLevelHeader("Projects", (projects as? Load.Ready)?.value?.let { "${it.size} projects · $spaces member spaces" } ?: "", hubName, onSwitchHub) {
      HubIconButton(G.Search, "Search", onSearch)
    }
    HubTabs(listOf("Projects" to (projects as? Load.Ready)?.value?.size, "Member spaces" to spaces), tab, { tab = it })
    when (projects) {
      Load.Loading -> Unit
      Load.Unreachable -> RetryNote("Projects could not be loaded.", onRetry)
      is Load.Ready -> if (tab == 0) LazyColumn(Modifier.weight(1f)) {
        items(projects.value, key = { it.name }) { p ->
          val w = waiting[p.name]
          ProjectRow(p, w, fmt, now) { onOpen(p.name) }
        }
      } else Text("Member spaces hold what each agent keeps for itself. Open them in the web app.", style = Hub.type.body.copy(color = c.ink2), modifier = Modifier.padding(24.dp))
    }
  }
}

@Composable
fun RetryNote(text: String, onRetry: () -> Unit) {
  Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
    Text(text, style = Hub.type.body.copy(color = Hub.colors.ink2))
    HubButton("Try again", onRetry, glyph = G.Refresh)
  }
}

@Composable
private fun ProjectRow(p: ProjectSummary, waiting: Pair<Int, Int>?, fmt: Fmt, now: Instant, onClick: () -> Unit) {
  val c = Hub.colors
  val needs = waiting?.first ?: 0
  val wait = waiting?.second ?: 0
  HubRow(
    glyph = { Glyph(if (p.confidential) G.Lock else G.Folder, tint = c.ink2) },
    onClick = onClick, clickLabel = "Open ${p.name}",
    head = {
      Row(verticalAlignment = Alignment.CenterVertically) {
        Text(p.name, style = Hub.type.bodyStrong.copy(color = c.ink))
        if (p.confidential) Text("confidential", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(start = 8.dp))
        Box(Modifier.weight(1f))
        when {
          needs > 0 -> Text("$needs needs you", style = Hub.type.mono.copy(color = c.accent, fontWeight = FontWeight.Medium))
          wait > 0 -> Text("$wait can wait", style = Hub.type.mono.copy(color = c.ink2, fontWeight = FontWeight.Medium))
        }
      }
    },
  ) {
    val sessionsText = when {
      p.live > 0 && p.idle > 0 -> "${p.live} live, ${p.idle} idle"
      p.live > 0 -> "${p.live} live"
      p.idle > 0 -> "${p.idle} idle"
      else -> "no sessions"
    }
    Text("$sessionsText · ${Fmt.bytes(p.bytes)} · ${p.lastActivity?.let { fmt.ago(it, now) + if (java.time.Duration.between(it, now).toDays() < 1) " ago" else "" } ?: ""}", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
  }
}

/** A project: Feed, Sessions, Artifacts. Wiki and brain open in the web app. */
@Composable
fun ProjectScreen(
  vm: ProjectViewModel,
  state: HubState,
  fmt: Fmt,
  now: Instant,
  onBack: () -> Unit,
  onSearch: () -> Unit,
  onMore: () -> Unit,
  onOpenAsk: (String) -> Unit,
  onOpenSession: (String) -> Unit,
  onOpenArtifact: (String) -> Unit,
  modifier: Modifier = Modifier,
  initialTab: Int = 0,
) {
  val c = Hub.colors
  var tab by rememberSaveable { mutableIntStateOf(initialTab) }
  val feed by vm.feed.state.collectAsState()
  val sessions by vm.sessions.state.collectAsState()
  val artifacts by vm.artifacts.state.collectAsState()
  val filter by vm.sessionFilter.collectAsState()
  val liveCount = (sessions as? Load.Ready)?.value?.count { it.state == SessionState.Live && it.parent == null } ?: 0
  val sub = buildString { append("$liveCount live"); (sessions as? Load.Ready)?.value?.sumOf { it.bytes }?.let { append(" · ${Fmt.bytes(it)}") } }
  Column(modifier.fillMaxSize().background(c.bg)) {
    SmallTopBar(vm.project, subtitle = sub, onBack = onBack) {
      HubIconButton(G.Search, "Search", onSearch)
    }
    val projectArtifacts = (artifacts as? Load.Ready)?.value?.filter { it.project == vm.project }.orEmpty()
    HubTabs(listOf("Feed" to null, "Sessions" to (sessions as? Load.Ready)?.value?.size, "Artifacts" to projectArtifacts.size), tab, { tab = it })
    when (tab) {
      0 -> LazyColumn(Modifier.weight(1f)) {
        val events = (feed as? Load.Ready)?.value.orEmpty()
        if (events.isNotEmpty()) item { SectionLabel("Today", note = fmt.day(now).uppercase(), topPadding = 16.dp) }
        items(events, key = { it.id }) { e ->
          val ask = e.askId?.let { id -> state.snapshot?.asks?.firstOrNull { it.id == id && it.waiting } }
          EventRow(
            e, fmt.stamp(e.at, now), who = listOfNotNull(e.tool, e.machine).joinToString(" · ").ifEmpty { null },
            onClick = ask?.let { a -> { onOpenAsk(a.id) } },
            trailingContent = if (ask != null) ({
              Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                StatusPill("Waiting on you")
                HubButton("Answer", { onOpenAsk(ask.id) }, size = ButtonSize.Small)
              }
            }) else null,
          )
        }
      }
      1 -> Column(Modifier.weight(1f)) {
        val all = (sessions as? Load.Ready)?.value.orEmpty()
        ToolsRow {
          HubChip("Live", filter == SessionFilter.Live, { vm.sessionFilter.value = SessionFilter.Live }, count = all.count { it.state == SessionState.Live })
          HubChip("All", filter == SessionFilter.All, { vm.sessionFilter.value = SessionFilter.All }, count = all.size)
          HubChip("Ended", filter == SessionFilter.Ended, { vm.sessionFilter.value = SessionFilter.Ended }, count = all.count { it.state == SessionState.Ended })
        }
        LazyColumn(Modifier.weight(1f)) {
          items(vm.tree(vm.filtered(all, filter)), key = { it.id }) { s -> SessionRow(s, now, fmt, onClick = { onOpenSession(s.id) }, facts = true, participants = s.parent == null) }
        }
      }
      else -> LazyColumn(Modifier.weight(1f)) {
        items(projectArtifacts, key = { it.id }) { a -> ArtifactRow(a, fmt, now) { onOpenArtifact(a.id) } }
      }
    }
  }
}

@Composable
fun SessionScreen(vm: SessionViewModel, fmt: Fmt, now: Instant, onBack: () -> Unit, onMore: () -> Unit, onOpenWeb: () -> Unit, onCopy: (String) -> Unit, onEnd: () -> Unit, modifier: Modifier = Modifier) {
  val c = Hub.colors
  val load by vm.session.state.collectAsState()
  var tab by rememberSaveable { mutableIntStateOf(0) }
  Column(modifier.fillMaxSize().background(c.bg)) {
    val s = (load as? Load.Ready)?.value
    SmallTopBar(s?.name.orEmpty(), subtitle = s?.let { "${it.project} · ${Fmt.bytes(it.bytes)}" }, onBack = onBack)
    HubTabs(listOf("Overview" to null, "Feed" to s?.events), tab, { tab = it })
    if (s != null && tab == 0) Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      Row(Modifier.padding(start = 16.dp, end = 4.dp, top = 6.dp), horizontalArrangement = Arrangement.spacedBy(16.dp), verticalAlignment = Alignment.CenterVertically) {
        StateWord(s.state.name.lowercase(), when (s.state) { SessionState.Live -> Dot.Live; SessionState.Idle -> Dot.Idle; SessionState.Ended -> Dot.Filled })
        Text("started ${s.startedAt?.let(fmt::time).orEmpty()} · ${s.id.take(4)}…${s.id.takeLast(4)}", style = Hub.type.mono.copy(color = c.ink3), modifier = Modifier.weight(1f))
        HubIconButton(G.Copy, "Copy session id", { onCopy(s.id) }, glyphSize = 20.dp)
      }
      SectionLabel("Participants", count = s.participants.size, topPadding = 8.dp)
      s.participants.forEach { p ->
        Row(Modifier.padding(horizontal = 16.dp, vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
          Glyph(G.Agent, tint = c.ink2)
          Column {
            Text("${p.tool} · ${p.machine}", style = Hub.type.mono.copy(fontSize = 14.sp, color = c.ink))
            p.lastWrite?.let { Text("last write ${fmt.time(it)}", style = Hub.type.meta.copy(color = c.ink2)) }
          }
        }
      }
      s.anchor?.let { a ->
        SectionLabel("Latest anchor")
        Column(Modifier.padding(horizontal = 16.dp).fillMaxWidth().hairlineBottom(c.line).padding(vertical = 14.dp)) {
          Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(a.phase, style = Hub.type.mono.copy(fontWeight = FontWeight.Medium, color = c.ink), modifier = Modifier.background(c.sunk, dev.agync.agenthub.ui.theme.ControlShape).padding(horizontal = 8.dp, vertical = 2.dp))
            Text("${a.by} · ${fmt.time(a.at)}", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink3))
          }
          Text(a.summary, style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 22.sp, color = c.ink), modifier = Modifier.padding(top = 10.dp))
          a.steps.forEachIndexed { i, st -> Text("${i + 1}. $st", style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 24.sp, color = c.ink), modifier = Modifier.padding(top = if (i == 0) 8.dp else 0.dp)) }
        }
      }
      dev.agync.agenthub.ui.components.SettingsRow(G.Kb, "Brain", Modifier.padding(top = 12.dp), detail = androidx.compose.ui.text.AnnotatedString("${s.brainEntries} entries. Read one from an ask's receipts; browse them in the web app."), end = null)
      if (s.state != SessionState.Ended) {
        Row(Modifier.padding(start = 16.dp, top = 16.dp)) { HubButton("End session…", onEnd) }
        Text("Once it has ended, Prune appears here.", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp))
      } else {
        Row(Modifier.padding(start = 16.dp, top = 16.dp)) { HubButton("Prune this session…", onEnd, kind = ButtonKind.Danger) }
      }
    }
    if (s != null && tab == 1) Text("The session's feed reads best in the web app.", style = Hub.type.body.copy(color = c.ink2), modifier = Modifier.padding(24.dp))
  }
}
