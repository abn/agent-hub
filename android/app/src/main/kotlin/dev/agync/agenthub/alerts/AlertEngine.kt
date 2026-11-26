package dev.agync.agenthub.alerts

import dev.agync.agenthub.data.Connection
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.FeedEvent
import java.time.Clock

/** The shade, behind an interface so the refetch logic is tested without Android. */
interface AlertPoster {
  fun post(hubId: String, hubName: String, ask: Ask)

  fun cancel(hubId: String, askId: String)

  fun postFinished(hubId: String, hubName: String, event: FeedEvent)

  /** The quiet ongoing notification of the held session. */
  fun connection(hubName: String, status: String)
}

/**
 * Holds one hub's session open for alerts. On every tick and on every reconnect it refetches
 * Now, posts a notification for each new ask and cancels the notification of each ask that no
 * longer waits. A notification that is opened after its ask was decided lands on the resolved
 * state, so a missed cancel is harmless.
 */
class AlertEngine(
  private val repo: HubRepository,
  private val poster: AlertPoster,
  private val prefs: () -> DevicePrefs,
  private val clock: Clock = Clock.systemDefaultZone(),
) {
  private val notified = mutableSetOf<String>()
  private var finishedSeen: MutableSet<String>? = null

  /** Collects the hub's ticks until cancelled. Each tick, and the first one on connect, refetches. */
  suspend fun run() {
    repo.client.ticks.collect { onTick() }
  }

  suspend fun onTick() {
    repo.refresh()
    apply()
  }

  /** Re-plans from the current state without a fetch: after a snooze, a local answer, a pref change. */
  @Synchronized
  fun apply() {
    val state = repo.state.value
    val p = prefs()
    val plan = AlertPlanner.plan(notified, state, clock.instant(), p, finishedSeen)
    for (id in plan.cancel) {
      poster.cancel(repo.hub.id, id)
      notified.remove(id)
    }
    for (ask in plan.post) {
      poster.post(repo.hub.id, repo.hub.name, ask)
      notified.add(ask.id)
    }
    val seen = finishedSeen
    if (seen == null) {
      finishedSeen = state.snapshot?.recent.orEmpty().map { it.id }.toMutableSet().takeIf { state.snapshot != null }
    } else {
      for (e in plan.finished) {
        poster.postFinished(repo.hub.id, repo.hub.name, e)
        seen.add(e.id)
      }
    }
    poster.connection(repo.hub.name, statusLine(state.connection, state.lastVia))
  }

  /** Posts every active ask again, so buttons built under an old setting are replaced. */
  @Synchronized
  fun repostAll() {
    val state = repo.state.value
    for (ask in AlertPlanner.active(state, clock.instant(), prefs())) poster.post(repo.hub.id, repo.hub.name, ask)
  }

  /** Forget what the shade holds, after the person clears it or the phone is unpaired. */
  @Synchronized
  fun reset() {
    notified.clear()
  }

  companion object {
    fun statusLine(c: Connection, via: String?): String =
      when (c) {
        Connection.Online -> via?.let { "via $it" } ?: "Connected"
        Connection.Connecting -> "Connecting"
        is Connection.Offline -> "Cannot reach the hub. Trying again."
        Connection.Unpaired -> "This phone was unpaired"
        is Connection.TooOld -> "The hub needs an update"
      }
  }
}
