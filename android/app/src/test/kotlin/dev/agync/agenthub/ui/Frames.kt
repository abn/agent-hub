package dev.agync.agenthub.ui

import androidx.compose.runtime.Composable
import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.Refusal
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.pairing.Handshake
import dev.agync.agenthub.pairing.PairOffer
import dev.agync.agenthub.ui.screens.ArtifactsScreen
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
import dev.agync.agenthub.ui.screens.SessionScreen
import dev.agync.agenthub.ui.screens.SettingsActions
import dev.agync.agenthub.ui.screens.SearchScreen
import dev.agync.agenthub.ui.screens.SettingsScreen
import dev.agync.agenthub.ui.screens.StorageScreen
import dev.agync.agenthub.ui.screens.ViewerScreen
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.viewmodel.ArtifactsViewModel
import dev.agync.agenthub.viewmodel.HubViewModel
import dev.agync.agenthub.viewmodel.ItemViewModel
import dev.agync.agenthub.viewmodel.MemberViewModel
import dev.agync.agenthub.viewmodel.NowViewModel
import dev.agync.agenthub.viewmodel.PairStep
import dev.agync.agenthub.viewmodel.ProjectViewModel
import dev.agync.agenthub.viewmodel.SearchViewModel
import dev.agync.agenthub.viewmodel.ProjectsViewModel
import dev.agync.agenthub.viewmodel.SessionViewModel
import dev.agync.agenthub.viewmodel.ViewerViewModel
import kotlinx.coroutines.runBlocking

/** One board frame: how to draw it and the device it is drawn on. */
class Frame(val name: String, val time: String = "14:32", val fontScale: Float = 1f, val tablet: Boolean = false, val nav: Int? = null, val badge: Int = 3, val content: @Composable () -> Unit)

private val noPair = PairActions({}, {}, {}, { _, _ -> }, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})
private val noNow = NowActions({}, { _, _ -> }, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})
private val noItem = ItemActions({}, {}, {}, {}, {}, { _, _, _ -> }, {}, {}, {}, {}, {}, {}, {})

object Frames {
  val names =
    listOf(
      "pair-welcome", "pair-scan", "pair-confirm", "pair-sas", "pair-notify", "pair-manual", "pair-code-failed", "pair-camera-denied", "pair-notify-denied",
      "pair-too-old", "home", "home-live", "home-empty", "home-offline", "unreachable", "unpaired", "home-200",
      "item-question", "item-approval", "item-enrol", "item-queued", "item-refused", "item-resolved",
      "projects", "project-feed", "project-sessions", "session", "artifacts", "artifact-viewer", "artifact-locked",
      "hub", "member", "settings", "storage-prune", "tablet", "nav-200", "search",
    )

  private fun rigLoaded(): Fixtures.Rig = Fixtures.rig().also { runBlocking { it.repo.refresh() } }

  private fun nowVm(rig: Fixtures.Rig) = NowViewModel(rig.repo, Fixtures.fmt(), rig.clock)

  private fun itemVm(rig: Fixtures.Rig, id: String) = ItemViewModel(rig.repo, id, Fixtures.fmt(), rig.clock)

  private val fmt: Fmt = Fixtures.fmt()
  private val offer = PairOffer("tendril://pair?v=1", "home hub", "TND-4F9K-22XQ-8N3P", Fixtures.now.plusSeconds(15 * 60))

