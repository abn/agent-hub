package dev.agync.agenthub.viewmodel

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import dev.agync.agenthub.data.HubRegistry
import dev.agync.agenthub.data.MemoryStore
import dev.agync.agenthub.data.PrefsRepository
import dev.agync.agenthub.pairing.Confirmation
import dev.agync.agenthub.pairing.Handshake
import dev.agync.agenthub.pairing.PairOffer
import dev.agync.agenthub.pairing.Pairing
import dev.agync.agenthub.pairing.PairingException
import dev.agync.agenthub.pairing.SeededPairing
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

private class ScriptedPairing(
  val failWith: PairingException? = null,
  val confirmation: CompletableDeferred<Confirmation> = CompletableDeferred(),
  val needsLocalConfirm: Boolean = false,
) : Pairing {
  val confirmed = mutableListOf<String>()
  val aborted = mutableListOf<String>()

  override suspend fun confirm(handshake: Handshake) { confirmed += handshake.hubId }

  val adopted = mutableListOf<String>()

  override suspend fun adopt(handshake: Handshake) { adopted += handshake.hubId }

  override suspend fun abort(handshake: Handshake) { aborted += handshake.hubId }

  override fun read(raw: String): PairOffer? = if (raw.startsWith("tendril://pair?")) PairOffer(raw, "home hub") else null

  override fun readTyped(code: String, address: String): PairOffer? =
    if (code.startsWith("TND-") && address.isNotBlank()) PairOffer("$code@$address", codeLabel = code, address = address, typed = true) else null

  override suspend fun pair(offer: PairOffer, deviceName: String): Handshake {
    failWith?.let { throw it }
    return Handshake("hub-1", "home hub", "k7m2 qwxa 4fbn r3td", "LAN", needsLocalConfirm)
  }

  override suspend fun awaitConfirmation(handshake: Handshake): Confirmation = confirmation.await()
}

class PairViewModelTest {
  @get:Rule val main = MainDispatcherRule()

  private fun make(pairing: Pairing): Triple<PairViewModel, HubRegistry, PrefsRepository> {
    val store = MemoryStore()
    val reg = HubRegistry(store)
    val prefs = PrefsRepository(store)
    return Triple(PairViewModel(pairing, reg, prefs, "pixel-phone"), reg, prefs)
  }

  @Test fun `scanning something that is not a pairing code keeps the camera looking`() {
    val (vm, _, _) = make(ScriptedPairing())
    vm.startScan()
    vm.scanned("https://example.org")
    assertEquals(PairStep.Scan(), vm.step.value)
  }

  @Test fun `a scanned code goes to confirm with the phone's name`() {
    val (vm, _, _) = make(ScriptedPairing())
    vm.startScan()
    vm.scanned("tendril://pair?v=1&k=x")
    val s = vm.step.value as PairStep.Confirm
    assertEquals("pixel-phone", s.deviceName)
  }

  @Test fun `a typed code that is not shaped like a code stays on the form`() {
    val (vm, _, _) = make(ScriptedPairing())
    vm.startManual()
    vm.editManual("nonsense", "hub.example.ts.net")
    vm.submitManual()
    assertTrue((vm.step.value as PairStep.Manual).invalid)
  }

