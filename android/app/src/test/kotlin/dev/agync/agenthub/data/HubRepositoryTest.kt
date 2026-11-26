package dev.agync.agenthub.data

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.model.AskStatus
import java.time.Duration
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HubRepositoryTest {
  private suspend fun ask(rig: Fixtures.Rig, id: String) = rig.repo.state.value.snapshot!!.asks.first { it.id == id }

  @Test fun `refresh loads the snapshot and says how the hub was reached`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val s = rig.repo.state.value
    assertEquals(Connection.Online, s.connection)
    assertEquals("LAN", s.lastVia)
    assertEquals(4, s.snapshot!!.asks.size)
  }

  @Test fun `an unreachable hub keeps the cache and records when it was last reached`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.clock.at = Fixtures.now.plus(Duration.ofMinutes(9))
    rig.seeded.reachable = false
    rig.repo.refresh()
    val c = rig.repo.state.value.connection as Connection.Offline
    assertEquals(Fixtures.now, c.lastConnected)
    assertEquals("LAN", c.lastVia)
    assertNotNull(rig.repo.state.value.snapshot)
  }

  @Test fun `an answer given offline waits and is sent when the hub is back`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val a = ask(rig, "9MX4TB")
    rig.seeded.reachable = false
    assertEquals(SendResult.Queued, rig.repo.answer(a, "Yes, tonight"))
    assertEquals(1, rig.repo.state.value.outbox.size)
    rig.seeded.reachable = true
    rig.repo.refresh()
    assertTrue(rig.repo.state.value.outbox.isEmpty())
    assertEquals(AskStatus.Answered, ask(rig, "9MX4TB").status)
  }

  @Test fun `the outbox survives a restart`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.reachable = false
    rig.repo.answer(ask(rig, "9MX4TB"), "Yes, tonight")
    val again = HubRepository(Fixtures.hub, rig.seeded, rig.store, rig.clock)
    assertEquals("Yes, tonight", again.state.value.outbox.single().text)
  }

  @Test fun `a queued answer the hub then refuses keeps the text and says why`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    val a = ask(rig, "9MX4TB")
    rig.seeded.reachable = false
    rig.repo.answer(a, "Yes, tonight. Start after 22:00.")
    rig.seeded.reachable = true
    rig.seeded.refuseNext = Refusal.AnsweredElsewhere(Fixtures.now, "Wait for the weekend")
    rig.repo.refresh()
    val q = rig.repo.queuedFor("9MX4TB")!!
    assertTrue(q.refused)
    assertEquals("Yes, tonight. Start after 22:00.", q.text)
    val r = q.refusal!!.toRefusal() as Refusal.AnsweredElsewhere
    assertEquals("Wait for the weekend", r.answer)
  }

  @Test fun `a live refusal is reported and kept`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.refuseNext = Refusal.ClosedAtDeadline(Fixtures.now)
    val r = rig.repo.decide(ask(rig, "7HQ2KD"), true, null)
    assertTrue(r is SendResult.Refused)
    assertTrue(rig.repo.queuedFor("7HQ2KD")!!.refused)
  }

  @Test fun `discarding drops the queued text`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.reachable = false
    rig.repo.answer(ask(rig, "9MX4TB"), "Yes")
    rig.repo.dropQueued("9MX4TB")
    assertNull(rig.repo.queuedFor("9MX4TB"))
  }

  @Test fun `a revoked phone is told apart from an unreachable hub and clears its cache`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.repo.snooze("DS5REF")
    rig.seeded.revoked = true
    rig.repo.refresh()
    val s = rig.repo.state.value
    assertEquals(Connection.Unpaired, s.connection)
    assertNull(s.snapshot)
    assertTrue(s.snoozes.isEmpty())
  }

  @Test fun `unsent text stays after the phone is unpaired`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.reachable = false
    rig.repo.answer(ask(rig, "9MX4TB"), "Yes")
    rig.seeded.reachable = true
    rig.seeded.revoked = true
    rig.repo.refresh()
    assertEquals(1, rig.repo.state.value.outbox.size)
  }

  @Test fun `a hub older than the app needs is flagged`() = runTest {
    val rig = Fixtures.rig()
    rig.seeded.version = "1.11.0"
    rig.repo.refresh()
    assertEquals(Connection.TooOld("1.11.0", "1.13.0"), rig.repo.state.value.connection)
  }

  @Test fun `snooze is per phone and lasts an hour`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.repo.snooze("7HQ2KD")
    assertEquals(Fixtures.now.plus(Duration.ofHours(1)), rig.repo.state.value.snoozes["7HQ2KD"])
    val again = HubRepository(Fixtures.hub, rig.seeded, rig.store, rig.clock)
    assertEquals(1, again.state.value.snoozes.size)
    rig.repo.unsnooze("7HQ2KD")
    assertFalse(rig.repo.state.value.snoozes.containsKey("7HQ2KD"))
  }

  @Test fun `version comparison reads dotted numbers`() {
    assertTrue(Versions.compare("1.13.0", "1.9.2") > 0)
    assertTrue(Versions.compare("1.12.9", "1.13.0") < 0)
    assertEquals(0, Versions.compare("1.13", "1.13.0"))
  }

  @Test fun `unpairing hub A deletes its remembered passwords and keeps hub B's`() = runTest {
    val vault = MemoryPasswordVault().apply { add("seeded-hub", "space-a"); add("other-hub", "space-a") }
    val rig = Fixtures.rig()
    val repo = HubRepository(Fixtures.hub, rig.seeded, rig.store, rig.clock, onUnpaired = vault::forgetHub)
    repo.refresh()
    rig.seeded.revoked = true
    repo.refresh()
    assertTrue(vault.projects("seeded-hub").isEmpty())
    assertEquals(listOf("space-a"), vault.projects("other-hub"))
  }

  @Test fun `forgetting a hub in the container deletes its passwords too`() {
    val vault = MemoryPasswordVault().apply { add("seeded-hub", "space-a"); add("other-hub", "space-a") }
    val c = AppContainer(Fixtures.clock(), MemoryStore(), dev.agync.agenthub.pairing.SeededPairing(), HubClientFactory { SeededHub(Fixtures.clock()) }, vault = vault)
    c.registry.add(Fixtures.hub)
    c.forget("seeded-hub")
    assertTrue(vault.projects("seeded-hub").isEmpty())
    assertEquals(1, vault.projects("other-hub").size)
  }

  @Test fun `two hubs with a project of the same name keep separate passwords`() {
    val vault = MemoryPasswordVault().apply { add("a", "space-a") }
    assertTrue(vault.projects("b").isEmpty())
    vault.forget("b", "space-a")
    assertEquals(listOf("space-a"), vault.projects("a"))
  }

  @Test fun `an idea resolved offline waits and is sent when the hub is back`() = runTest {
    val rig = Fixtures.rig()
    rig.seeded.captureIdea("home-lab", "Try Longhorn", null)
    rig.repo.refresh()
    val idea = rig.repo.state.value.snapshot!!.asks.first { it.kind == dev.agync.agenthub.model.AskKind.Idea }
    rig.seeded.reachable = false
    assertEquals(SendResult.Queued, rig.repo.resolveIdea(idea))
    assertEquals(DecisionKind.Resolve, rig.repo.state.value.outbox.single().kind)
    rig.seeded.reachable = true
    rig.repo.refresh()
    assertTrue(rig.repo.state.value.outbox.isEmpty())
    assertEquals(AskStatus.Resolved, ask(rig, idea.id).status)
  }

  @Test fun `a note on an idea is never queued and leaves the idea waiting`() = runTest {
    val rig = Fixtures.rig()
    rig.seeded.captureIdea("home-lab", "Try Longhorn", null)
    rig.repo.refresh()
    val idea = rig.repo.state.value.snapshot!!.asks.first { it.kind == dev.agync.agenthub.model.AskKind.Idea }
    assertEquals(SendResult.Sent, rig.repo.replyIdea(idea, "Check the disks first"))
    assertEquals(listOf(idea.id to "Check the disks first"), rig.seeded.ideaNotes)
    rig.seeded.reachable = false
    assertEquals(SendResult.Failed, rig.repo.replyIdea(idea, "again"))
    assertTrue(rig.repo.state.value.outbox.isEmpty())
    assertEquals(AskStatus.Waiting, ask(rig, idea.id).status)
  }

  @Test fun `an approval the hub reserves for the web app is reported, queues nothing and leaves the connection alone`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.refuseAdmitFromPhone = true
    val join = ask(rig, "JN3STU")
    assertEquals(SendResult.NotAllowed, rig.repo.admit(join, true, "studio-mac", emptySet()))
    assertTrue(rig.repo.state.value.outbox.isEmpty())
    assertEquals(Connection.Online, rig.repo.state.value.connection)
    assertEquals(AskStatus.Waiting, ask(rig, "JN3STU").status)
  }

  @Test fun `an answer the hub rejects with a 4xx is reported, never queued, and the hub stays reachable`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.rejectAsks += "9MX4TB"
    assertEquals(SendResult.Rejected(422), rig.repo.answer(ask(rig, "9MX4TB"), "Yes"))
    assertTrue(rig.repo.state.value.outbox.isEmpty())
    assertEquals(Connection.Online, rig.repo.state.value.connection)
  }

  @Test fun `one entry the hub rejects does not hold up the ones behind it`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.reachable = false
    rig.repo.answer(ask(rig, "9MX4TB"), "Yes")
    rig.repo.decide(ask(rig, "7HQ2KD"), true, null)
    rig.repo.answer(ask(rig, "DS5REF"), "Guides")
    assertEquals(3, rig.repo.state.value.outbox.size)
    rig.seeded.reachable = true
    rig.seeded.rejectAsks += "9MX4TB"
    rig.repo.refresh()
    assertTrue(rig.repo.state.value.outbox.isEmpty())
    assertEquals(AskStatus.Waiting, ask(rig, "9MX4TB").status)
    assertEquals(AskStatus.Approved, ask(rig, "7HQ2KD").status)
    assertEquals(AskStatus.Answered, ask(rig, "DS5REF").status)
  }

  @Test fun `an entry the hub will not let a phone send is dropped and the rest still go`() = runTest {
    val rig = Fixtures.rig()
    rig.repo.refresh()
    rig.seeded.reachable = false
    rig.repo.admit(ask(rig, "JN3STU"), true, "studio-mac", emptySet())
    rig.repo.answer(ask(rig, "9MX4TB"), "Yes")
    rig.seeded.reachable = true
    rig.seeded.refuseAdmitFromPhone = true
    rig.repo.refresh()
    assertTrue(rig.repo.state.value.outbox.isEmpty())
    assertEquals(AskStatus.Answered, ask(rig, "9MX4TB").status)
    assertEquals(AskStatus.Waiting, ask(rig, "JN3STU").status)
  }
}
