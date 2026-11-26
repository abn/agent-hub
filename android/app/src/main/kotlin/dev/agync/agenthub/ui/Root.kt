package dev.agync.agenthub.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import dev.agync.agenthub.data.AppContainer
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.nav.Navigator
import dev.agync.agenthub.nav.Route
import dev.agync.agenthub.ui.components.HubDialog
import dev.agync.agenthub.ui.components.HubSheet
import dev.agync.agenthub.ui.components.HubSnackbar
import dev.agync.agenthub.ui.components.SettingsRow
import dev.agync.agenthub.ui.screens.ArtifactsScreen
import dev.agync.agenthub.ui.screens.SearchScreen
import dev.agync.agenthub.ui.screens.BrainEntryScreen
import dev.agync.agenthub.ui.screens.HubScreen
import dev.agync.agenthub.ui.screens.ItemActions
import dev.agync.agenthub.ui.screens.ItemScreen
import dev.agync.agenthub.ui.screens.MemberScreen
import dev.agync.agenthub.ui.screens.NowActions
import dev.agync.agenthub.ui.screens.NowScreen
import dev.agync.agenthub.ui.screens.PairActions
import dev.agync.agenthub.ui.screens.PairScreen
import dev.agync.agenthub.ui.screens.ProjectScreen
import dev.agync.agenthub.ui.screens.ProjectsScreen
import dev.agync.agenthub.ui.screens.ProtectedGate
import dev.agync.agenthub.ui.screens.SessionScreen
import dev.agync.agenthub.ui.screens.TabletListHeader
import dev.agync.agenthub.ui.screens.SettingsActions
import dev.agync.agenthub.ui.screens.SettingsScreen
import dev.agync.agenthub.ui.screens.StorageScreen
import dev.agync.agenthub.ui.screens.ViewerScreen
import dev.agync.agenthub.ui.theme.BottomNav
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubTheme
import dev.agync.agenthub.ui.theme.NavItem
import dev.agync.agenthub.ui.theme.NavRail
import dev.agync.agenthub.ui.theme.ThemeMode
import dev.agync.agenthub.ui.theme.clickableRow
import dev.agync.agenthub.ui.theme.hairlineRight
import dev.agync.agenthub.model.SearchKind
import dev.agync.agenthub.model.SessionState
import dev.agync.agenthub.viewmodel.ArtifactsViewModel
import dev.agync.agenthub.viewmodel.BrainEntryViewModel
import dev.agync.agenthub.viewmodel.HubViewModel
import dev.agync.agenthub.viewmodel.ItemEvent
import dev.agync.agenthub.viewmodel.SearchViewModel
import dev.agync.agenthub.viewmodel.Load
import dev.agync.agenthub.viewmodel.ItemUi
import dev.agync.agenthub.viewmodel.ItemViewModel
import dev.agync.agenthub.viewmodel.MemberViewModel
import dev.agync.agenthub.viewmodel.NowEvent
import dev.agync.agenthub.viewmodel.NowViewModel
import dev.agync.agenthub.viewmodel.PairStep
import dev.agync.agenthub.viewmodel.PairViewModel
import dev.agync.agenthub.viewmodel.ProjectViewModel
import dev.agync.agenthub.viewmodel.ProjectsViewModel
import dev.agync.agenthub.viewmodel.SessionViewModel
import dev.agync.agenthub.viewmodel.ViewerViewModel
import kotlinx.coroutines.launch

/** What only the host activity can do. Fakes stand in for it in tests. */
interface Platform {
  fun copy(label: String, text: String)

  fun openAppSettings()

  fun openNotificationSettings()

  /** Sends the browser to [url]: a hub's web address with a path. Only called with an http or https address. */
  fun openWebApp(url: String)

  /** Asks for the notification permission. Calls back with the answer. */
  fun requestNotifications(onResult: (Boolean) -> Unit)

  /** The fingerprint prompt. Resolves false on cancel or failure. */
  suspend fun confirmWithFingerprint(reason: String): Boolean

  fun setStayConnected(stay: Boolean)

  val protectedGate: ProtectedGate

  @Composable fun Scanner(onScanned: (String) -> Unit, onDenied: () -> Unit)
}

