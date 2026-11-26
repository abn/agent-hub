package dev.agync.agenthub.viewmodel

import dev.agync.agenthub.data.HubException
import dev.agync.agenthub.data.HubRepository
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

sealed interface Load<out T> {
  data object Loading : Load<Nothing>

  data class Ready<T>(val value: T) : Load<T>

  /** The hub could not be reached for this. The screen says so and offers Try again. */
  data object Unreachable : Load<Nothing>
}

/** One hub fetch a screen shows. Connection trouble is folded into the repository's state too. */
class Loader<T>(private val scope: CoroutineScope, private val repo: HubRepository, private val fetch: suspend dev.agync.agenthub.data.HubClient.() -> T) {
  private val _state = MutableStateFlow<Load<T>>(Load.Loading)
  val state: StateFlow<Load<T>> = _state

  fun reload() {
    scope.launch {
      val v =
        try {
          repo.client.fetch()
        } catch (e: HubException) {
          repo.guard(e)
          null
        }
      _state.value = if (v == null) Load.Unreachable else Load.Ready(v)
    }
  }

  init {
    reload()
  }
}
