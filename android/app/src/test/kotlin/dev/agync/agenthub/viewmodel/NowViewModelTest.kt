package dev.agync.agenthub.viewmodel

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import dev.agync.agenthub.model.NowFilter
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class NowViewModelTest {
  @get:Rule val main = MainDispatcherRule()

  private fun vm(rig: Fixtures.Rig) = NowViewModel(rig.repo, Fixtures.fmt(), rig.clock)

  /** The screen is built off the main thread and only for a collector, so a test waits for the state it expects. */
  private suspend fun NowViewModel.until(p: (NowUi) -> Boolean): NowUi = ui.first(p)

  /** A screen on view: something collects the state, as the app does while it is started. */
  private fun TestScope.watch(v: NowViewModel) {
    backgroundScope.launch(kotlinx.coroutines.Dispatchers.Unconfined) { v.ui.collect {} }
  }

  @Test fun `before the first fetch the screen is loading`() {
    val rig = Fixtures.rig()
    assertEquals(NowScreenKind.Loading, vm(rig).ui.value.kind)
  }

  @Test fun `nothing is built while no screen collects`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    rig.repo.refresh()
    assertEquals("the screen was built with nobody looking", NowScreenKind.Loading, v.ui.value.kind)
    watch(v)
    assertEquals(NowScreenKind.Content, v.until { it.kind == NowScreenKind.Content }.kind)
  }

  @Test fun `after a fetch Now shows the meta line the board shows`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.repo.refresh()
    val ui = v.until { it.kind == NowScreenKind.Content }
    assertEquals(NowScreenKind.Content, ui.kind)
    assertEquals("Tue 24 Nov · 2 live · synced 14:32", ui.meta)
    assertNull(ui.offlineBanner)
  }

  @Test fun `offline with a cache shows the banner with how the hub was last reached`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.repo.refresh()
    rig.seeded.reachable = false
    rig.repo.refresh()
    val ui = v.until { it.offlineBanner != null }
    assertEquals("Cannot reach home hub. Last connected via LAN at 14:32. Answers you give now wait on this phone.", ui.offlineBanner)
    assertEquals("Waiting to send", ui.sinceLabel)
    assertEquals("Tue 24 Nov · as of 14:32", ui.meta)
  }

  @Test fun `offline with no cache is the unreachable screen`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.seeded.reachable = false
    rig.repo.refresh()
    val k = v.until { it.kind is NowScreenKind.Unreachable }.kind as NowScreenKind.Unreachable
    assertEquals("14:32", k.attemptedAt)
    assertNull(k.lastConnected)
  }

  @Test fun `a revoked phone gets its own screen, not the unreachable one`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.repo.refresh()
    rig.seeded.revoked = true
    rig.repo.refresh()
    assertEquals(NowScreenKind.Unpaired("pixel-phone"), v.until { it.kind is NowScreenKind.Unpaired }.kind)
  }

  @Test fun `an old hub gets the update screen with how it is connected`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.seeded.version = "1.11.0"
    rig.repo.refresh()
    assertEquals(NowScreenKind.TooOld("1.11.0", "1.13.0", null), v.until { it.kind is NowScreenKind.TooOld }.kind)
  }

  @Test fun `snoozing an ask moves it to can wait and says so`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.repo.refresh()
    val events = mutableListOf<NowEvent>()
    val job = backgroundScope.launch(kotlinx.coroutines.Dispatchers.Unconfined) { v.events.collect { events += it } }
    val ask = v.until { u -> u.model.needsYou.any { it.id == "9MX4TB" } }.model.needsYou.first { it.id == "9MX4TB" }
    v.snooze(ask)
    job.cancel()
    assertTrue(v.until { u -> u.model.canWait.any { it.id == "9MX4TB" } }.model.canWait.any { it.id == "9MX4TB" })
    assertEquals(NowEvent.Snoozed("9MX4TB", "15:32"), events.single())
    v.undoSnooze("9MX4TB")
    assertTrue(v.until { u -> u.model.needsYou.any { it.id == "9MX4TB" } }.model.needsYou.any { it.id == "9MX4TB" })
  }

  @Test fun `answering a question sends it and clears it from needs you`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.repo.refresh()
    v.answer(v.until { u -> u.model.needsYou.any { it.id == "9MX4TB" } }.model.needsYou.first { it.id == "9MX4TB" }, "Yes, tonight")
    assertTrue(v.until { u -> u.model.needsYou.none { it.id == "9MX4TB" } }.model.needsYou.none { it.id == "9MX4TB" })
  }

  @Test fun `answering offline queues and tells the person`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.repo.refresh()
    rig.seeded.reachable = false
    val events = mutableListOf<NowEvent>()
    val job = backgroundScope.launch(kotlinx.coroutines.Dispatchers.Unconfined) { v.events.collect { events += it } }
    v.answer(v.until { u -> u.model.needsYou.any { it.id == "9MX4TB" } }.model.needsYou.first { it.id == "9MX4TB" }, "Yes, tonight")
    job.cancel()
    assertTrue(events.single() is NowEvent.Queued)
    assertEquals(1, v.until { it.unsent.isNotEmpty() }.unsent.size)
  }

  @Test fun `the filter narrows the list`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig)
    watch(v)
    rig.repo.refresh()
    v.setFilter(NowFilter.NeedsYou)
    val ui = v.until { it.filter == NowFilter.NeedsYou && it.kind == NowScreenKind.Content }
    assertEquals(NowFilter.NeedsYou, ui.filter)
    assertNotNull(ui.model.needsYou.firstOrNull())
  }
}