private val tabs =
  listOf(Route.Now to NavItem("Now", G.Now), Route.Projects to NavItem("Projects", G.Folder), Route.Artifacts to NavItem("Artifacts", G.Doc), Route.Hub to NavItem("Hub", G.Hub))

@Composable
fun AppRoot(container: AppContainer, platform: Platform, navigator: Navigator = remember { Navigator() }) {
  val prefs by container.prefs.state.collectAsState()
  val reg by container.registry.state.collectAsState()
  // Pairing registers the hub before it asks how alerts should arrive, so the flow keeps the
  // screen until it says it is done rather than following whether a hub exists.
  var pairing by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf(container.registry.state.value.current == null) }
  LaunchedEffect(reg.current == null) { if (reg.current == null) pairing = true }
  HubTheme(
    when (prefs.theme) { "light" -> ThemeMode.Light; "dark" -> ThemeMode.Dark; else -> ThemeMode.System },
  ) {
    val current = reg.current
    if (pairing || current == null) {
      PairHost(container, platform, prefs, onDone = { pairing = false }, onClose = if (current != null) ({ pairing = false }) else null)
    } else {
      MainShell(container, container.repository(current), platform, navigator, prefs, onPair = { pairing = true })
    }
  }
}

@Composable
fun PairHost(container: AppContainer, platform: Platform, prefs: DevicePrefs, onDone: () -> Unit, onClose: (() -> Unit)?) {
  val vm: PairViewModel = viewModel(factory = viewModelFactory { initializer { PairViewModel(container.pairing, container.registry, container.prefs, container.deviceName) } })
  val step by vm.step.collectAsState()
  val notice by vm.notice.collectAsState()
  LaunchedEffect(step) { if (step is PairStep.Done) { platform.setStayConnected(prefs.stayConnected); onDone() } }
  BackHandler(enabled = onClose != null || step !is PairStep.Welcome) {
    if (step is PairStep.Welcome) onClose?.invoke() else vm.back()
  }
  PairScreen(
    step = step, prefs = prefs, fmt = container.fmt, notice = notice,
    actions = PairActions(
      onStartScan = vm::startScan, onStartManual = vm::startManual, onBack = vm::back, onEditManual = vm::editManual, onSubmitManual = vm::submitManual,
      onName = vm::setDeviceName, onPair = vm::pair, onCancelPairing = vm::cancelPairing, onCheckMatch = vm::checkStringsMatch, onCheckDiffer = vm::checkStringsDiffer, onStayConnected = vm::setStayConnected,
      onAllowNotifications = { platform.requestNotifications(vm::notificationsAnswered) }, onNotNow = vm::skipAlerts,
      onOpenSettings = { if (step is PairStep.Scan) platform.openAppSettings() else platform.openNotificationSettings() },
      onPrefs = container.prefs::update,
    ),
    scanner = { platform.Scanner(onScanned = vm::scanned, onDenied = vm::cameraDenied) },
  )
}

private class Snack(val message: String, val actionLabel: String? = null, val lastsMillis: Long = 5000, val onAction: (() -> Unit)? = null)

