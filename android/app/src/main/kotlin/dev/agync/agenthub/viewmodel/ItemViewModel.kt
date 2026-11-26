package dev.agync.agenthub.viewmodel

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.agync.agenthub.data.Connection
import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.data.QueuedDecision
import dev.agync.agenthub.data.Refusal
import dev.agync.agenthub.data.SendResult
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.AskStatus
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.NowBuilder
import java.time.Clock
import java.time.Instant
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

/** The item screen's states: open, waiting to send, refused, resolved elsewhere. */
sealed interface ItemUi {
  data object Loading : ItemUi

  data object Missing : ItemUi

  /** The ask waits on you. [position] of [total] counts the asks that need you. */
  data class Open(
    val ask: Ask,
    val position: Int,
    val total: Int,
    val next: Ask?,
    val snoozedUntil: Instant?,
    val offline: Boolean,
  ) : ItemUi

  /** Answered here while offline: waits to send. */
  data class Queued(val ask: Ask, val queued: QueuedDecision) : ItemUi

  /** The hub refused the queued answer because the ask closed first. */
  data class Refused(val ask: Ask, val queued: QueuedDecision, val refusal: Refusal) : ItemUi

  /** Decided on another device before this was opened. */
  data class Resolved(val ask: Ask, val stillNeedYou: Int) : ItemUi
}

sealed interface ItemEvent {
  /** The decision went through or is queued. Open [nextAskId] if there is one, else go back. */
  data class Done(val nextAskId: String?, val queued: Boolean) : ItemEvent

  data class Failed(val message: String) : ItemEvent

  data class Snoozed(val until: Instant) : ItemEvent

  /** A note was added to an idea; the idea stays open. */
  data object Noted : ItemEvent
}

class ItemViewModel(
  private val repo: HubRepository,
  private val askId: String,
  private val fmt: Fmt,
  private val clock: Clock,
) : ViewModel() {
  private val fetched = MutableStateFlow<Ask?>(null)
  private val _events = MutableSharedFlow<ItemEvent>(extraBufferCapacity = 4)
  val events: SharedFlow<ItemEvent> = _events.asSharedFlow()

  val ui: StateFlow<ItemUi> =
    combine(repo.state, fetched) { state, fetchedAsk ->
      val now = clock.instant()
      val inSnapshot = state.snapshot?.asks?.firstOrNull { it.id == askId }
      val ask = inSnapshot ?: fetchedAsk
      val queued = state.outbox.firstOrNull { it.askId == askId }
      val needs = NowBuilder.build(state, now, fmt).needsYou
      when {
        queued != null && queued.refusal != null && ask != null -> ItemUi.Refused(ask, queued, queued.refusal.toRefusal())
        queued != null && ask != null -> ItemUi.Queued(ask, queued)
        ask == null -> if (state.snapshot != null || state.connection is Connection.Offline) ItemUi.Missing else ItemUi.Loading
        ask.status != AskStatus.Waiting -> ItemUi.Resolved(ask, needs.size)
        else -> {
          val idx = needs.indexOfFirst { it.id == askId }
          val position = if (idx >= 0) idx + 1 else needs.size + 1
          val next = needs.firstOrNull { it.id != askId }
          ItemUi.Open(ask, position, needs.size.coerceAtLeast(position), next, state.snoozes[askId]?.takeIf { it.isAfter(now) }, state.connection is Connection.Offline)
        }
      }
    }.stateIn(viewModelScope, SharingStarted.Eagerly, ItemUi.Loading)

  init {
    // A notification can open an ask the cache never saw: ask the hub for it.
    if (repo.state.value.snapshot?.asks?.none { it.id == askId } != false) {
      viewModelScope.launch { fetched.value = repo.askById(askId) }
    }
  }

  private fun nextAfter(): String? = (ui.value as? ItemUi.Open)?.next?.id

  private fun finish(r: SendResult, next: String?) {
    when (r) {
      SendResult.Sent -> _events.tryEmit(ItemEvent.Done(next, queued = false))
      SendResult.Queued -> _events.tryEmit(ItemEvent.Done(next, queued = true))
      is SendResult.Refused -> Unit // the item screen turns into the refused state by itself
      SendResult.Failed -> _events.tryEmit(ItemEvent.Failed("The hub could not take that. Try again."))
      is SendResult.Rejected -> _events.tryEmit(ItemEvent.Failed("The hub did not accept that."))
      SendResult.NotAllowed -> _events.tryEmit(ItemEvent.Failed("Approve this one from the web app. The hub does not let a phone do it."))
    }
  }

  fun answer(text: String) {
    val ask = (ui.value as? ItemUi.Open)?.ask ?: return
    if (text.isBlank()) return
    val next = nextAfter()
    viewModelScope.launch { finish(repo.answer(ask, text.trim()), next) }
  }

  fun decide(approve: Boolean, note: String?) {
    val ask = (ui.value as? ItemUi.Open)?.ask ?: return
    val next = nextAfter()
    viewModelScope.launch { finish(repo.decide(ask, approve, note?.trim()?.takeIf { it.isNotEmpty() }), next) }
  }

  fun admit(approve: Boolean, name: String, grants: Set<String>) {
    val ask = (ui.value as? ItemUi.Open)?.ask ?: return
    val next = nextAfter()
    viewModelScope.launch { finish(repo.admit(ask, approve, name.trim().ifEmpty { ask.agent }, grants), next) }
  }

  fun resolveIdea() = closeIdea(resolve = true)

  fun clearIdea() = closeIdea(resolve = false)

  private fun closeIdea(resolve: Boolean) {
    val ask = (ui.value as? ItemUi.Open)?.ask ?: return
    val next = nextAfter()
    viewModelScope.launch { finish(if (resolve) repo.resolveIdea(ask) else repo.clearIdea(ask), next) }
  }

  /** A note on the idea. The item stays open: the field empties on [ItemEvent.Noted]. */
  fun replyIdea(text: String) {
    val ask = (ui.value as? ItemUi.Open)?.ask ?: return
    if (text.isBlank()) return
    viewModelScope.launch {
      when (repo.replyIdea(ask, text.trim())) {
        SendResult.Sent -> _events.tryEmit(ItemEvent.Noted)
        else -> _events.tryEmit(ItemEvent.Failed("The hub could not take that note. Try again."))
      }
    }
  }

  fun snooze() {
    val ask = (ui.value as? ItemUi.Open)?.ask ?: return
    repo.snooze(ask.id)
    _events.tryEmit(ItemEvent.Snoozed(clock.instant().plus(HubRepository.SNOOZE)))
  }

  /** Cancels a queued answer, or discards a refused one. */
  fun discardQueued() = repo.dropQueued(askId)

  fun kindOf(): AskKind? = (ui.value as? ItemUi.Open)?.ask?.kind
}
