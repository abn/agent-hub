package dev.agync.agenthub.tendril

import dev.agync.agenthub.pairing.PairingException
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

private class FakeOps(var hostId: String = "hub-1", var hostConfirmed: Boolean = true) : TendrilOps {
  val calls = mutableListOf<String>()

  private fun result() = """{"host_id":"$hostId","sas":"k7m2 qwxa 4fbn r3td","host_confirmed":$hostConfirmed,"provider":"direct"}"""

  var lastUri: String? = null

  override fun pairUri(uri: String, deviceName: String): String { calls += "pairUri"; lastUri = uri; return result() }

  override fun pairInitiate(endpoint: String, code: String, deviceName: String): String { calls += "pairInitiate"; return result() }

  override fun confirmPeer(peerId: String, sas: String?) { calls += "confirm:$peerId" }

  override fun revokePeer(peerId: String) { calls += "revoke:$peerId" }

  override fun setPeerRoutes(peerId: String, routes: List<String>) { calls += "routes:$peerId:${routes.joinToString()}" }

  var standings: ArrayDeque<Standing> = ArrayDeque()

  var onStanding: () -> Unit = {}

  override fun standing(hostId: String): Standing { onStanding(); return standings.removeFirstOrNull() ?: Standing.Pending }

  override val stateDir: String? = "dir-1"

  override fun adopt() { calls += "adopt" }

