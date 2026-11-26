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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.PairedHub
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.Member
import dev.agync.agenthub.model.Storage
import dev.agync.agenthub.ui.components.HubDialog
import dev.agync.agenthub.ui.components.HubRow
import dev.agync.agenthub.ui.components.SettingsRow
import dev.agync.agenthub.ui.theme.ButtonSize
import dev.agync.agenthub.ui.theme.Dot
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton
import dev.agync.agenthub.ui.theme.HubIconButton
import dev.agync.agenthub.ui.theme.HubSwitch
import dev.agync.agenthub.ui.theme.SectionLabel
import dev.agync.agenthub.ui.theme.SmallTopBar
import dev.agync.agenthub.ui.theme.StateWord
import dev.agync.agenthub.ui.theme.StatusPill
import dev.agync.agenthub.ui.theme.switchRow
import dev.agync.agenthub.viewmodel.HubViewModel
import dev.agync.agenthub.viewmodel.Load
import dev.agync.agenthub.viewmodel.MemberViewModel
import java.time.Instant

@Composable
fun HubScreen(
  vm: HubViewModel, fmt: Fmt, now: Instant, hubName: String, version: String?, via: String?,
  onSwitchHub: () -> Unit, onSearch: () -> Unit, onSettings: () -> Unit, onMember: (String) -> Unit, onStorage: () -> Unit,
  onPhones: () -> Unit, onOpenWeb: (() -> Unit)?, webAddress: String?, phones: String, onRetry: () -> Unit, modifier: Modifier = Modifier,
) {
  val c = Hub.colors
  val members by vm.members.state.collectAsState()
  val storage by vm.storage.state.collectAsState()
  Column(modifier.fillMaxSize().background(c.bg)) {
    TopLevelHeader("Hub", listOfNotNull(version, via?.let { "connected via $it" }).joinToString(" · "), hubName, onSwitchHub) {
      HubIconButton(G.Search, "Search", onSearch)
      HubIconButton(G.Gear, "Settings", onSettings)
    }
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      when (val m = members) {
        Load.Loading -> Unit
        Load.Unreachable -> RetryNote("Members could not be loaded.", onRetry)
        is Load.Ready -> {
          SectionLabel("Members", count = m.value.size, accent = m.value.any { it.pending }, topPadding = 12.dp)
          m.value.sortedByDescending { it.pending }.forEach { mem -> MemberRow(mem, fmt) { onMember(mem.name) } }
        }
      }
      SectionLabel("This hub")
      SettingsRow(G.Storage, "Storage", detail = AnnotatedString((storage as? Load.Ready)?.value?.let { "${Fmt.bytes(it.totalBytes)} used · ${Fmt.bytes(it.endedBytes)} in ended sessions" } ?: ""), onClick = onStorage, end = { Glyph(G.ChevR, tint = c.ink3) })
      SettingsRow(G.Phone, "Phones", detail = AnnotatedString(phones), onClick = onPhones, end = { Glyph(G.ChevR, tint = c.ink3) })
      SettingsRow(G.Gear, "Settings", onClick = onSettings, end = { Glyph(G.ChevR, tint = c.ink3) })
      if (onOpenWeb != null) SettingsRow(G.Globe, "Open web app", detail = webAddress?.let { AnnotatedString(it) }, onClick = onOpenWeb, end = { Glyph(G.Open, tint = c.ink3) })
    }
  }
}