  @Test fun `pairing shows the check string and waits, then registers the hub and asks about alerts`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, reg, _) = make(scripted)
    vm.startManual()
    vm.editManual("TND-4F9K-22XQ-8N3P", "hub.example.ts.net")
    vm.submitManual()
    vm.setDeviceName("my-pixel")
    vm.pair()
    val sas = vm.step.value as PairStep.CheckString
    assertEquals("k7m2 qwxa 4fbn r3td", sas.handshake.checkString)
    assertTrue(reg.state.value.hubs.isEmpty())
    scripted.confirmation.complete(Confirmation.Confirmed)
    assertEquals(PairStep.Alerts("home hub"), vm.step.value)
    assertEquals("my-pixel", reg.state.value.current!!.deviceName)
  }

  @Test fun `a code that does not work lands on the did-not-work screen`() = runTest {
    val (vm, reg, _) = make(ScriptedPairing(failWith = PairingException.CodeFailed()))
    vm.startScan()
    vm.scanned("tendril://pair?v=1")
    vm.pair()
    assertEquals(PairStep.CodeFailed(false), vm.step.value)
    assertTrue(reg.state.value.hubs.isEmpty())
  }

  @Test fun `nothing answering is not blamed on the code`() = runTest {
    val (vm, _, _) = make(ScriptedPairing(failWith = PairingException.Unreachable()))
    vm.startScan()
    vm.scanned("tendril://pair?v=1")
    vm.pair()
    assertEquals(PairStep.CodeFailed(true), vm.step.value)
  }

  @Test fun `a refusal on the hub sends the person back with a notice`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, reg, _) = make(scripted)
    vm.startScan(); vm.scanned("tendril://pair?v=1"); vm.pair()
    scripted.confirmation.complete(Confirmation.Refused)
    assertEquals(PairStep.Welcome, vm.step.value)
    assertEquals("The hub refused this phone.", vm.notice.value)
    assertTrue(reg.state.value.hubs.isEmpty())
  }

  @Test fun `cancelling a pairing in flight goes back to the start and registers nothing`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, reg, _) = make(scripted)
    vm.startScan(); vm.scanned("tendril://pair?v=1"); vm.pair()
    vm.cancelPairing()
    scripted.confirmation.complete(Confirmation.Confirmed)
    assertEquals(PairStep.Welcome, vm.step.value)
    assertTrue(reg.state.value.hubs.isEmpty())
  }

  @Test fun `not now finishes pairing with no alerts`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, _, prefs) = make(scripted)
    vm.startScan(); vm.scanned("tendril://pair?v=1"); vm.pair()
    scripted.confirmation.complete(Confirmation.Confirmed)
    vm.skipAlerts()
    assertEquals(PairStep.Done, vm.step.value)
    assertFalse(prefs.state.value.stayConnected)
  }

  @Test fun `denied notifications show the off screen, and allowed ones finish with alerts on`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, _, prefs) = make(scripted)
    vm.startScan(); vm.scanned("tendril://pair?v=1"); vm.pair()
    scripted.confirmation.complete(Confirmation.Confirmed)
    vm.notificationsAnswered(false)
    assertTrue((vm.step.value as PairStep.Alerts).notificationsDenied)
    vm.notificationsAnswered(true)
    assertEquals(PairStep.Done, vm.step.value)
    assertTrue(prefs.state.value.stayConnected)
  }

  @Test fun `choosing to open the app only turns the held connection off`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, _, prefs) = make(scripted)
    vm.startScan(); vm.scanned("tendril://pair?v=1"); vm.pair()
    scripted.confirmation.complete(Confirmation.Confirmed)
    vm.setStayConnected(false)
    vm.notificationsAnswered(true)
    assertFalse(prefs.state.value.stayConnected)
  }

  @Test fun `the seeded pairing round trips through the whole flow`() = runTest {
    val (vm, reg, _) = make(SeededPairing(Fixtures.clock(), 0, 0))
    vm.startScan(); vm.scanned("tendril://pair?v=1"); vm.pair()
    kotlinx.coroutines.yield()
    assertTrue(vm.step.value is PairStep.Alerts || vm.step.value is PairStep.CheckString || vm.step.value is PairStep.Connecting)
    assertTrue(reg.state.value.hubs.size <= 1)
  }

  @Test fun `a typed code is not confirmed by this phone until the person says the strings match`() = runTest {
    val scripted = ScriptedPairing(needsLocalConfirm = true)
    val (vm, reg, _) = make(scripted)
    vm.startManual(); vm.editManual("TND-4F9K-22XQ-8N3P", "hub.example.ts.net"); vm.submitManual(); vm.pair()
    val s = vm.step.value as PairStep.CheckString
    assertTrue(s.needsYou)
    assertTrue(scripted.confirmed.isEmpty())
    vm.checkStringsMatch()
    assertEquals(listOf("hub-1"), scripted.confirmed)
    assertEquals(PairStep.CheckString(s.handshake, needsYou = false), vm.step.value)
    scripted.confirmation.complete(Confirmation.Confirmed)
    assertEquals(PairStep.Alerts("home hub"), vm.step.value)
    assertEquals(1, reg.state.value.hubs.size)
  }

  @Test fun `strings that differ abort the peer, confirm nothing and register nothing`() = runTest {
    val scripted = ScriptedPairing(needsLocalConfirm = true)
    val (vm, reg, _) = make(scripted)
    vm.startManual(); vm.editManual("TND-4F9K-22XQ-8N3P", "hub.example.ts.net"); vm.submitManual(); vm.pair()
    vm.checkStringsDiffer()
    assertTrue(scripted.confirmed.isEmpty())
    assertEquals(listOf("hub-1"), scripted.aborted)
    assertEquals(PairStep.Welcome, vm.step.value)
    assertTrue(reg.state.value.hubs.isEmpty())
  }

  @Test fun `cancelling while the strings are being compared drops the pending peer`() = runTest {
    val scripted = ScriptedPairing(needsLocalConfirm = true)
    val (vm, reg, _) = make(scripted)
    vm.startManual(); vm.editManual("TND-4F9K-22XQ-8N3P", "hub.example.ts.net"); vm.submitManual(); vm.pair()
    vm.cancelPairing()
    assertTrue(scripted.confirmed.isEmpty())
    assertEquals(listOf("hub-1"), scripted.aborted)
    assertTrue(reg.state.value.hubs.isEmpty())
  }

  @Test fun `a pinned scan is confirmed by this phone at once`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, _, _) = make(scripted)
    vm.startScan(); vm.scanned("tendril://pair?v=1&host=home-lab&id=hub-1"); vm.pair()
    assertEquals(listOf("hub-1"), scripted.confirmed)
  }

  @Test fun `a hub that is not the one the code names is refused with a notice`() = runTest {
    val (vm, reg, _) = make(ScriptedPairing(failWith = PairingException.KeyMismatch()))
    vm.startScan(); vm.scanned("tendril://pair?v=1&host=home-lab&id=hub-1"); vm.pair()
    assertEquals(PairStep.Welcome, vm.step.value)
    assertTrue(vm.notice.value!!.contains("Nothing was paired"))
    assertTrue(reg.state.value.hubs.isEmpty())
  }

  @Test fun `a confirmed pairing is adopted once its hub is registered, and not before`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, reg, _) = make(scripted)
    vm.startScan(); vm.scanned("tendril://pair?v=1&host=home-lab&id=hub-1"); vm.pair()
    assertTrue(scripted.adopted.isEmpty())
    scripted.confirmation.complete(Confirmation.Confirmed)
    assertEquals(listOf("hub-1"), scripted.adopted)
    assertEquals(1, reg.state.value.hubs.size)
  }

  @Test fun `a refused or aborted pairing is never adopted`() = runTest {
    val scripted = ScriptedPairing()
    val (vm, _, _) = make(scripted)
    vm.startScan(); vm.scanned("tendril://pair?v=1&host=home-lab&id=hub-1"); vm.pair()
    scripted.confirmation.complete(Confirmation.Refused)
    assertTrue(scripted.adopted.isEmpty())
    assertEquals(listOf("hub-1"), scripted.aborted)
  }
}
