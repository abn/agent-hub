package dev.agync.agenthub.data

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.serialization.Serializable

@Serializable
private data class RegistryDisk(val hubs: List<PairedHub> = emptyList(), val currentId: String? = null)

data class RegistryState(val hubs: List<PairedHub>, val currentId: String?) {
  val current: PairedHub? get() = hubs.firstOrNull { it.id == currentId } ?: hubs.firstOrNull()
}

/** The hubs this phone is paired with, and which one is showing. Holds names and ids only. */
class HubRegistry(private val store: LocalStore, private val onReplaced: (PairedHub) -> Unit = {}) {
  private val _state = MutableStateFlow(load())
  val state: StateFlow<RegistryState> = _state

  private fun load(): RegistryState {
    val disk = store.get(KEY)?.let { runCatching { StoreJson.decodeFromString(RegistryDisk.serializer(), it) }.getOrNull() } ?: RegistryDisk()
    return RegistryState(disk.hubs, disk.currentId)
  }

  private fun save() = store.put(KEY, StoreJson.encodeToString(RegistryDisk.serializer(), RegistryDisk(_state.value.hubs, _state.value.currentId)))

  fun add(hub: PairedHub) {
    // Pairing a hub again brings a fresh identity: what the old one kept on the phone goes.
    _state.value.hubs.firstOrNull { it.id == hub.id && it.stateDir != hub.stateDir }?.let(onReplaced)
    _state.update { s -> RegistryState(s.hubs.filter { it.id != hub.id } + hub, hub.id) }
    save()
  }

  /** The hub revoked this phone. Persisted, so it survives a restart. */
  fun markUnpaired(id: String) = change(id) { it.copy(unpaired = true) }

  fun markActive(id: String) = change(id) { it.copy(wasActive = true) }

  private fun change(id: String, edit: (PairedHub) -> PairedHub) {
    val before = _state.value.hubs.firstOrNull { it.id == id } ?: return
    val after = edit(before)
    if (after == before) return
    _state.update { s -> s.copy(hubs = s.hubs.map { if (it.id == id) after else it }) }
    save()
  }

  fun select(id: String) {
    _state.update { it.copy(currentId = id) }
    save()
  }

  fun remove(id: String) {
    _state.update { s ->
      val rest = s.hubs.filter { it.id != id }
      RegistryState(rest, if (s.currentId == id) rest.firstOrNull()?.id else s.currentId)
    }
    save()
  }


  private companion object {
    const val KEY = "registry"
  }
}

/** Device-wide preferences: how alerts arrive, the fingerprint opt-in, the theme. */
class PrefsRepository(private val store: LocalStore) {
  private val _state = MutableStateFlow(load())
  val state: StateFlow<DevicePrefs> = _state

  private fun load() =
    store.get(KEY)?.let { runCatching { StoreJson.decodeFromString(DevicePrefs.serializer(), it) }.getOrNull() } ?: DevicePrefs()

  fun update(change: (DevicePrefs) -> DevicePrefs) {
    _state.update(change)
    store.put(KEY, StoreJson.encodeToString(DevicePrefs.serializer(), _state.value))
  }

  private companion object {
    const val KEY = "prefs"
  }
}