@Composable
private fun MainShell(container: AppContainer, repo: HubRepository, platform: Platform, nav: Navigator, prefs: DevicePrefs, onPair: () -> Unit) {
  val stack by nav.stack.collectAsState()
  val route = stack.last()
  val state by repo.state.collectAsState()
  val scope = rememberCoroutineScope()
  var snack by remember { mutableStateOf<Snack?>(null) }
  var dialog by remember { mutableStateOf<(@Composable () -> Unit)?>(null) }
  var sheet by remember { mutableStateOf<(@Composable () -> Unit)?>(null) }
  val fmt = container.fmt
  val nowVm: NowViewModel = viewModel(key = "now-" + repo.hub.id, factory = viewModelFactory { initializer { NowViewModel(repo, fmt, container.clock) } })
  val nowUi by nowVm.ui.collectAsState()
  val badge = nowUi.model.badge

  LaunchedEffect(repo.hub.id) { repo.refresh() }
  LaunchedEffect(Unit) {
    nowVm.events.collect { e ->
      snack = when (e) {
        is NowEvent.Snoozed -> Snack("Snoozed on this phone until ${e.until}", "Undo") { nowVm.undoSnooze(e.askId); snack = null }
        is NowEvent.Queued -> Snack(e.what)
        is NowEvent.Refused -> Snack(e.what)
      }
    }
  }
  BackHandler(enabled = stack.size > 1) { nav.pop() }

  /** Runs [action] behind the fingerprint when the person asked for that. */
  fun gated(reason: String, action: () -> Unit) {
    if (!prefs.fingerprintBeforeApproving) action() else scope.launch { if (platform.confirmWithFingerprint(reason)) action() }
  }

  fun onVerb(ask: Ask, verb: String) {
    when (ask.kind) {
      AskKind.Approval -> gated("${verb} ${ask.agent}") { nowVm.decide(ask, verb == "Approve") }
      AskKind.Question -> nowVm.answer(ask, verb)
      AskKind.Joining -> nav.push(Route.Item(ask.id))
      AskKind.Idea -> if (verb == "Resolve") nowVm.resolveIdea(ask) else nowVm.clearIdea(ask)
    }
  }

  val nowActions = NowActions(
    onOpenAsk = { nav.push(Route.Item(it.id)) }, onVerb = ::onVerb, onFilter = nowVm::setFilter, onRefresh = nowVm::refresh,
    onSwitchHub = { sheet = { HubSwitchSheet(container, repo.hub.id, { sheet = null }, onPair) } },
    onSearch = { nav.push(Route.Search) },
    onMore = {},
    onSessions = { nav.selectTab(Route.Projects) }, onMarkSeen = nowVm::markSeen, onCancelQueued = nowVm::cancelQueued,
    onScanFresh = onPair, onPairAgain = onPair, onForget = { container.forget(repo.hub.id) }, onDetails = { snack = Snack("Cannot reach ${repo.hub.name}. Tendril tries every way it knows.") },
    onCopy = { platform.copy("answer", it) }, onOpenSession = { nav.push(Route.Session(it)) },
  )

  BoxWithConstraints(Modifier.fillMaxSize().background(Hub.colors.bg)) {
    val wide = maxWidth >= 840.dp
    val selectedIdx = tabs.indexOfFirst { it.first == nav.tab }
    val items = tabs.map { (_, item) -> if (item.label == "Now") item.copy(badge = badge) else item }
    Row(Modifier.fillMaxSize()) {
      if (wide) Box(Modifier.width(96.dp).fillMaxHeight()) { NavRail(items, selectedIdx, { nav.selectTab(tabs[it].first) }) }
      Column(Modifier.weight(1f)) {
        Box(Modifier.weight(1f).fillMaxWidth()) {
          if (wide && route == Route.Now) TabletNow(container, repo, nowVm, nowActions, nav, platform, ::gated)
          else RouteContent(route, container, repo, nowVm, nowActions, nav, platform, prefs, ::gated, { s -> snack = s }, { sheet = it }, { dialog = it }, onPair)
        }
        if (!wide && route.showsNav) BottomNav(items, selectedIdx, { nav.selectTab(tabs[it].first) })
      }
    }
    snack?.let { s ->
      Box(Modifier.align(Alignment.BottomCenter).windowInsetsPadding(WindowInsets.navigationBars).padding(bottom = if (route.showsNav && !wide) 76.dp else 12.dp)) {
        HubSnackbar(s.message, actionLabel = s.actionLabel, onAction = s.onAction)
      }
      LaunchedEffect(s) { kotlinx.coroutines.delay(s.lastsMillis); if (snack === s) snack = null }
    }
    sheet?.invoke()
    dialog?.invoke()
  }
}

@Composable
private fun HubSwitchSheet(container: AppContainer, currentId: String, onClose: () -> Unit, onPair: () -> Unit) {
  val reg by container.registry.state.collectAsState()
  HubSheet("Switch hub", onClose) {
    reg.hubs.forEach { h ->
      SettingsRow(G.Hub, h.name, Modifier.clickableRow { container.registry.select(h.id); onClose() }, detail = androidx.compose.ui.text.AnnotatedString("this phone is ${h.deviceName}"), end = { if (h.id == currentId) dev.agync.agenthub.ui.theme.Glyph(G.Check, description = "Current hub") })
    }
    SettingsRow(G.Plus, "Pair another hub", Modifier.clickableRow { onClose(); onPair() })
  }
}

