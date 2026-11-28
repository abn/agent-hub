package dev.agync.agenthub.data

import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskStatus
import java.time.Clock
import java.time.Duration
import java.time.Instant
import java.time.temporal.ChronoUnit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.encodeToString

sealed interface Connection {
  /** Nothing asked yet since the app opened. */
  data object Connecting : Connection

  data object Online : Connection

  /** No route answered. [lastConnected] and [lastVia] are the last time one did. */
  data class Offline(val attemptedAt: Instant, val lastConnected: Instant?, val lastVia: String?) : Connection

  /** The hub revoked this phone. */
  data object Unpaired : Connection

  data class TooOld(val hubVersion: String, val needs: String) : Connection
}

data class HubState(
  val hub: PairedHub,
  val connection: Connection = Connection.Connecting,
  val snapshot: NowSnapshot? = null,
  val outbox: List<QueuedDecision> = emptyList(),
  val snoozes: Map<String, Instant> = emptyMap(),
  val lastVia: String? = null,
  val seenAt: Instant? = null,
)

/**
 * One paired hub: its cache, what waits to be sent, this phone's snoozes, and the connection.
 * A single place decides what an answer does when the hub cannot be reached or has moved on.
 */
class HubRepository(
  val hub: PairedHub,
  val client: HubClient,
  private val store: LocalStore,
  private val clock: Clock = Clock.systemDefaultZone(),
  private val onUnpaired: (String) -> Unit = {},
  private val onActive: (String) -> Unit = {},
) {
  private val mutex = Mutex()
  private var activeReported = false
  private val outboxKey = "outbox." + hub.id
  private val snoozeKey = "snoozes." + hub.id
  private val seenKey = "seen." + hub.id

  private val _state =
    MutableStateFlow(
      HubState(
        hub = hub,
        connection = if (hub.unpaired) Connection.Unpaired else Connection.Connecting,
        outbox = loadOutbox(),
        snoozes = loadSnoozes(),
        seenAt = store.get(seenKey)?.toLongOrNull()?.let(Instant::ofEpochMilli),
      ),
    )
  val state: StateFlow<HubState> = _state

  /** The last time the hub answered, as precisely as the clock gives it. Not part of [state], so it never wakes a collector. */
  private var lastReached: Instant? = null
  private val _syncedAt = MutableStateFlow<Instant?>(null)

  /**
   * When the hub last answered, to the minute, which is all the screens show of it. It is kept out of [state] so a
   * refresh that finds nothing new changes nothing a collector of [state] sees; this one moves at most once a minute.
   */
  val syncedAt: StateFlow<Instant?> = _syncedAt

  val now: Instant get() = clock.instant()

  /** Fetches Now, settles the connection, and sends what waited. Safe to call from any trigger. */
  suspend fun refresh() = mutex.withLock { refreshLocked() }

  private suspend fun refreshLocked() {
    // A hub that revoked this phone is not asked again: the person pairs again.
    if (hub.unpaired) return
    try {
      val snapshot = client.now()
      checkVersion(snapshot.info.version)
      val info = snapshot.info
      if (!hub.wasActive && !activeReported) {
        activeReported = true
        onActive(hub.id)
      }
      val reached = clock.instant()
      lastReached = reached
      _syncedAt.value = reached.truncatedTo(ChronoUnit.MINUTES)
      val via = snapshot.info.via ?: info.via
      // The same hub data over the same route is not news: the state is left alone, so nothing downstream runs.
      _state.update {
        if (it.connection == Connection.Online && it.lastVia == via && it.snapshot?.sameContentAs(snapshot) == true) it
        else it.copy(connection = Connection.Online, snapshot = snapshot, lastVia = via)
      }
      if (flushOutbox()) {
        // What waited has gone through, so what the hub holds has moved on.
        val after = client.now()
        _state.update { if (it.snapshot?.sameContentAs(after) == true) it else it.copy(snapshot = after) }
      }
    } catch (e: HubException) {
      settle(e)
    }
  }

  private fun checkVersion(version: String) {
    if (Versions.compare(version, MIN_HUB_VERSION) < 0) throw HubException.TooOld(version, MIN_HUB_VERSION)
  }

  /** Records what a failed call says about the connection. Rethrows nothing. */
  private fun settle(e: HubException) {
    when (e) {
      is HubException.Unreachable ->
        _state.update {
          it.copy(connection = Connection.Offline(clock.instant(), lastReached, it.lastVia))
        }
      is HubException.Unpaired -> {
        // What the phone kept for this hub is cleared: cached asks, snoozes and remembered passwords. Unsent text stays to copy.
        _state.update { it.copy(connection = Connection.Unpaired, snapshot = null, snoozes = emptyMap()) }
        persistSnoozes()
        onUnpaired(hub.id)
      }
      is HubException.TooOld -> _state.update { it.copy(connection = Connection.TooOld(e.hubVersion, e.needs)) }
      is HubException.Refused, is HubException.NotAllowed, is HubException.Rejected, is HubException.ServerError -> Unit
    }
  }

  /** Folds a failure raised outside the repository into the connection state. */
  fun guard(e: HubException) = settle(e)

  /** Runs a hub call and folds connection failures into state. Returns null on failure. */
  suspend fun <T> guarded(block: suspend HubClient.() -> T): T? =
    try {
      client.block().also {
        if (_state.value.connection !is Connection.Online) _state.update { s -> s.copy(connection = Connection.Online) }
      }
    } catch (e: HubException) {
      settle(e)
      null
    }

  suspend fun answer(ask: Ask, text: String): SendResult = send(ask, DecisionKind.Answer, text = text) { client.answer(ask.id, text) }

  suspend fun decide(ask: Ask, approve: Boolean, note: String?): SendResult =
    send(ask, if (approve) DecisionKind.Approve else DecisionKind.Decline, note = note) { client.decide(ask.id, approve, note) }

  suspend fun admit(ask: Ask, approve: Boolean, name: String, grants: Set<String>): SendResult =
    send(ask, if (approve) DecisionKind.Admit else DecisionKind.TurnAway, name = name, grants = grants) {
      client.admit(ask.id, approve, name, grants)
    }

  /**
   * Puts a decision in the outbox without sending it now, for a caller that could not wait on the hub (a shade button
   * with seconds to live). The idempotency key makes the later send safe even if the hub did get the first.
   */
  suspend fun queueAnswer(ask: Ask, kind: DecisionKind, text: String? = null): SendResult =
    mutex.withLock {
      enqueue(QueuedDecision(ask.id, ask.agent, ask.sentence, kind, text, null, null, emptySet(), clock.instant().toEpochMilli()))
      SendResult.Queued
    }

  suspend fun resolveIdea(ask: Ask): SendResult = send(ask, DecisionKind.Resolve) { client.resolveIdea(ask.id) }

  suspend fun clearIdea(ask: Ask): SendResult = send(ask, DecisionKind.Clear) { client.clearIdea(ask.id) }

  /** A note is not an answer: it never waits in the outbox, and the idea stays where it is. */
  suspend fun replyIdea(ask: Ask, text: String): SendResult =
    mutex.withLock {
      try {
        client.replyIdea(ask.id, text)
        SendResult.Sent
      } catch (e: HubException.Unreachable) {
        settle(e)
        SendResult.Failed
      } catch (e: HubException) {
        settle(e)
        SendResult.Failed
      }
    }

  private suspend fun send(
    ask: Ask,
    kind: DecisionKind,
    text: String? = null,
    note: String? = null,
    name: String? = null,
    grants: Set<String> = emptySet(),
    call: suspend () -> Unit,
  ): SendResult =
    mutex.withLock {
      try {
        call()
        refreshLocked()
        SendResult.Sent
      } catch (e: HubException.Unreachable) {
        val q = QueuedDecision(ask.id, ask.agent, ask.sentence, kind, text, note, name, grants, clock.instant().toEpochMilli())
        enqueue(q)
        settle(e)
        SendResult.Queued
      } catch (e: HubException.NotAllowed) {
        SendResult.NotAllowed
      } catch (e: HubException.Rejected) {
        // The hub is reachable and said no: a retry would get the same answer, so nothing waits.
        SendResult.Rejected(e.status)
      } catch (e: HubException.ServerError) {
        SendResult.Failed
      } catch (e: HubException.Refused) {
        val q = QueuedDecision(ask.id, ask.agent, ask.sentence, kind, text, note, name, grants, clock.instant().toEpochMilli(), StoredRefusal.of(e.refusal))
        enqueue(q)
        SendResult.Refused(e.refusal)
      } catch (e: HubException) {
        settle(e)
        SendResult.Failed
      }
    }

  /** Sends what waited. True when at least one went through. */
  private suspend fun flushOutbox(): Boolean {
    var sent = false
    for (q in _state.value.outbox.filter { !it.refused }) {
      try {
        when (q.kind) {
          DecisionKind.Answer -> client.answer(q.askId, q.text.orEmpty())
          DecisionKind.Approve -> client.decide(q.askId, true, q.note)
          DecisionKind.Decline -> client.decide(q.askId, false, q.note)
          DecisionKind.Admit -> client.admit(q.askId, true, q.name.orEmpty(), q.grants)
          DecisionKind.TurnAway -> client.admit(q.askId, false, q.name.orEmpty(), q.grants)
          DecisionKind.Resolve -> client.resolveIdea(q.askId)
          DecisionKind.Clear -> client.clearIdea(q.askId)
        }
        remove(q.askId)
        sent = true
      } catch (e: HubException.Refused) {
        replace(q.copy(refusal = StoredRefusal.of(e.refusal)))
      } catch (e: HubException.Rejected) {
        // The hub will never take this one. It must not hold up what is behind it; the ask stays on Now to answer again.
        remove(q.askId)
      } catch (e: HubException.NotAllowed) {
        remove(q.askId)
      } catch (e: HubException.ServerError) {
        // Left for the next refresh, and the rest still go.
      } catch (e: HubException.Unreachable) {
        return sent
      } catch (e: HubException) {
        settle(e)
        return sent
      }
    }
    return sent
  }

  fun queuedFor(askId: String): QueuedDecision? = _state.value.outbox.firstOrNull { it.askId == askId }

  /** Cancels an answer that has not been sent, or discards one the hub refused. */
  fun dropQueued(askId: String) = remove(askId)

  private fun enqueue(q: QueuedDecision) = replace(q)

  private fun replace(q: QueuedDecision) {
    _state.update { s -> s.copy(outbox = s.outbox.filter { it.askId != q.askId } + q) }
    persistOutbox()
  }

  private fun remove(askId: String) {
    _state.update { s -> s.copy(outbox = s.outbox.filter { it.askId != askId }) }
    persistOutbox()
  }

  /** Snoozes an ask on this phone only. */
  fun snooze(askId: String, duration: Duration = SNOOZE) {
    _state.update { it.copy(snoozes = it.snoozes + (askId to clock.instant().plus(duration))) }
    persistSnoozes()
  }

  fun unsnooze(askId: String) {
    _state.update { it.copy(snoozes = it.snoozes - askId) }
    persistSnoozes()
  }

  /** "Mark seen": the Since you looked list starts from now. */
  suspend fun markSeen() {
    val at = clock.instant()
    store.put(seenKey, at.toEpochMilli().toString())
    _state.update { it.copy(seenAt = at) }
    guarded { markSeen() }
  }

  /** The ask as the hub has it now, for a notification that is opened after the ask was decided. */
  suspend fun askById(id: String): Ask? =
    _state.value.snapshot?.asks?.firstOrNull { it.id == id } ?: guarded { ask(id) }

  /** The ids of asks that wait on the hub and are not decided. */
  fun waitingIds(): Set<String> =
    _state.value.snapshot?.asks?.filter { it.status == AskStatus.Waiting }?.map { it.id }?.toSet().orEmpty()

  private fun loadOutbox(): List<QueuedDecision> =
    store.get(outboxKey)?.let { runCatching { StoreJson.decodeFromString(ListSerializer(QueuedDecision.serializer()), it) }.getOrNull() }.orEmpty()

  private fun persistOutbox() = store.put(outboxKey, StoreJson.encodeToString(ListSerializer(QueuedDecision.serializer()), _state.value.outbox))

  private fun loadSnoozes(): Map<String, Instant> =
    store.get(snoozeKey)?.let { runCatching { StoreJson.decodeFromString(Snoozes.serializer(), it) }.getOrNull() }
      ?.untilEpochMilli?.mapValues { Instant.ofEpochMilli(it.value) }.orEmpty()

  private fun persistSnoozes() =
    store.put(snoozeKey, StoreJson.encodeToString(Snoozes.serializer(), Snoozes(_state.value.snoozes.mapValues { it.value.toEpochMilli() })))

  companion object {
    const val MIN_HUB_VERSION = "1.13.0"
    val SNOOZE: Duration = Duration.ofHours(1)
  }
}

sealed interface SendResult {
  data object Sent : SendResult

  data object Queued : SendResult

  data class Refused(val refusal: Refusal) : SendResult

  data object Failed : SendResult

  /** The hub does not let a phone do this one: it is for the web app. Nothing waits to be sent. */
  data object NotAllowed : SendResult

  /** The hub turned the call down with a 4xx. It is not queued: the same call would get the same answer. */
  data class Rejected(val status: Int) : SendResult
}

object Versions {
  /** Compares dotted numeric versions: 1.13.0 against 1.9.2. Anything that is not a number counts as 0. */
  fun compare(a: String, b: String): Int {
    val x = a.split('.', '-').map { it.toIntOrNull() ?: 0 }
    val y = b.split('.', '-').map { it.toIntOrNull() ?: 0 }
    for (i in 0 until maxOf(x.size, y.size)) {
      val c = x.getOrElse(i) { 0 }.compareTo(y.getOrElse(i) { 0 })
      if (c != 0) return c
    }
    return 0
  }
}
