package dev.agync.agenthub.alerts

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.HubClient
import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.data.MemoryStore
import dev.agync.agenthub.data.NowSnapshot
import dev.agync.agenthub.data.SeededHub
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.FeedEvent
import java.time.Duration
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
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
  val connections = mutableListOf<String>()
  val groupKeys = mutableMapOf<String, String?>()
  val summaries = mutableListOf<AskSummary>()
  var summaryDropped = 0

  override fun post(hubId: String, hubName: String, ask: Ask, groupKey: String?) { posted += ask.id; groupKeys[ask.id] = groupKey }

  override fun summary(hubId: String, hubName: String, summary: AskSummary): Boolean {
    if (!permitted) return false
    summaries += summary
    return true
  }

  override fun cancelSummary(hubId: String) { summaryDropped++ }

  override fun cancel(hubId: String, askId: String) { cancelled += askId }

  override fun postFinished(hubId: String, hubName: String, event: FeedEvent) { finished += event.id }

  /** False while the notification permission is missing: nothing is posted and the engine is told so. */
  var permitted = true

  override fun connection(hubName: String, status: String): Boolean {
    if (!permitted) return false
    connection = status
    connections += status
    return true
  }
}

@OptIn(ExperimentalCoroutinesApi::class)
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
    // The group never dissolved, so only the ask that came back goes up, and the summary counts it.
    assertEquals(listOf("7HQ2KD"), poster.posted)
    assertEquals(4, poster.summaries.last().count)
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

  @Test fun `four asks go up as a group with one summary, and the next tick does not repeat it`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    e.onTick()
    assertEquals(listOf(4), poster.summaries.map { it.count })
    assertEquals(setOf("dev.agync.agenthub.asks.seeded-hub"), poster.groupKeys.values.toSet())
    e.onTick()
    assertEquals(4, poster.posted.size)
    assertEquals(listOf(4), poster.summaries.map { it.count })
  }

  @Test fun `a group stays a group when an ask is answered, and its summary moves`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    e.onTick()
    rig.seeded.answer("9MX4TB", "Wait for the weekend")
    poster.posted.clear()
    e.onTick()
    assertEquals(0, poster.summaryDropped)
    assertEquals(emptyList<String>(), poster.posted)
    assertEquals(3, poster.summaries.last().count)
    assertEquals(listOf("9MX4TB"), poster.cancelled)
  }

  @Test fun `a summary the system refused is posted once the permission arrives`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster().apply { permitted = false }
    val e = engine(rig, poster)
    e.onTick()
    assertEquals(emptyList<AskSummary>(), poster.summaries)
    poster.permitted = true
    e.apply()
    assertEquals(listOf(4), poster.summaries.map { it.count })
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

  @Test fun `identical applies post the connection notification once`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    rig.repo.refresh()
    repeat(10) { e.apply() }
    assertEquals(listOf("via LAN"), poster.connections)
  }

  @Test fun `a connection notification the system refused is posted once the permission arrives`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster().apply { permitted = false }
    val e = engine(rig, poster)
    rig.repo.refresh()
    repeat(3) { e.apply() }
    assertTrue(poster.connections.isEmpty())
    poster.permitted = true
    e.apply()
    e.apply()
    assertEquals(listOf("via LAN"), poster.connections)
  }

  @Test fun `the connection notification posts again when its words change`() = runTest {
    val rig = Fixtures.rig()
    val poster = RecordingPoster()
    val e = engine(rig, poster)
    rig.repo.refresh()
    e.apply()
    rig.seeded.reachable = false
    rig.repo.refresh()
    e.apply()
    e.apply()
    rig.seeded.reachable = true
    rig.repo.refresh()
    e.apply()
    assertEquals(listOf("via LAN", "Cannot reach the hub. Trying again.", "via LAN"), poster.connections)
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

  @Test fun `a tick makes a refetch after the window, and not before`() = runTest {
    val seeded = SeededHub(Fixtures.clock())
    var fetches = 0
    val counting = object : HubClient by seeded {
      override suspend fun now(): NowSnapshot { fetches++; return seeded.now() }
    }
    val repo = HubRepository(Fixtures.hub, counting, MemoryStore(), Fixtures.clock())
    val e = AlertEngine(repo, RecordingPoster(), { DevicePrefs() }, Fixtures.clock(), MutableStateFlow(true))
    backgroundScope.launch { e.run() }
    runCurrent()
    assertEquals("a refetch with no tick", 0, fetches)
    seeded.emitTick()
    runCurrent()
    assertEquals("the first tick after quiet makes no refetch at once", 1, fetches)
    seeded.emitTick()
    advanceTimeBy(TickWindow.FOREGROUND_MILLIS - 1)
    assertEquals("refetched inside the window", 1, fetches)
    advanceTimeBy(2)
    assertEquals("the second tick made no refetch", 2, fetches)
    advanceTimeBy(60_000)
    assertEquals("a refetch with no tick behind it", 2, fetches)
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