@Composable
private fun RouteContent(
  route: Route, container: AppContainer, repo: HubRepository, nowVm: NowViewModel, nowActions: NowActions, nav: Navigator, platform: Platform,
  prefs: DevicePrefs, gated: (String, () -> Unit) -> Unit, snack: (Snack?) -> Unit, sheet: ((@Composable () -> Unit)?) -> Unit, dialog: ((@Composable () -> Unit)?) -> Unit, onPair: () -> Unit,
) {
  val fmt = container.fmt
  val state by repo.state.collectAsState()
  val nowUi by nowVm.ui.collectAsState()
  val now = nowUi.now
  val webUrl = state.snapshot?.info?.webUrl
  /** Opens the hub's web app at [path], when the hub told this phone where it is. */
  fun openWeb(path: String) { if (webUrl != null) platform.openWebApp(webUrl.trimEnd('/') + path) }
  val tick = rememberTicker(nowVm)
  @Suppress("UNUSED_EXPRESSION") tick
  when (route) {
    Route.Now -> NowScreen(nowUi, fmt, nowActions)
    Route.Projects -> {
      val vm: ProjectsViewModel = viewModel(key = "projects-" + repo.hub.id, factory = viewModelFactory { initializer { ProjectsViewModel(repo) } })
      val projects by vm.projects.state.collectAsState()
      ProjectsScreen(projects, state, vm.waitingByProject(state, now), 3, fmt, now, repo.hub.name, nowActions.onSwitchHub, { nav.push(Route.Project(it)) }, nowActions.onSearch, { openWeb("/projects/new") }, vm.projects::reload)
    }
    Route.Artifacts -> {
      val vm: ArtifactsViewModel = viewModel(key = "artifacts-" + repo.hub.id, factory = viewModelFactory { initializer { ArtifactsViewModel(repo) } })
      ArtifactsScreen(vm, fmt, now, repo.hub.name, nowActions.onSwitchHub, { nav.push(Route.Viewer(it.id)) }, nowActions.onSearch, {}, vm.artifacts::reload)
    }
    Route.Hub -> {
      val vm: HubViewModel = viewModel(key = "hub-" + repo.hub.id, factory = viewModelFactory { initializer { HubViewModel(repo) } })
      HubScreen(
        vm, fmt, now, repo.hub.name, state.snapshot?.info?.version, state.lastVia, nowActions.onSwitchHub, nowActions.onSearch,
        { nav.push(Route.Settings) }, { nav.push(Route.Member(it)) }, { nav.push(Route.Storage) }, { nav.push(Route.Phones) }, webUrl?.let { { openWeb("/") } }, webUrl,
        state.snapshot?.info?.phones?.joinToString(" · ") { if (it.thisPhone) it.name + ", this one" else it.name }.orEmpty(), vm.members::reload,
      )
    }
    is Route.Item -> ItemRoute(route.askId, container, repo, nav, platform, gated, sheet)
    is Route.Project -> {
      val vm: ProjectViewModel = viewModel(key = "project-" + route.name, factory = viewModelFactory { initializer { ProjectViewModel(repo, route.name) } })
      ProjectScreen(vm, state, fmt, now, { nav.pop() }, { nav.push(Route.Search) }, { openWeb("/projects/" + route.name + "/wiki") }, { nav.push(Route.Item(it)) }, { nav.push(Route.Session(it)) }, { nav.push(Route.Viewer(it)) }, initialTab = route.tab)
    }
    is Route.Session -> {
      val vm: SessionViewModel = viewModel(key = "session-" + route.id, factory = viewModelFactory { initializer { SessionViewModel(repo, route.id) } })
      val undo by vm.undo.collectAsState()
      val sessionLoad by vm.session.state.collectAsState()
      val shown = (sessionLoad as? Load.Ready)?.value
      LaunchedEffect(undo) {
        if (undo != null) {
          snack(Snack("Pruned ${shown?.name ?: "the session"}", "Undo", 30_000) { vm.undoPrune() })
          nav.pop()
        }
      }
      SessionScreen(vm, fmt, now, { nav.pop() }, {}, { openWeb("/sessions/" + route.id) }, { platform.copy("session id", it) }, {
        dialog {
          if (shown?.state == SessionState.Ended) {
            HubDialog("Prune ${shown.name}?", "Deletes its brain and feed rows. The knowledge base and artifacts stay. You can undo for 30 seconds.", { dialog(null) }, "Prune", { dialog(null); vm.prune() }, destructive = true)
          } else {
            HubDialog("End this session?", "The agent that holds it is told. Its brain stays until you prune it.", { dialog(null) }, "End session", { dialog(null); vm.end() })
          }
        }
      })
    }
    is Route.Viewer -> {
      val vm: ViewerViewModel = viewModel(key = "viewer-" + route.artifactId, factory = viewModelFactory { initializer { ViewerViewModel(repo, route.artifactId) } })
      ViewerScreen(vm, fmt, now, platform.protectedGate, repo.hub.id, { nav.pop() }, {})
    }
    Route.Search -> {
      val vm: SearchViewModel = viewModel(key = "search-" + repo.hub.id, factory = viewModelFactory { initializer { SearchViewModel(repo) } })
      SearchScreen(vm, fmt, now, { nav.pop() }, { hit ->
        when (hit.kind) {
          SearchKind.Feed -> if (state.snapshot?.asks?.any { it.id == hit.ref } == true) nav.push(Route.Item(hit.ref)) else hit.project?.let { nav.push(Route.Project(it)) }
          SearchKind.Artifact -> nav.push(Route.Viewer(hit.ref))
          SearchKind.Brain -> nav.push(Route.Session(hit.session ?: hit.ref))
          SearchKind.Knowledge -> hit.project?.let { nav.push(Route.Project(it)) }
        }
      })
    }
    is Route.Brain -> {
      val vm: BrainEntryViewModel = viewModel(key = "brain-" + route.session + route.path, factory = viewModelFactory { initializer { BrainEntryViewModel(repo, route.session, route.path) } })
      BrainEntryScreen(vm, { nav.pop() }, { platform.copy("brain entry", it) })
    }
    is Route.Member -> {
      val vm: MemberViewModel = viewModel(key = "member-" + route.name, factory = viewModelFactory { initializer { MemberViewModel(repo, route.name) } })
      MemberScreen(vm, fmt, now, { nav.pop() }, {}, { openWeb("/settings/members") }, { openWeb("/settings/members") }, { openWeb("/settings/members") })
    }
    Route.Settings -> {
      val reg by container.registry.state.collectAsState()
      SettingsScreen(
        reg.hubs, reg.currentId, state.lastVia, emptyMap(), prefs, container.vault.projects(repo.hub.id),
        SettingsActions(
          onBack = { nav.pop() }, onPairAnother = onPair, onSelectHub = { container.registry.select(it.id) }, onAlertHow = { platform.setStayConnected(!prefs.stayConnected); container.prefs.update { it.copy(stayConnected = !it.stayConnected) } },
          onAlertKinds = platform::openNotificationSettings, onFingerprint = { v -> container.prefs.update { it.copy(fingerprintBeforeApproving = v) } },
          onHideLock = { v -> container.prefs.update { it.copy(hideOnLockScreen = v) } }, onForget = { container.vault.forget(repo.hub.id, it) },
          onTheme = { container.prefs.update { it.copy(theme = when (it.theme) { "system" -> "light"; "light" -> "dark"; else -> "system" }) } },
        ),
      )
    }
    Route.Storage -> {
      val vm: HubViewModel = viewModel(key = "hub-" + repo.hub.id, factory = viewModelFactory { initializer { HubViewModel(repo) } })
      val undo by vm.undo.collectAsState()
      LaunchedEffect(undo) {
        val u = undo
        if (u == null) snack(null)
        else snack(Snack("Pruned ${u.tokens.size} ended sessions", "Undo", 30_000) { vm.undoPrune() })
      }
      StorageScreen(vm, fmt, { nav.pop() })
    }
    Route.Phones -> PhonesScreen(state.snapshot?.info?.phones.orEmpty(), { nav.pop() }, { openWeb("/settings/phones") })
    Route.PairAnother -> onPair()
  }
}