  override fun discard() { calls += "discard" }
}

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class TendrilPairingTest {
  @Test fun `a typed code never reaches confirmPeer during pair`() = runTest {
    val ops = FakeOps()
    val p = TendrilPairing(ops)
    val h = p.pair(p.readTyped("TND-4F9K-22XQ-8N3P", "hub.example.ts.net")!!, "pixel-phone")
    assertTrue(h.needsLocalConfirm)
    assertEquals(listOf("pairInitiate"), ops.calls)
  }

  @Test fun `confirming a typed code happens only when asked, and abort revokes the pending peer`() = runTest {
    val ops = FakeOps()
    val p = TendrilPairing(ops)
    val h = p.pair(p.readTyped("TND-4F9K-22XQ-8N3P", "hub.example.ts.net")!!, "pixel-phone")
    p.abort(h)
    assertEquals(listOf("pairInitiate", "revoke:hub-1", "discard"), ops.calls)
    assertFalse(ops.calls.any { it.startsWith("confirm") })
    p.confirm(h)
    assertTrue(ops.calls.contains("confirm:hub-1"))
    assertTrue(ops.calls.contains("routes:hub-1:hub.example.ts.net"))
    assertEquals("dir-1", h.stateDir)
  }

  @Test fun `a scanned code that pins the host is verified and needs no local confirm`() = runTest {
    val ops = FakeOps()
    val p = TendrilPairing(ops)
    val offer = p.read("tendril://pair?v=1&host=home-lab&id=hub-1&code=TND-AAAA-BBBB&k=AA&sh=AA")!!
    assertEquals("hub-1", offer.pinnedHostId)
    assertEquals("home-lab", offer.hubName)
    val h = p.pair(offer, "pixel-phone")
    assertFalse(h.needsLocalConfirm)
    assertEquals("LAN", h.via)
    assertEquals(listOf("pairUri"), ops.calls)
  }

  @Test fun `a scanned code with a different host id fails closed and drops the peer`() = runTest {
    val ops = FakeOps(hostId = "someone-else")
    val p = TendrilPairing(ops)
    val offer = p.read("tendril://pair?v=1&host=home-lab&id=hub-1&k=AA&sh=AA")!!
    assertThrows(PairingException.KeyMismatch::class.java) { kotlinx.coroutines.runBlocking { p.pair(offer, "pixel-phone") } }
    assertEquals(listOf("pairUri", "revoke:someone-else", "discard"), ops.calls)
  }

  @Test fun `a scanned pin Tendril did not verify fails closed`() = runTest {
    val ops = FakeOps(hostConfirmed = false)
    val p = TendrilPairing(ops)
    val offer = p.read("tendril://pair?v=1&host=home-lab&id=hub-1&k=AA&sh=AA")!!
    assertThrows(PairingException.KeyMismatch::class.java) { kotlinx.coroutines.runBlocking { p.pair(offer, "pixel-phone") } }
    assertTrue(ops.calls.none { it.startsWith("confirm") })
  }

  @Test fun `a scanned code that names no host pins nothing and asks the person`() = runTest {
    val ops = FakeOps()
    val p = TendrilPairing(ops)
    val offer = p.read("tendril://pair?v=1&k=AA&sh=AA")!!
    assertNull(offer.pinnedHostId)
    assertTrue(p.pair(offer, "pixel-phone").needsLocalConfirm)
  }

  @Test fun `only tendril pairing URIs are read`() {
    assertNull(TendrilPairing(FakeOps()).read("https://example.org"))
  }

  @Test fun `confirming a pinned scan calls nothing, because the pin already verified the hub`() = runTest {
    val ops = FakeOps()
    val p = TendrilPairing(ops)
    val h = p.pair(p.read("tendril://pair?v=1&host=home-lab&id=hub-1&k=AA&sh=AA")!!, "pixel-phone")
    p.confirm(h)
    assertEquals(listOf("pairUri"), ops.calls)
  }

  @Test fun `a typed code is always three groups, and a two-group one is not accepted`() {
    val p = TendrilPairing(FakeOps())
    assertTrue(p.readTyped("TND-4F9K-22XQ-8N3P", "hub.example.ts.net") != null)
    assertEquals(null, p.readTyped("TND-4F9K-22XQ", "hub.example.ts.net"))
    assertEquals(null, p.readTyped("TND-4F9K-22XQ-8N3P-9ZZZ", "hub.example.ts.net"))
  }

  /** A clock that reads the test's virtual time, so the wait's deadline and its delays agree. */
  private fun kotlinx.coroutines.test.TestScope.clock() = object : java.time.Clock() {
    override fun getZone() = java.time.ZoneOffset.UTC
    override fun withZone(zone: java.time.ZoneId?) = this
    override fun instant(): java.time.Instant = java.time.Instant.ofEpochMilli(testScheduler.currentTime)
  }

  @Test fun `waiting for the operator backs off from two seconds to thirty, and ends when the hub confirms`() = runTest {
    val ops = FakeOps()
    val asked = mutableListOf<Long>()
    ops.onStanding = { asked += testScheduler.currentTime }
    repeat(7) { ops.standings.addLast(Standing.Pending) }
    ops.standings.addLast(Standing.Active)
    val p = TendrilPairing(ops, clock())
    val h = p.pair(p.readTyped("TND-4F9K-22XQ-8N3P", "hub.example.ts.net")!!, "pixel-phone")
    assertEquals(dev.agync.agenthub.pairing.Confirmation.Confirmed, p.awaitConfirmation(h))
    assertEquals(listOf(0L, 2_000L, 6_000L, 14_000L, 30_000L, 60_000L, 90_000L, 120_000L), asked)
  }

  @Test fun `a hub that turns the phone away ends the wait as refused, at once`() = runTest {
    val ops = FakeOps()
    ops.standings.addLast(Standing.Pending)
    ops.standings.addLast(Standing.Refused)
    val p = TendrilPairing(ops, clock())
    val h = p.pair(p.readTyped("TND-4F9K-22XQ-8N3P", "hub.example.ts.net")!!, "pixel-phone")
    assertEquals(dev.agync.agenthub.pairing.Confirmation.Refused, p.awaitConfirmation(h))
  }

  @Test fun `a wait nobody answers times out after the allowed time`() = runTest {
    val ops = FakeOps()
    val p = TendrilPairing(ops, clock(), confirmTimeout = java.time.Duration.ofMinutes(2))
    val h = p.pair(p.readTyped("TND-4F9K-22XQ-8N3P", "hub.example.ts.net")!!, "pixel-phone")
    assertEquals(dev.agync.agenthub.pairing.Confirmation.TimedOut, p.awaitConfirmation(h))
  }

  @Test fun `the scanned string reaches Tendril exactly as it was read`() = runTest {
    val ops = FakeOps()
    val p = TendrilPairing(ops)
    val scanned = "tendril://pair?v=1&host=home%2Dlab&id=hub-1&k=AA%3D%3D&sh=AA&ep=direct%3A10.0.0.2%3A48732"
    val offer = p.read(scanned)!!
    assertEquals(scanned, offer.raw)
    p.pair(offer, "pixel-phone")
    assertEquals(scanned, ops.lastUri)
    assertNull("a string with stray whitespace is not what the hub showed", p.read(" $scanned\n"))
  }
}
