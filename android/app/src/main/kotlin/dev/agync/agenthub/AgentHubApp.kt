package dev.agync.agenthub

import android.app.Application
import dev.agync.agenthub.alerts.AlertCenter
import dev.agync.agenthub.data.AppContainer
import dev.agync.agenthub.data.HubClient
import dev.agync.agenthub.data.HubClientFactory
import dev.agync.agenthub.data.PairedHub
import dev.agync.agenthub.data.PrefsStore
import dev.agync.agenthub.data.SeededHub
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.pairing.Pairing
import dev.agync.agenthub.pairing.SeededPairing
import dev.agync.agenthub.tendril.TendrilHubClient
import dev.agync.agenthub.tendril.TendrilPairing
import dev.agync.agenthub.tendril.EndpointOps
import dev.agync.agenthub.tendril.EndpointStore
import dev.agync.agenthub.tendril.HubTransport
import dev.agync.agenthub.tendril.SessionPipe
import java.io.File
import java.time.Clock

class AgentHubApp : Application() {
  lateinit var container: AppContainer
    private set

  lateinit var alerts: AlertCenter
    private set

  override fun onCreate() {
    super.onCreate()
    val clock = Clock.systemDefaultZone()
    val store = PrefsStore(this)
    val seeded = BuildConfig.HUB_TRANSPORT == "seeded"
    // The directories of paired hubs, read from the persisted registry so no pairing can delete one.
    fun pairedDirs() = dev.agync.agenthub.data.HubRegistry(store).state.value.hubs.mapNotNull { it.stateDir }.toSet()
    val pairing: Pairing
    val clients: HubClientFactory
    var endpoints: EndpointStore? = null
    if (seeded) {
      val hub = SeededHub(clock)
      pairing = SeededPairing(clock)
      clients = HubClientFactory { _: PairedHub -> hub as HubClient }
    } else {
      val tendrilStore = EndpointStore(File(filesDir, "tendril").also { it.mkdirs() })
      endpoints = tendrilStore
      // A pairing a killed process left behind has no hub: its directory goes.
      tendrilStore.sweep(pairedDirs())
      pairing = TendrilPairing(EndpointOps(tendrilStore, owned = { dir -> pairedDirs().contains(dir) }), clock)
      clients =
        HubClientFactory { h: PairedHub ->
          val ep = tendrilStore.open(h.stateDir ?: error("a paired hub has a state directory"))
          // A hub that was active before keeps that across restarts: a session that ends with nothing back then reads as an unpairing.
          TendrilHubClient(HubTransport { SessionPipe(ep.connect(targetHostId = h.id)) }.also { if (h.wasActive) it.markActive() }, clock)
        }
    }
    if (seeded && store.get("snoozes.seeded-hub") == null) {
      // The board shows the docs-sweep ask snoozed on this phone.
      val until = clock.instant().plusSeconds(90 * 60).toEpochMilli()
      store.put("snoozes.seeded-hub", dev.agync.agenthub.data.StoreJson.encodeToString(dev.agync.agenthub.data.Snoozes.serializer(), dev.agync.agenthub.data.Snoozes(mapOf("DS5REF" to until))))
    }
    container = AppContainer(clock, store, pairing, clients, Fmt(), deviceName = android.os.Build.MODEL?.lowercase()?.replace(' ', '-') ?: "my-phone", vault = dev.agync.agenthub.data.PrefsPasswordVault(this), wipe = { hub -> hub.stateDir?.let { endpoints?.delete(it) } })
    alerts = AlertCenter(this, container)
  }
}