@Composable
private fun rememberTicker(vm: NowViewModel): Int {
  var minute by remember { mutableStateOf(0) }
  LaunchedEffect(Unit) {
    while (true) {
      kotlinx.coroutines.delay(60_000)
      vm.tickClock()
      minute++
    }
  }
  return minute
}

@Composable
private fun ItemRoute(askId: String, container: AppContainer, repo: HubRepository, nav: Navigator, platform: Platform, gated: (String, () -> Unit) -> Unit, sheet: ((@Composable () -> Unit)?) -> Unit, wide: Boolean = false) {
  val vm: ItemViewModel = viewModel(key = "item-$askId", factory = viewModelFactory { initializer { ItemViewModel(repo, askId, container.fmt, container.clock) } })
  val ui by vm.ui.collectAsState()
  val clipboard = platform
  var noteVersion by remember { mutableIntStateOf(0) }
  var notice by remember { mutableStateOf<String?>(null) }
  LaunchedEffect(vm) {
    vm.events.collect { e ->
      when (e) {
        is ItemEvent.Done -> if (e.nextAskId != null) nav.replace(Route.Item(e.nextAskId)) else nav.pop()
        ItemEvent.Noted -> noteVersion++
        is ItemEvent.Failed -> notice = e.message
        else -> Unit
      }
    }
  }
  val ask = (ui as? ItemUi.Open)?.ask
  ItemScreen(
    ui, container.fmt, container.clock.instant(),
    ItemActions(
      onBack = { nav.pop() }, onMore = { sheet { HubSheet(null, { sheet(null) }) { SettingsRow(G.Copy, "Copy the event id", Modifier.clickableRow { ask?.receipts?.eventId?.let { clipboard.copy("event id", it) }; sheet(null) }) } } },
      onAnswer = vm::answer, onApprove = { note -> gated("Approve ${ask?.agent.orEmpty()}") { vm.decide(true, note) } }, onDecline = { note -> gated("Decline ${ask?.agent.orEmpty()}") { vm.decide(false, note) } },
      onAdmit = { ok, name, grants -> if (ok) gated("Approve ${ask?.agent.orEmpty()}") { vm.admit(true, name, grants) } else vm.admit(false, name, grants) },
      onSnooze = vm::snooze, onDiscard = { vm.discardQueued(); nav.pop() }, onEdit = { vm.discardQueued() },
      onNext = { nav.pop() }, onOpenSession = { nav.push(Route.Session(it)) }, onOpenArtifact = { nav.push(Route.Viewer("ar1")) }, onCopy = { clipboard.copy("copied", it) },
      onResolve = vm::resolveIdea, onClear = vm::clearIdea, onNote = vm::replyIdea,
      onOpenBrain = { session, key -> nav.push(Route.Brain(session, dev.agync.agenthub.model.BrainPath.of(key))) },
    ),
    wide = wide, noteVersion = noteVersion, notice = notice,
  )
}

