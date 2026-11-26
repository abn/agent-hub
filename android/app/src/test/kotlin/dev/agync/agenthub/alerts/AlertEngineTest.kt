package dev.agync.agenthub.alerts

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.FeedEvent
import java.time.Duration
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.yield
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

private class RecordingPoster : AlertPoster {
  val posted = mutableListOf<String>()
  val cancelled = mutableListOf<String>()
  val finished = mutableListOf<String>()
  var connection: String? = null

  override fun post(hubId: String, hubName: String, ask: Ask) { posted += ask.id }

  override fun cancel(hubId: String, askId: String) { cancelled += askId }

  override fun postFinished(hubId: String, hubName: String, event: FeedEvent) { finished += event.id }

  override fun connection(hubName: String, status: String) { connection = status }
}

class AlertEngineTest {
  private fun engine(rig: Fixtures.Rig, poster: RecordingPoster, prefs: DevicePrefs = DevicePrefs()) = AlertEngine(rig.repo, poster, { prefs }, rig.clock)

  @Test fun `the first tick posts every waiting ask once`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    e.onTick()
    assertEquals(setOf("7HQ2KD", "9MX4TB", "JN3STU", "DS5REF"), poster.posted.toSet())
    e.onTick()
    assertEquals(4, poster.posted.size)
  }

  @Test fun `a tick after an ask is answered elsewhere cancels its notification`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    e.onTick()
    rig.seeded.answer("9MX4TB", "Wait for the weekend")
    e.onTick()
    assertEquals(listOf("9MX4TB"), poster.cancelled)
  }

  @Test fun `a new ask posts on the next tick and nothing else repeats`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    e.onTick()
    poster.posted.clear()
    e.onTick()
    assertTrue(poster.posted.isEmpty())
  }

  @Test fun `a snoozed ask leaves the shade and comes back when the snooze ends`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    e.onTick()
    rig.repo.snooze("7HQ2KD")
    e.apply()
    assertEquals(listOf("7HQ2KD"), poster.cancelled)
    poster.posted.clear()
    rig.clock.at = Fixtures.now.plus(Duration.ofMinutes(61))
    e.onTick()
    assertEquals(listOf("7HQ2KD"), poster.posted)
  }

  @Test fun `an answer given on this phone cancels its notification at once`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    e.onTick()
    val ask = rig.repo.state.value.snapshot!!.asks.first { it.id == "9MX4TB" }
    rig.repo.answer(ask, "Yes, tonight")
    e.apply()
    assertEquals(listOf("9MX4TB"), poster.cancelled)
  }

  @Test fun `an unreachable hub changes nothing in the shade and says so on the connection line`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    e.onTick()
    rig.seeded.reachable = false
    e.onTick()
    assertTrue(poster.cancelled.isEmpty())
    assertEquals("Cannot reach the hub. Trying again.", poster.connection)
  }

  @Test fun `the connection line says how the hub was reached`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    engine(rig, poster).onTick()
    assertEquals("via LAN", poster.connection)
  }

  @Test fun `kinds the person turned off stay out of the shade`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    engine(rig, poster, DevicePrefs(alertApprovals = false, alertJoining = false)).onTick()
    assertTrue(poster.posted.isEmpty())
  }

  @Test fun `finished work already on the hub is taken as known on the first fetch`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster, DevicePrefs(alertFinished = true))
    e.onTick()
    assertTrue(poster.finished.isEmpty())
  }

  @Test fun `ticks from the hub drive refetches`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    val job = launch(kotlinx.coroutines.Dispatchers.Unconfined) { e.run() }
    yield()
    rig.seeded.emitTick()
    yield()
    job.cancel()
    assertEquals(4, poster.posted.size)
  }

  @Test fun `the plan lists posts and cancels without touching the shade`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val plan = AlertPlanner.plan(setOf("gone"), rig.repo.state.value, Fixtures.now, DevicePrefs(), null)
    assertEquals(setOf("gone"), plan.cancel)
    assertEquals(4, plan.post.size)
  }

  @Test fun `an idea the person captured never raises an alert`() = runTest {
    val rig = Fixtures.rig()
    rig.seeded.captureIdea("home-lab", "Try Longhorn", null)
    val poster = RecordingPoster()
    engine(rig, poster).onTick()
    assertEquals(setOf("7HQ2KD", "9MX4TB", "JN3STU", "DS5REF"), poster.posted.toSet())
  }
}
