package dev.agync.agenthub.viewmodel

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.agync.agenthub.data.Connection
import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.data.SendResult
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.NowBuilder
import dev.agync.agenthub.model.NowFilter
import dev.agync.agenthub.model.NowModel
import java.time.Clock
import java.time.Instant
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

/** What the Now screen is, before it is a list. */
sealed interface NowScreenKind {
  data object Loading : NowScreenKind

  data object Content : NowScreenKind

  /** No route answered and nothing is cached. */
  data class Unreachable(val attemptedAt: String, val lastConnected: String?) : NowScreenKind

  data class Unpaired(val deviceName: String) : NowScreenKind

  data class TooOld(val hubVersion: String, val needs: String, val via: String?) : NowScreenKind
}

data class NowUi(
  val hubName: String,
  val kind: NowScreenKind,
  val model: NowModel,
  val filter: NowFilter,
  /** "Tue 24 Nov · 2 live · synced 14:32", or "as of 14:30" when the hub cannot be reached. */
  val meta: String,
  /** The offline banner, or null when connected. */
  val offlineBanner: String?,
  val sinceLabel: String,
  val now: Instant,
  val unsent: List<dev.agync.agenthub.data.QueuedDecision>,
  val refused: List<dev.agync.agenthub.data.QueuedDecision>,
  val lastVia: String?,
  val snoozes: Map<String, Instant> = emptyMap(),
)

sealed interface NowEvent {
  data class Snoozed(val askId: String, val until: String) : NowEvent

  data class Queued(val what: String) : NowEvent

  data class Refused(val what: String) : NowEvent
}

class NowViewModel(
  private val repo: HubRepository,
  private val fmt: Fmt,
  private val clock: Clock,
  private val dispatcher: CoroutineDispatcher = Dispatchers.Default,
) : ViewModel() {
  private val filter = MutableStateFlow(NowFilter.All)
  private val tick = MutableStateFlow(clock.instant())
  private val _events = MutableSharedFlow<NowEvent>(extraBufferCapacity = 8)
  val events: SharedFlow<NowEvent> = _events.asSharedFlow()

  val ui: StateFlow<NowUi> =
    combine(repo.state, repo.syncedAt, filter, tick) { state, syncedAt, f, now ->
      val model = NowBuilder.build(state, now, fmt)
      val offline = state.connection as? Connection.Offline
      val snapshot = state.snapshot
      val kind =
        when {
          state.connection is Connection.Unpaired -> NowScreenKind.Unpaired(repo.hub.deviceName)
          state.connection is Connection.TooOld ->
            NowScreenKind.TooOld(state.connection.hubVersion, state.connection.needs, state.lastVia)
          snapshot == null && offline != null ->
            NowScreenKind.Unreachable(fmt.time(offline.attemptedAt), offline.lastConnected?.let(fmt::time)?.let { t -> via(state.lastVia, t) })
          snapshot == null -> NowScreenKind.Loading
          else -> NowScreenKind.Content
        }
      val meta =
        if (offline != null && snapshot != null) "${fmt.day(now)} · as of ${fmt.time(syncedAt ?: snapshot.asOf)}"
        else "${fmt.day(now)} · ${model.live.count { it.parent == null }} live · synced ${fmt.time(syncedAt ?: snapshot?.asOf ?: now)}"
      val banner =
        offline?.takeIf { snapshot != null }?.let {
          val last = it.lastConnected?.let { t -> "Last connected ${via(state.lastVia, fmt.time(t))}. " }.orEmpty()
          "Cannot reach ${repo.hub.name}. ${last}Answers you give now wait on this phone."
        }
      NowUi(
        hubName = repo.hub.name,
        kind = kind,
        model = model,
        filter = f,
        meta = meta,
        offlineBanner = banner,
        sinceLabel = if (offline != null) "Waiting to send" else "Since you looked",
        now = now,
        unsent = state.outbox.filter { !it.refused },
        refused = state.outbox.filter { it.refused },
        lastVia = state.lastVia,
        snoozes = state.snoozes,
      )
    }
      // The list is built off the main thread, and only while a screen is collecting it: with the app in the background
      // nothing is built, and it is built once more, from the latest state, when the screen comes back.
      .flowOn(dispatcher)
      .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), initialUi())

  private fun initialUi(): NowUi =
    NowUi(repo.hub.name, NowScreenKind.Loading, NowBuilder.build(repo.state.value, clock.instant(), fmt), NowFilter.All, "", null, "Since you looked", clock.instant(), emptyList(), emptyList(), null)

  private fun via(via: String?, time: String) = if (via != null) "via $via at $time" else "at $time"

  fun setFilter(f: NowFilter) {
    filter.value = f
  }

  /** The minute hand: lets "due in" lines and snoozes expire on screen. */
  fun tickClock() {
    tick.value = clock.instant()
  }

  fun refresh() {
    viewModelScope.launch {
      repo.refresh()
      tickClock()
    }
  }

  fun answer(ask: Ask, text: String) {
    viewModelScope.launch {
      when (val r = repo.answer(ask, text)) {
        SendResult.Queued -> _events.tryEmit(NowEvent.Queued("Your answer waits on this phone"))
        is SendResult.Refused -> _events.tryEmit(NowEvent.Refused("Your answer was not sent"))
        is SendResult.Rejected -> _events.tryEmit(NowEvent.Refused("The hub did not accept your answer"))
        else -> Unit
      }
    }
  }

  fun resolveIdea(ask: Ask) = closeIdea(ask, resolve = true)

  fun clearIdea(ask: Ask) = closeIdea(ask, resolve = false)

  private fun closeIdea(ask: Ask, resolve: Boolean) {
    viewModelScope.launch {
      when (if (resolve) repo.resolveIdea(ask) else repo.clearIdea(ask)) {
        SendResult.Queued -> _events.tryEmit(NowEvent.Queued("Waits on this phone until the hub is back"))
        else -> Unit
      }
    }
  }

  fun decide(ask: Ask, approve: Boolean) {
    viewModelScope.launch {
      when (repo.decide(ask, approve, null)) {
        SendResult.Queued -> _events.tryEmit(NowEvent.Queued("Your decision waits on this phone"))
        is SendResult.Refused -> _events.tryEmit(NowEvent.Refused("Your decision was not sent"))
        is SendResult.Rejected -> _events.tryEmit(NowEvent.Refused("The hub did not accept your decision"))
        else -> Unit
      }
    }
  }

  fun snooze(ask: Ask) {
    repo.snooze(ask.id)
    tickClock()
    _events.tryEmit(NowEvent.Snoozed(ask.id, fmt.time(clock.instant().plus(HubRepository.SNOOZE))))
  }

  fun undoSnooze(askId: String) {
    repo.unsnooze(askId)
    tickClock()
  }

  fun cancelQueued(askId: String) = repo.dropQueued(askId)

  fun markSeen() {
    viewModelScope.launch { repo.markSeen() }
  }
}