@Composable
private fun MemberRow(m: Member, fmt: Fmt, onClick: () -> Unit) {
  val c = Hub.colors
  HubRow(
    glyph = { Glyph(G.Laptop, tint = if (m.pending) c.accent else c.ink2) },
    wait = m.pending, onClick = onClick, clickLabel = "Open ${m.name}",
    head = {
      Row(verticalAlignment = Alignment.CenterVertically) {
        Text(m.name, style = Hub.type.bodyStrong.copy(color = c.ink), modifier = Modifier.weight(1f))
        if (m.pending) StatusPill("Pending") else TokenLive()
      }
    },
  ) {
    if (m.pending) Text("“${m.reason.orEmpty()}” · ${m.tools.joinToString(", ")} · ${m.lastUsed?.let(fmt::time).orEmpty()}", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
    else Text("${m.tools.joinToString(", ")} · " + listOfNotNull(m.live.takeIf { it > 0 }?.let { "$it live" }, m.idle.takeIf { it > 0 }?.let { "$it idle" }).joinToString(", "), style = Hub.type.mono.copy(color = c.ink2))
  }
}

@Composable
private fun TokenLive() {
  val c = Hub.colors
  Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
    Text("token", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
    StateWord("live", Dot.Live)
  }
}

@Composable
fun MemberScreen(vm: MemberViewModel, fmt: Fmt, now: Instant, onBack: () -> Unit, onMore: () -> Unit, onReissue: () -> Unit, onRevoke: () -> Unit, onGrantActions: (String) -> Unit, modifier: Modifier = Modifier) {
  val c = Hub.colors
  val load by vm.members.state.collectAsState()
  val m = (load as? Load.Ready)?.value?.let(vm::find)
  Column(modifier.fillMaxSize().background(c.bg)) {
    SmallTopBar(vm.name, subtitle = m?.firstSeen?.let { "member · joined ${fmt.date(it)}" }, onBack = onBack)
    if (m != null) Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      Box(Modifier.padding(start = 16.dp, top = 8.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
          Text("token", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
          StateWord("live", Dot.Live, rest = "· issued ${m.firstSeen?.let(fmt::date).orEmpty()} · last used ${m.lastUsed?.let(fmt::time).orEmpty()}")
        }
      }
      SectionLabel("Agents seen in its sessions", count = m.agentsSeen.size)
      m.agentsSeen.forEach { a ->
        HubRow(glyph = { Glyph(G.Agent, tint = c.ink2) }, trailing = a.last?.let { fmt.stamp(it, now) }, head = { Text(a.tool, style = Hub.type.mono.copy(fontSize = 15.sp, fontWeight = FontWeight.Medium, color = c.ink)) }) {
          Text(a.sessions, style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
        }
      }
      SectionLabel("Confidential projects", count = m.grants.size)
      m.grants.forEach { g ->
        SettingsRow(G.Lock, g.project, detail = AnnotatedString("confidential · granted ${fmt.date(g.since)}"))
      }
      Text("Every member reads and writes every ordinary project. Only confidential projects need a grant.", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 10.dp))
    }
  }
}

data class SettingsActions(
  val onBack: () -> Unit,
  val onPairAnother: () -> Unit,
  val onSelectHub: (PairedHub) -> Unit,
  val onAlertHow: () -> Unit,
  val onAlertKinds: () -> Unit,
  val onFingerprint: (Boolean) -> Unit,
  val onHideLock: (Boolean) -> Unit,
  val onForget: (String) -> Unit,
  val onTheme: () -> Unit,
)

@Composable
fun SettingsScreen(
  hubs: List<PairedHub>, currentId: String?, via: String?, needs: Map<String, Int>, prefs: DevicePrefs, remembered: List<String>,
  actions: SettingsActions, modifier: Modifier = Modifier,
) {
  val c = Hub.colors
  Column(modifier.fillMaxSize().background(c.bg)) {
    SmallTopBar("Settings", onBack = actions.onBack)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      SectionLabel("Hubs", topPadding = 8.dp)
      hubs.forEach { h ->
        val n = needs[h.id] ?: 0
        SettingsRow(
          if (h.id == currentId) G.Hub else G.Hub, h.name, onClick = { actions.onSelectHub(h) },
          detail = buildAnnotatedString {
            append((via?.let { "Connected via $it" } ?: "Connected") + if (h.id == currentId) " · this phone is ${h.deviceName}" else "")
            if (n > 0 && h.id != currentId) { append(" · "); withStyle(SpanStyle(color = c.accent, fontFamily = dev.agync.agenthub.ui.theme.PlexMono)) { append("$n needs you") } }
          },
          end = { if (h.id == currentId) Glyph(G.Check, tint = c.ink, description = "Current hub") },
        )
      }
      SettingsRow(G.Plus, "Pair another hub", onClick = actions.onPairAnother)
      SectionLabel("Alerts")
      SettingsRow(G.Bell, "How alerts arrive", onClick = actions.onAlertHow, detail = AnnotatedString(if (prefs.stayConnected) (if (hubs.size > 1) "Stay connected, to all hubs. Each alert names its hub." else "Stay connected. Alerts arrive within seconds.") else "Only when you open the app."), end = { Glyph(G.ChevR, tint = c.ink3) })
      SettingsRow(G.Tile, "Alert kinds", onClick = actions.onAlertKinds, detail = AnnotatedString("Approvals, Questions, Joining, Finished work, Connection. Set sound and priority for each in Android."), end = { Glyph(G.Open, tint = c.ink3) })
      SectionLabel("Security")
      SettingsRow(G.Finger, "Fingerprint before approving", Modifier.switchRow(prefs.fingerprintBeforeApproving, actions.onFingerprint), detail = AnnotatedString("In the app and the shade. The lock screen always asks for an unlock."), end = { HubSwitch(prefs.fingerprintBeforeApproving, null) })
      SettingsRow(G.EyeOff, "Hide what agents ask on the lock screen", Modifier.switchRow(prefs.hideOnLockScreen, actions.onHideLock), end = { HubSwitch(prefs.hideOnLockScreen, null) })
      SettingsRow(G.Key, "Remembered project passwords", detail = AnnotatedString(remembered.joinToString(", ").ifEmpty { "None" }), end = { remembered.firstOrNull()?.let { r -> HubButton("Forget", { actions.onForget(r) }, size = ButtonSize.Small) } })
      SectionLabel("Display")
      SettingsRow(G.Sun, "Theme", onClick = actions.onTheme, detail = AnnotatedString(when (prefs.theme) { "light" -> "Light"; "dark" -> "Dark"; else -> "Follow Android" }))
    }
  }
}

