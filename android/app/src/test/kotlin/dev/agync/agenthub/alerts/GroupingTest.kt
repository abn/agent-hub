package dev.agync.agenthub.alerts

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.HubState
import dev.agync.agenthub.model.AskText
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class GroupingTest {
  private suspend fun stateWith(n: Int): HubState {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val s = rig.repo.state.value
    val first = s.snapshot!!.asks.first { it.waiting }
    val asks = (1..n).map { first.copy(id = "A$it", askedAt = first.askedAt.plusSeconds(it.toLong())) }
    return s.copy(snapshot = s.snapshot.copy(asks = asks))
  }

  private fun plan(state: HubState, notified: Set<String> = emptySet(), summarized: Int? = null) =
    AlertPlanner.plan(notified, state, Fixtures.now, DevicePrefs(), null, summarized)

  @Test fun `two asks are posted on their own with no summary`() = runTest {
    val p = plan(stateWith(2))
    assertEquals(2, p.post.size)
    assertNull(p.groupKey)
    assertNull(p.summary)
  }

  @Test fun `three asks still make no group`() = runTest {
    assertNull(plan(stateWith(3)).groupKey)
  }

  @Test fun `five asks are posted in one group with a summary`() = runTest {
    val p = plan(stateWith(5))
    assertEquals(5, p.post.size)
    assertEquals("dev.agync.agenthub.asks.seeded-hub", p.groupKey)
    assertEquals("dev.agync.agenthub.asks.seeded-hub", p.summary?.groupKey)
    assertEquals(5, p.summary?.count)
  }

  @Test fun `asks already showing are posted again when the group forms`() = runTest {
    val p = plan(stateWith(4), notified = setOf("A1", "A2", "A3"))
    assertEquals(listOf("A1", "A2", "A3", "A4"), p.post.map { it.id })
  }

  @Test fun `an ask that joins an existing group posts alone and the summary count moves`() = runTest {
    val p = plan(stateWith(5), notified = setOf("A1", "A2", "A3", "A4"), summarized = 4)
    assertEquals(listOf("A5"), p.post.map { it.id })
    assertEquals(5, p.summary?.count)
  }

  @Test fun `an unchanged group posts nothing`() = runTest {
    val p = plan(stateWith(5), notified = setOf("A1", "A2", "A3", "A4", "A5"), summarized = 5)
    assertNull(p.summary)
    assertEquals(emptyList<String>(), p.post.map { it.id })
  }

  @Test fun `once grouped the asks stay grouped down to the last one, and the summary goes with it`() = runTest {
    val five = plan(stateWith(5))
    assertEquals(5, five.summary?.count)
    val ids = (1..5).map { "A$it" }.toSet()
    val three = plan(stateWith(3), notified = ids, summarized = 5)
    assertEquals("dev.agync.agenthub.asks.seeded-hub", three.groupKey)
    assertEquals(3, three.summary?.count)
    assertEquals(false, three.dropSummary)
    assertEquals("the children stay as they are", emptyList<String>(), three.post.map { it.id })
    assertEquals(setOf("A4", "A5"), three.cancel)
    val one = plan(stateWith(1), notified = setOf("A1", "A2", "A3"), summarized = 3)
    assertEquals(1, one.summary?.count)
    assertEquals(false, one.dropSummary)
    val none = plan(stateWith(0), notified = setOf("A1"), summarized = 1)
    assertEquals(true, none.dropSummary)
    assertNull(none.groupKey)
    assertEquals(setOf("A1"), none.cancel)
  }

  @Test fun `the summary names the kind of ask it stands for, or the questions when they are mixed`() = runTest {
    val s = stateWith(5)
    assertEquals(setOf(dev.agync.agenthub.model.AskKind.Approval), plan(s).summary?.kinds)
    val asks = s.snapshot!!.asks
    val mixed = s.copy(snapshot = s.snapshot.copy(asks = asks.mapIndexed { i, a -> if (i == 0) a.copy(kind = dev.agync.agenthub.model.AskKind.Question) else a }))
    assertEquals(2, plan(mixed).summary?.kinds?.size)
  }

  @Test fun `the summary says how many asks wait`() {
    assertEquals("Five asks wait on you", AskText.summaryTitle(5))
    assertEquals("One ask waits on you", AskText.summaryTitle(1))
    assertEquals("12 asks wait on you", AskText.summaryTitle(12))
  }
}
