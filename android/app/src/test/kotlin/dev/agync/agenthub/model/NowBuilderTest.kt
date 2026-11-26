package dev.agync.agenthub.model

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.Connection
import dev.agync.agenthub.data.DecisionKind
import dev.agync.agenthub.data.QueuedDecision
import java.time.Duration
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NowBuilderTest {
  private suspend fun loaded(): Fixtures.Rig = Fixtures.rig().also { it.repo.refresh() }

  private fun text(spans: List<Span>) = spans.joinToString("") { it.text }

  @Test fun `three asks need you and the snoozed one can wait`() = runTest {
    val rig = loaded()
    rig.repo.snooze("DS5REF")
    val m = NowBuilder.build(rig.repo.state.value, Fixtures.now, Fixtures.fmt())
    assertEquals(listOf("7HQ2KD", "9MX4TB", "JN3STU").sortedBy { it }, m.needsYou.map { it.id }.sortedBy { it })
    assertEquals(3, m.badge)
    assertEquals(listOf("DS5REF"), m.canWait.map { it.id })
  }

  @Test fun `the lede names two asks and counts the rest`() = runTest {
    val rig = loaded()
    rig.repo.snooze("DS5REF")
    val m = NowBuilder.build(rig.repo.state.value, Fixtures.now, Fixtures.fmt())
    assertEquals(
      "Three agents need you. feat-invoices wants a decision on “Run the migration on staging”, and nightly asks “Upgrade k3s to 1.31 tonight?”, and one more. One can wait.",
      text(m.lede),
    )
    assertTrue(m.lede.first { it.text == "feat-invoices" }.bold)
    assertTrue(m.lede.last().muted)
  }

  @Test fun `a snooze that has run out puts the ask back under needs you`() = runTest {
    val rig = loaded()
    rig.repo.snooze("DS5REF")
    val later = Fixtures.now.plus(Duration.ofMinutes(61))
    val m = NowBuilder.build(rig.repo.state.value, later, Fixtures.fmt())
    assertEquals(4, m.needsYou.size)
    assertTrue(m.canWait.isEmpty())
  }

  @Test fun `a queued answer leaves needs you and waits to send`() = runTest {
    val rig = loaded()
    rig.seeded.reachable = false
    val ask = rig.repo.state.value.snapshot!!.asks.first { it.id == "9MX4TB" }
    rig.repo.answer(ask, "Yes, tonight")
    val state = rig.repo.state.value
    val m = NowBuilder.build(state, Fixtures.now, Fixtures.fmt())
    assertTrue(state.connection is Connection.Offline)
    assertEquals(listOf("9MX4TB"), m.waitingToSend.map { it.queued.askId })
    assertTrue(m.needsYou.none { it.id == "9MX4TB" })
  }

  @Test fun `with nothing waiting the lede says so and names live work`() = runTest {
    val rig = loaded()
    for (a in rig.repo.state.value.snapshot!!.asks) {
      when (a.kind) {
        AskKind.Approval -> rig.repo.decide(a, true, null)
        AskKind.Question -> rig.repo.answer(a, a.answers.first())
        AskKind.Joining -> rig.repo.admit(a, true, a.agent, emptySet())
        AskKind.Idea -> rig.repo.resolveIdea(a)
      }
    }
    val m = NowBuilder.build(rig.repo.state.value, Fixtures.now, Fixtures.fmt())
    assertTrue(m.needsYou.isEmpty())
    assertEquals("Nothing waits on you. Two sessions are working, and the last ask was answered at 14:32.", text(m.lede))
  }

  @Test fun `a refused answer is not counted as waiting to send`() {
    val q = QueuedDecision("a", "nightly", "x", DecisionKind.Answer, "Yes", queuedAtEpochMilli = 0, refusal = dev.agync.agenthub.data.StoredRefusal("answered", 0))
    assertTrue(q.refused)
  }

  @Test fun `verbs follow the kind of ask`() = runTest {
    val asks = loaded().repo.state.value.snapshot!!.asks.associateBy { it.id }
    assertEquals(listOf("Approve", "Decline"), NowBuilder.verbs(asks.getValue("7HQ2KD")))
    assertEquals(listOf("Yes, tonight", "Wait for the weekend"), NowBuilder.verbs(asks.getValue("9MX4TB")))
    assertEquals(listOf("Review"), NowBuilder.verbs(asks.getValue("JN3STU")))
  }

  @Test fun `an idea waits under Needs you with Resolve and Clear`() = runTest {
    val rig = Fixtures.rig()
    rig.seeded.captureIdea("home-lab", "Try Longhorn", null)
    rig.repo.refresh()
    val m = NowBuilder.build(rig.repo.state.value, Fixtures.now, Fixtures.fmt())
    val idea = m.needsYou.single { it.kind == AskKind.Idea }
    assertEquals(listOf("Resolve", "Clear"), NowBuilder.verbs(idea))
    assertEquals("noted: Try Longhorn", AskText.rowTail(idea))
  }
}