  fun build(name: String): Frame {
    val rig = rigLoaded()
    val now = Fixtures.now
    rig.repo.snooze("DS5REF")
    return when (name) {
      "pair-welcome" -> Frame(name) { PairScreen(PairStep.Welcome, DevicePrefs(), fmt, noPair) {} }
      "pair-scan" -> Frame(name) { PairScreen(PairStep.Scan(), DevicePrefs(), fmt, noPair, scanner = {}) }
      "pair-confirm" -> Frame(name) { PairScreen(PairStep.Confirm(offer, "pixel-phone"), DevicePrefs(), fmt, noPair) {} }
      "pair-sas" -> Frame(name) { PairScreen(PairStep.CheckString(Handshake("h", "home hub", "k7m2 qwxa 4fbn r3td", "LAN")), DevicePrefs(), fmt, noPair) {} }
      "pair-notify" -> Frame(name) { PairScreen(PairStep.Alerts("home hub"), DevicePrefs(alertFinished = false), fmt, noPair) {} }
      "pair-manual" -> Frame(name) { PairScreen(PairStep.Manual("TND-4F9K-22XQ-8N3P", "192.168.1.10:48732"), DevicePrefs(), fmt, noPair) {} }
      "pair-code-failed" -> Frame(name) { PairScreen(PairStep.CodeFailed(), DevicePrefs(), fmt, noPair) {} }
      "pair-camera-denied" -> Frame(name) { PairScreen(PairStep.Scan(cameraDenied = true), DevicePrefs(), fmt, noPair) {} }
      "pair-notify-denied" -> Frame(name) { PairScreen(PairStep.Alerts("home hub", notificationsDenied = true), DevicePrefs(), fmt, noPair) {} }
      "pair-too-old" -> {
        rig.seeded.version = "1.11.0"
        runBlocking { rig.repo.refresh() }
        Frame(name) { NowScreen(nowVm(rig).ui.value, fmt, noNow) }
      }
      "home" -> Frame(name, nav = 0, badge = 3) { NowScreen(nowVm(rig).ui.value, fmt, noNow) }
      "home-live" -> Frame(name, nav = 0, badge = 3) { NowScreen(nowVm(rig).ui.value, fmt, noNow, initialScrollIndex = 6) }
      "home-empty" -> {
        val state = rig.repo.state.value.snapshot!!.asks
        runBlocking {
          for (a in state) when (a.kind) {
            dev.agync.agenthub.model.AskKind.Approval -> rig.repo.decide(a, true, null)
            dev.agync.agenthub.model.AskKind.Question -> rig.repo.answer(a, a.answers.first())
            dev.agync.agenthub.model.AskKind.Joining -> rig.repo.admit(a, true, a.agent, emptySet())
            dev.agync.agenthub.model.AskKind.Idea -> rig.repo.resolveIdea(a)
          }
        }
        Frame(name, time = "16:05", nav = 0, badge = 0) { NowScreen(nowVm(rig).ui.value, fmt, noNow) }
      }
      "home-offline" -> {
        rig.seeded.reachable = false
        runBlocking {
          rig.repo.refresh()
          rig.repo.answer(rig.repo.state.value.snapshot!!.asks.first { it.id == "9MX4TB" }, "Yes, tonight")
        }
        Frame(name, time = "14:41", nav = 0, badge = 2) { NowScreen(nowVm(rig).ui.value, fmt, noNow) }
      }
      "unreachable" -> {
        val cold = Fixtures.rig().also { it.seeded.reachable = false }
        runBlocking { cold.repo.refresh() }
        Frame(name) { NowScreen(nowVm(cold).ui.value, fmt, noNow) }
      }
      "unpaired" -> {
        rig.seeded.reachable = false
        runBlocking { rig.repo.answer(rig.repo.state.value.snapshot!!.asks.first { it.id == "9MX4TB" }, "Yes, tonight. Start after 22:00 so the backups finish first.") }
        rig.seeded.reachable = true
        rig.seeded.revoked = true
        runBlocking { rig.repo.refresh() }
        Frame(name) { NowScreen(nowVm(rig).ui.value, fmt, noNow) }
      }
      "search" -> {
        val vm = SearchViewModel(rig.repo)
        vm.setQuery("backup")
        vm.submit()
        Frame(name) { SearchScreen(vm, fmt, now, {}, {}) }
      }
      "nav-200" -> Frame(name, nav = 0, badge = 5, fontScale = 2f) { }
      "home-200" -> Frame(name, nav = 0, badge = 3, fontScale = 2f) { NowScreen(nowVm(rig).ui.value, fmt, noNow) }
      "item-question" -> Frame(name) { ItemScreen(itemVm(rig, "9MX4TB").ui.value, fmt, now, noItem) }
      "item-approval" -> Frame(name) { ItemScreen(itemVm(rig, "7HQ2KD").ui.value, fmt, now, noItem) }
      "item-enrol" -> Frame(name) { ItemScreen(itemVm(rig, "JN3STU").ui.value, fmt, now, noItem) }
      "item-queued" -> {
        rig.seeded.reachable = false
        runBlocking { rig.repo.answer(rig.repo.state.value.snapshot!!.asks.first { it.id == "9MX4TB" }, "Yes, tonight. Start after 22:00 so the backups finish first.") }
        Frame(name, time = "14:41") { ItemScreen(itemVm(rig, "9MX4TB").ui.value, fmt, now, noItem) }
      }
      "item-refused" -> {
        rig.seeded.reachable = false
        runBlocking { rig.repo.answer(rig.repo.state.value.snapshot!!.asks.first { it.id == "9MX4TB" }, "Yes, tonight. Start after 22:00 so the backups finish first.") }
        rig.seeded.reachable = true
        rig.seeded.refuseNext = Refusal.AnsweredElsewhere(now.plusSeconds(4 * 60), "Wait for the weekend")
        runBlocking { rig.repo.refresh() }
        Frame(name, time = "14:52") { ItemScreen(itemVm(rig, "9MX4TB").ui.value, fmt, now, noItem) }
      }
      "item-resolved" -> {
        runBlocking { rig.seeded.decide("7HQ2KD", true, null); rig.repo.refresh() }
        Frame(name, time = "14:44") { ItemScreen(itemVm(rig, "7HQ2KD").ui.value, fmt, now, noItem) }
      }
      "projects" -> Frame(name, nav = 1, badge = 3) {
        val vm = ProjectsViewModel(rig.repo)
        val projects = vm.projects.state.value
        ProjectsScreen(projects, rig.repo.state.value, vm.waitingByProject(rig.repo.state.value, now), 3, fmt, now, "home hub", {}, {}, {}, {}, {})
      }
      "project-feed" -> Frame(name, nav = 1, badge = 3) { ProjectScreen(ProjectViewModel(rig.repo, "home-lab"), rig.repo.state.value, fmt, now, {}, {}, {}, {}, {}, {}) }
      "project-sessions" -> Frame(name, nav = 1, badge = 3) { ProjectScreen(ProjectViewModel(rig.repo, "billing-svc"), rig.repo.state.value, fmt, now, {}, {}, {}, {}, {}, {}, initialTab = 1) }
      "session" -> Frame(name, nav = 1, badge = 3) { SessionScreen(SessionViewModel(rig.repo, "01JDQ7RT"), fmt, now, {}, {}, {}, {}, {}) }
      "artifacts" -> Frame(name, nav = 2, badge = 3) { ArtifactsScreen(ArtifactsViewModel(rig.repo), fmt, now, "home hub", {}, {}, {}, {}, {}) }
      "artifact-viewer" -> Frame(name) { ViewerScreen(ViewerViewModel(rig.repo, "ar1"), fmt, now, FakePlatform().protectedGate, "seeded-hub", {}, {}) }
      "artifact-locked" -> Frame(name) { ViewerScreen(ViewerViewModel(rig.repo, "ar4"), fmt, now, FakePlatform().protectedGate, "seeded-hub", {}, {}) }
      "hub" -> Frame(name, nav = 3, badge = 3) {
        HubScreen(HubViewModel(rig.repo), fmt, now, "home hub", "1.13.0", "LAN", {}, {}, {}, {}, {}, {}, null, null, "pixel-phone, this one · tablet", {})
      }
      "member" -> Frame(name, nav = 3, badge = 3) { MemberScreen(MemberViewModel(rig.repo, "laptop"), fmt, now, {}, {}, {}, {}, {}) }
      "settings" -> Frame(name) {
        SettingsScreen(
          listOf(Fixtures.hub, dev.agync.agenthub.data.PairedHub("work-lab", "work-lab", "pixel-phone")), Fixtures.hub.id, "LAN", mapOf("work-lab" to 1), DevicePrefs(), listOf("space-a"),
          SettingsActions({}, {}, {}, {}, {}, {}, {}, {}, {}),
        )
      }
      "storage-prune" -> Frame(name) { StorageScreen(HubViewModel(rig.repo), fmt, {}, showPrune = true) }
      "tablet" -> Frame(name, tablet = true) { TabletPreview(rig, fmt) }
      else -> error("no frame $name")
    }
  }
}
