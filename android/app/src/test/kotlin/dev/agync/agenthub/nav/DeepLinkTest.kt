package dev.agync.agenthub.nav

import dev.agync.agenthub.data.HubRegistry
import dev.agync.agenthub.data.MemoryStore
import dev.agync.agenthub.data.PairedHub
import org.junit.Assert.assertEquals
import org.junit.Test

class DeepLinkTest {
  private fun registry() = HubRegistry(MemoryStore()).also {
    it.add(PairedHub("hub-a", "a", "phone", "d1"))
    it.add(PairedHub("hub-b", "b", "phone", "d2"))
    it.select("hub-b")
  }

  @Test fun `an ask on another hub opens on that hub`() {
    val r = registry()
    val nav = Navigator()
    DeepLink.open("ask-1", "hub-a", r, nav)
    assertEquals("hub-a", r.state.value.current!!.id)
    assertEquals(Route.Item("ask-1"), nav.current)
  }

  @Test fun `a finished-work notification names no ask and opens Now, not an item with no id`() {
    val r = registry()
    val nav = Navigator()
    DeepLink.open("", "hub-a", r, nav)
    assertEquals(Route.Now, nav.current)
    assertEquals("hub-a", r.state.value.current!!.id)
  }

  @Test fun `a hub the phone is not paired with changes nothing`() {
    val r = registry()
    val nav = Navigator()
    DeepLink.open("ask-1", "hub-zzz", r, nav)
    assertEquals("hub-b", r.state.value.current!!.id)
  }
}