@Composable
private fun TabletNow(container: AppContainer, repo: HubRepository, nowVm: NowViewModel, nowActions: NowActions, nav: Navigator, platform: Platform, gated: (String, () -> Unit) -> Unit) {
  val ui by nowVm.ui.collectAsState()
  var selected by remember { mutableStateOf<String?>(null) }
  val first = ui.model.needsYou.firstOrNull()?.id ?: ui.model.canWait.firstOrNull()?.id
  val shown = selected ?: first
  Row(Modifier.fillMaxSize()) {
    Column(Modifier.width(420.dp).fillMaxHeight().hairlineRight(Hub.colors.line)) {
      TabletListHeader(ui.hubName, nowActions.onSwitchHub, nowActions.onSearch)
      NowScreen(ui, container.fmt, nowActions.copy(onOpenAsk = { selected = it.id }), selectedAskId = shown, bar = false, modifier = Modifier.weight(1f))
    }
    Box(Modifier.weight(1f).fillMaxHeight()) {
      if (shown != null) ItemRoute(shown, container, repo, nav, platform, gated, {}, wide = true)
      else Text("Nothing waits on you.", style = Hub.type.body.copy(color = Hub.colors.ink2), modifier = Modifier.padding(32.dp))
    }
  }
}

@Composable
private fun PhonesScreen(phones: List<dev.agync.agenthub.model.PhoneDevice>, onBack: () -> Unit, onOpenWeb: () -> Unit) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize().background(c.bg)) {
    dev.agync.agenthub.ui.theme.SmallTopBar("Phones", onBack = onBack)
    phones.forEach { p -> SettingsRow(G.Phone, p.name, detail = androidx.compose.ui.text.AnnotatedString(if (p.thisPhone) "This phone" else "Paired")) }
    Text("Phones are added and removed in the hub's web app.", style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(16.dp))
  }
}
