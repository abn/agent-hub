package dev.agync.agenthub.alerts

import dev.agync.agenthub.data.Connection
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.FeedEvent
import java.time.Clock
import kotlinx.coroutines.flow.StateFlow

/** The shade, behind an interface so the refetch logic is tested without Android. */
interface AlertPoster {
  /** [groupKey] is set when the ask is one of a group, which [summary] stands for. */
  fun post(hubId: String, hubName: String, ask: Ask, groupKey: String?)

  fun cancel(hubId: String, askId: String)

  /** The notification that heads a group of asks, posted again whenever its count changes. */
  fun summary(hubId: String, hubName: String, summary: AskSummary): Boolean

  fun cancelSummary(hubId: String)

  fun postFinished(hubId: String, hubName: String, event: FeedEvent)

  /** The quiet ongoing notification of the held session. False when it could not be posted, so the caller tries again. */
  fun connection(hubName: String, status: String): Boolean
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
  private val foreground: StateFlow<Boolean> = ForegroundTracker.app.state,
) {
  private val notified = mutableSetOf<String>()
  private var finishedSeen: MutableSet<String>? = null
  private var summarized: Int? = null
  private var connectionShown: Pair<String, String>? = null

  /**
   * Refetches on the hub's ticks, gathered into windows (see [coalesced]), until cancelled. The stream sends a tick
   * when it opens, so the first refetch is at once.
   */
  suspend fun run() {
    repo.client.ticks.coalesced(foreground).collect { onTick() }
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
    val plan = AlertPlanner.plan(notified, state, clock.instant(), p, finishedSeen, summarized)
    for (id in plan.cancel) {
      poster.cancel(repo.hub.id, id)
      notified.remove(id)
    }
    for (ask in plan.post) {
      poster.post(repo.hub.id, repo.hub.name, ask, plan.groupKey)
      notified.add(ask.id)
    }
    // A summary the system refused is not remembered, so the next apply posts it.
    plan.summary?.let { if (poster.summary(repo.hub.id, repo.hub.name, it)) summarized = it.count }
    if (plan.dropSummary) {
      poster.cancelSummary(repo.hub.id)
      summarized = null
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
    // An ongoing notification posted again with the same words still wakes the system, so it is posted when they change.
    val line = repo.hub.name to statusLine(state.connection, state.lastVia)
    // A post the system refused (no permission yet) is not remembered, so the next apply tries it again.
    if (line != connectionShown) connectionShown = line.takeIf { poster.connection(line.first, line.second) }
  }

  /** Posts every active ask again, so buttons built under an old setting are replaced. */
  @Synchronized
  fun repostAll() {
    val state = repo.state.value
    val key = AlertPlanner.groupKey(repo.hub.id).takeIf { summarized != null }
    for (ask in AlertPlanner.active(state, clock.instant(), prefs())) poster.post(repo.hub.id, repo.hub.name, ask, key)
  }

  /** Forget what the shade holds, after the person clears it or the phone is unpaired. */
  @Synchronized
  fun reset() {
    notified.clear()
    summarized = null
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
