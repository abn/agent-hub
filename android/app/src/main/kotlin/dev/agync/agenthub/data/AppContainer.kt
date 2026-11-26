package dev.agync.agenthub.data

import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.pairing.Pairing
import java.time.Clock

/** Builds the client for a paired hub. The real one speaks Tendril; the seeded one is in memory. */
fun interface HubClientFactory {
  fun create(hub: PairedHub): HubClient
}

/** The app's graph, built by hand: few enough pieces that a framework would only hide them. */
class AppContainer(
  val clock: Clock,
  val store: LocalStore,
  val pairing: Pairing,
  private val clients: HubClientFactory,
  val fmt: Fmt = Fmt(),
  val deviceName: String = "my-phone",
  val vault: PasswordVault = MemoryPasswordVault(),
  private val wipe: (PairedHub) -> Unit = {},
) {
  // Pairing a hub again brings a new identity: the repository built on the old one (a closed endpoint, a stuck unpaired
  // transport) is dropped with its files, so the next one is built on the new.
  val registry = HubRegistry(store, onReplaced = { hub -> dropRepository(hub.id); wipe(hub) })
  val prefs = PrefsRepository(store)
  private val repos = HashMap<String, HubRepository>()

  @Synchronized
  fun repository(hub: PairedHub): HubRepository =
    repos.getOrPut(hub.id) {
      HubRepository(
        hub, clients.create(hub), store, clock,
        onUnpaired = { id -> vault.forgetHub(id); registry.markUnpaired(id) },
        onActive = registry::markActive,
      )
    }

  @Synchronized
  private fun dropRepository(hubId: String) {
    repos.remove(hubId)
  }

  fun currentRepository(): HubRepository? = registry.state.value.current?.let(::repository)

  @Synchronized
  fun forget(hubId: String) {
    repos.remove(hubId)
    vault.forgetHub(hubId)
    registry.state.value.hubs.firstOrNull { it.id == hubId }?.let(wipe)
    registry.remove(hubId)
  }
}