@Composable
fun StorageScreen(vm: HubViewModel, fmt: Fmt, onBack: () -> Unit, modifier: Modifier = Modifier, showPrune: Boolean = false) {
  val c = Hub.colors
  val load by vm.storage.state.collectAsState()
  var confirm by remember { mutableStateOf(showPrune) }
  val st = (load as? Load.Ready)?.value
  Box(modifier.fillMaxSize().background(c.bg)) {
    Column(Modifier.fillMaxSize()) {
      SmallTopBar("Storage", subtitle = st?.let { "${Fmt.bytes(it.totalBytes)} on the node" }, onBack = onBack)
      if (st != null) Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
        StorageBar(st)
        SectionLabel("Ended sessions", count = st.ended.size, endText = if (st.ended.isNotEmpty()) "Prune all" else null, onEnd = { confirm = true }, endColor = c.danger)
        st.ended.forEach { e ->
          HubRow(glyph = { Glyph(G.Branch, tint = c.ink2) }, trailing = Fmt.bytes(e.bytes), head = { Text(buildAnnotatedString { withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = c.ink)) { append(e.name) } }, style = Hub.type.body) }) {
            StateWord("ended · ${e.project}", Dot.Filled)
          }
        }
      }
    }
    if (confirm && st != null) HubDialog(
      "Prune ${st.ended.size} ended sessions?",
      "Deletes their brains and feed rows, ${Fmt.bytes(st.endedBytes)}. Knowledge bases and artifacts stay. You can undo for 30 seconds.",
      onDismiss = { confirm = false }, confirmLabel = "Prune", destructive = true, onConfirm = { confirm = false; vm.prune() },
    )
  }
}

@Composable
private fun StorageBar(st: Storage) {
  val c = Hub.colors
  val tones = listOf(c.ink, c.ink2, c.ink3, c.knowledge)
  Row(Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 8.dp).fillMaxWidth().height(12.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
    st.segments.forEach { seg -> Box(Modifier.weight(seg.bytes.toFloat()).fillMaxSize().background(tones[seg.tone % 4], RoundedCornerShape(2.dp))) }
  }
  Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
    st.segments.forEach { seg ->
      Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(12.dp).background(tones[seg.tone % 4], RoundedCornerShape(2.dp)))
        Text(seg.label, style = Hub.type.meta.copy(color = c.ink), modifier = Modifier.weight(1f))
        Text(Fmt.bytes(seg.bytes), style = Hub.type.mono.copy(color = c.ink2))
      }
    }
  }
}
