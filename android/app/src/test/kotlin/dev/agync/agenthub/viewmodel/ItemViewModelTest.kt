package dev.agync.agenthub.viewmodel

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import dev.agync.agenthub.data.Refusal
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class ItemViewModelTest {
  @get:Rule val main = MainDispatcherRule()

  private fun vm(rig: Fixtures.Rig, id: String) = ItemViewModel(rig.repo, id, Fixtures.fmt(), rig.clock)

  private fun collect(v: ItemViewModel, into: MutableList<ItemEvent>) = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.Unconfined).launch { v.events.collect { into += it } }

  @Test fun `an approval is open with its position and the next ask`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.repo.snooze("DS5REF")
    val ui = vm(rig, "7HQ2KD").ui.value as ItemUi.Open
    assertEquals(3, ui.total)
    assertTrue(ui.position in 1..3)
    assertTrue(ui.next != null)
  }

  @Test fun `approving sends the decision and opens the next ask`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val v = vm(rig, "7HQ2KD")
    val events = mutableListOf<ItemEvent>()
    val job = collect(v, events)
    v.decide(true, "go ahead")
    job.cancel()
    val done = events.single() as ItemEvent.Done
    assertEquals(false, done.queued)
    assertTrue(done.nextAskId != null)
    assertTrue(v.ui.value is ItemUi.Resolved)
  }

  @Test fun `an answer typed offline waits and the screen shows the queued state`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.reachable = false
    val v = vm(rig, "9MX4TB")
    v.answer("Yes, tonight. Start after 22:00.")
    val q = v.ui.value as ItemUi.Queued
    assertEquals("Yes, tonight. Start after 22:00.", q.queued.text)
  }

  @Test fun `a refusal turns the screen into the refused state and keeps the text to copy`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.refuseNext = Refusal.AnsweredElsewhere(Fixtures.now, "Wait for the weekend")
    val v = vm(rig, "9MX4TB")
    v.answer("Yes, tonight")
    val r = v.ui.value as ItemUi.Refused
    assertEquals("Yes, tonight", r.queued.text)
    assertEquals(Refusal.AnsweredElsewhere(Fixtures.now, "Wait for the weekend"), r.refusal)
    v.discardQueued()
    assertTrue(v.ui.value is ItemUi.Open || v.ui.value is ItemUi.Resolved)
  }

  @Test fun `an ask decided elsewhere opens as resolved`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.decide("7HQ2KD", true, null)
    rig.repo.refresh()
    val ui = vm(rig, "7HQ2KD").ui.value as ItemUi.Resolved
    assertEquals(3, ui.stillNeedYou)
  }

  @Test fun `an unknown ask is missing`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    assertEquals(ItemUi.Missing, vm(rig, "nope").ui.value)
  }

  @Test fun `snoozing says until when and hides the verb`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val v = vm(rig, "9MX4TB")
    val events = mutableListOf<ItemEvent>()
    val job = collect(v, events)
    v.snooze()
    job.cancel()
    assertEquals(Fixtures.now.plusSeconds(3600), (events.single() as ItemEvent.Snoozed).until)
    assertEquals(Fixtures.now.plusSeconds(3600), (v.ui.value as ItemUi.Open).snoozedUntil)
  }

  @Test fun `a blank reply is not sent`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val v = vm(rig, "9MX4TB")
    v.answer("   ")
    assertTrue(v.ui.value is ItemUi.Open)
    assertNull(rig.repo.queuedFor("9MX4TB"))
  }

  @Test fun `approving a joining ask can grant the confidential project it asked for`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val v = vm(rig, "JN3STU")
    v.admit(true, "studio-mac", setOf("space-a"))
    assertEquals("studio-mac", rig.seeded.members().first { it.name == "studio-mac" && !it.pending }.name)
    assertEquals("space-a", rig.seeded.members().first { it.name == "studio-mac" && !it.pending }.grants.single().project)
  }

  private suspend fun idea(rig: Fixtures.Rig): String {
    rig.seeded.captureIdea("home-lab", "Try Longhorn", null)
    rig.repo.refresh()
    return rig.repo.state.value.snapshot!!.asks.first { it.kind == dev.agync.agenthub.model.AskKind.Idea }.id
  }

  @Test fun `resolving an idea closes it and leaves the screen`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig, idea(rig))
    val events = mutableListOf<ItemEvent>()
    val job = collect(v, events)
    v.resolveIdea()
    job.cancel()
    assertTrue(events.single() is ItemEvent.Done)
    assertEquals(dev.agync.agenthub.model.AskStatus.Resolved, (v.ui.value as ItemUi.Resolved).ask.status)
  }

  @Test fun `a note on an idea keeps it open and says it was noted`() = runTest {
    val rig = Fixtures.rig()
    val v = vm(rig, idea(rig))
    val events = mutableListOf<ItemEvent>()
    val job = collect(v, events)
    v.replyIdea("  Check the disks first ")
    job.cancel()
    assertEquals(listOf<ItemEvent>(ItemEvent.Noted), events)
    assertTrue(v.ui.value is ItemUi.Open)
    assertEquals("Check the disks first", rig.seeded.ideaNotes.single().second)
  }

  @Test fun `a refused approval tells the person to use the web app`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.refuseAdmitFromPhone = true
    val v = vm(rig, "JN3STU")
    val events = mutableListOf<ItemEvent>()
    val job = collect(v, events)
    v.admit(true, "studio-mac", emptySet())
    job.cancel()
    val failed = events.single() as ItemEvent.Failed
    assertTrue(failed.message, failed.message.contains("web app"))
    assertTrue(v.ui.value is ItemUi.Open)
  }
}
