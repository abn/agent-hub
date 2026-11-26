package dev.agync.agenthub.data

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.pairing.SeededPairing
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RegistryTest {
  private val hub = PairedHub("seeded-hub", "home hub", "pixel-phone", stateDir = "dir-1")

  private fun container(store: LocalStore, seeded: SeededHub, clock: Fixtures.MutableClock) =
    AppContainer(clock, store, SeededPairing(clock, 0, 0), HubClientFactory { seeded }, Fixtures.fmt(), "pixel-phone")

  @Test fun `a revocation is kept across a restart, and the restarted app does not ask the hub again`() = runTest {
    val store = MemoryStore()
    val clock = Fixtures.clock()
    val seeded = SeededHub(clock)
    val first = container(store, seeded, clock)
    first.registry.add(hub)
    seeded.revoked = true
    first.repository(hub).refresh()
    assertEquals(Connection.Unpaired, first.repository(hub).state.value.connection)

    // Restart: a new container over the same store. The hub would answer now, and must not be asked.
    seeded.revoked = false
    val second = container(store, seeded, clock)
    assertTrue(second.registry.state.value.hubs.single().unpaired)
    val repo = second.repository(second.registry.state.value.current!!)
    assertEquals(Connection.Unpaired, repo.state.value.connection)
    repo.refresh()
    assertEquals(Connection.Unpaired, repo.state.value.connection)
    assertEquals(null, repo.state.value.snapshot)
  }

  @Test fun `pairing the hub again clears the revocation`() = runTest {
    val store = MemoryStore()
    val clock = Fixtures.clock()
    val seeded = SeededHub(clock)
    val c = container(store, seeded, clock)
    c.registry.add(hub)
    c.registry.markUnpaired(hub.id)
    c.registry.add(hub.copy(stateDir = "dir-2"))
    assertFalse(c.registry.state.value.hubs.single().unpaired)
    c.repository(c.registry.state.value.current!!).refresh()
    assertEquals(Connection.Online, c.repository(c.registry.state.value.current!!).state.value.connection)
  }

  @Test fun `a hub that has answered as active is remembered, so a silent session reads as an unpairing after a restart`() = runTest {
    val store = MemoryStore()
    val clock = Fixtures.clock()
    val seeded = SeededHub(clock)
    val c = container(store, seeded, clock)
    c.registry.add(hub)
    assertFalse(c.registry.state.value.hubs.single().wasActive)
    c.repository(hub).refresh()
    assertTrue(HubRegistry(store).state.value.hubs.single().wasActive)
  }

  @Test fun `forgetting a hub drops its repository, its remembered passwords and its files`() = runTest {
    val store = MemoryStore()
    val clock = Fixtures.clock()
    val seeded = SeededHub(clock)
    val wiped = mutableListOf<String>()
    val vault = MemoryPasswordVault()
    val c = AppContainer(clock, store, SeededPairing(clock, 0, 0), HubClientFactory { seeded }, Fixtures.fmt(), "pixel-phone", vault = vault, wipe = { wiped += it.stateDir.orEmpty() })
    c.registry.add(hub)
    val first = c.repository(hub)
    vault.add(hub.id, "p")
    c.forget(hub.id)
    assertTrue(c.registry.state.value.hubs.isEmpty())
    assertEquals(listOf("dir-1"), wiped)
    assertEquals(null, vault.projects(hub.id).firstOrNull())
    c.registry.add(hub)
    assertTrue("a repository outlived its hub", first !== c.repository(hub))
  }
}
