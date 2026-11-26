package dev.agync.agenthub.alerts

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.model.AskStatus
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationActionsTest {
  private fun actions(rig: Fixtures.Rig, prefs: DevicePrefs = DevicePrefs()) =
    NotificationActions({ id -> if (id == Fixtures.hub.id) rig.repo else null }, { prefs })

  private fun status(rig: Fixtures.Rig, id: String) = runBlocking { rig.seeded.ask(id) }.status

  @Test fun `approve decides a waiting approval`() = runTest {
    val rig = Fixtures.rig()
    assertEquals(ActionOutcome.Done, actions(rig).act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_APPROVE, AlertIntents.TYPED, null))
    assertEquals(AskStatus.Approved, status(rig, "7HQ2KD"))
  }

  @Test fun `decline declines and does not approve`() = runTest {
    val rig = Fixtures.rig()
    actions(rig).act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_DECLINE, AlertIntents.TYPED, null)
    assertEquals(AskStatus.Declined, status(rig, "7HQ2KD"))
  }

  @Test fun `with the fingerprint opt-in on, approve and decline open the item and decide nothing`() = runTest {
    val rig = Fixtures.rig()
    val a = actions(rig, DevicePrefs(fingerprintBeforeApproving = true))
    assertEquals(ActionOutcome.OpenItem, a.act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_APPROVE, AlertIntents.TYPED, null))
    assertEquals(ActionOutcome.OpenItem, a.act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_DECLINE, AlertIntents.TYPED, null))
    assertEquals(AskStatus.Waiting, status(rig, "7HQ2KD"))
  }

  @Test fun `an answer is taken by index from the live ask, not from text on the button`() = runTest {
    val rig = Fixtures.rig()
    actions(rig).act("seeded-hub", "9MX4TB", AlertIntents.ACTION_ANSWER, 1, null)
    assertEquals("answered nightly: Wait for the weekend", runBlocking { rig.seeded.ask("9MX4TB") }.decidedWith)
  }

  @Test fun `an option index the ask does not have opens the item`() = runTest {
    val rig = Fixtures.rig()
    assertEquals(ActionOutcome.OpenItem, actions(rig).act("seeded-hub", "9MX4TB", AlertIntents.ACTION_ANSWER, 7, null))
    assertEquals(AskStatus.Waiting, status(rig, "9MX4TB"))
  }

  @Test fun `an approval button on a question, or an answer button on an approval, does nothing`() = runTest {
    val rig = Fixtures.rig()
    val a = actions(rig)
    assertEquals(ActionOutcome.OpenItem, a.act("seeded-hub", "9MX4TB", AlertIntents.ACTION_APPROVE, AlertIntents.TYPED, null))
    assertEquals(ActionOutcome.OpenItem, a.act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_ANSWER, 0, null))
    assertEquals(AskStatus.Waiting, status(rig, "9MX4TB"))
    assertEquals(AskStatus.Waiting, status(rig, "7HQ2KD"))
  }

  @Test fun `a typed reply is sent as typed, and a blank one is not`() = runTest {
    val rig = Fixtures.rig()
    val a = actions(rig)
    assertEquals(ActionOutcome.OpenItem, a.act("seeded-hub", "9MX4TB", AlertIntents.ACTION_ANSWER, AlertIntents.TYPED, "  "))
    a.act("seeded-hub", "9MX4TB", AlertIntents.ACTION_ANSWER, AlertIntents.TYPED, "Tonight, after 22:00")
    assertEquals("answered nightly: Tonight, after 22:00", runBlocking { rig.seeded.ask("9MX4TB") }.decidedWith)
  }

  @Test fun `an ask already decided is not decided again`() = runTest {
    val rig = Fixtures.rig()
    rig.seeded.decide("7HQ2KD", true, null)
    assertEquals(ActionOutcome.OpenItem, actions(rig).act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_DECLINE, AlertIntents.TYPED, null))
    assertEquals(AskStatus.Approved, status(rig, "7HQ2KD"))
  }

  @Test fun `another hub's button does not reach this hub's ask`() = runTest {
    val rig = Fixtures.rig()
    assertEquals(ActionOutcome.OpenItem, actions(rig).act("other-hub", "7HQ2KD", AlertIntents.ACTION_APPROVE, AlertIntents.TYPED, null))
    assertEquals(AskStatus.Waiting, status(rig, "7HQ2KD"))
  }

  @Test fun `snooze is per phone and never decides`() = runTest {
    val rig = Fixtures.rig()
    assertEquals(ActionOutcome.Done, actions(rig).act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_SNOOZE, AlertIntents.TYPED, null))
    assertTrue(rig.repo.state.value.snoozes.containsKey("7HQ2KD"))
    assertEquals(AskStatus.Waiting, status(rig, "7HQ2KD"))
  }

  @Test fun `when the hub cannot be asked in time, the verb waits in the outbox instead of deciding on a stale snapshot`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.hangNow = true
    assertEquals(ActionOutcome.Done, actions(rig).act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_APPROVE, AlertIntents.TYPED, null))
    assertEquals(AskStatus.Waiting, status(rig, "7HQ2KD"))
    assertEquals(dev.agync.agenthub.data.DecisionKind.Approve, rig.repo.state.value.outbox.single().kind)
  }

  @Test fun `a send that does not finish in its share of the time is queued, and a later refresh sends it`() = runTest {
    val rig = Fixtures.rig()
    rig.seeded.hangDecide = true
    assertEquals(ActionOutcome.Done, actions(rig).act("seeded-hub", "7HQ2KD", AlertIntents.ACTION_APPROVE, AlertIntents.TYPED, null))
    assertEquals(AskStatus.Waiting, status(rig, "7HQ2KD"))
    assertEquals(1, rig.repo.state.value.outbox.size)
    rig.seeded.hangDecide = false
    rig.repo.refresh()
    assertEquals(AskStatus.Approved, status(rig, "7HQ2KD"))
    assertTrue(rig.repo.state.value.outbox.isEmpty())
  }

  @Test fun `a typed reply with a stale check is queued with its text`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.hangNow = true
    actions(rig).act("seeded-hub", "9MX4TB", AlertIntents.ACTION_ANSWER, AlertIntents.TYPED, " Tonight ")
    assertEquals("Tonight", rig.repo.state.value.outbox.single().text)
  }
}
