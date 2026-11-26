package dev.agync.agenthub.alerts

import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.HubState
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.EventKind
import dev.agync.agenthub.model.FeedEvent
import java.time.Instant

/** What to do to the shade after a refetch: post the new, cancel what no longer waits. */
data class AlertPlan(val post: List<Ask>, val cancel: Set<String>, val finished: List<FeedEvent>) {
  val isEmpty: Boolean get() = post.isEmpty() && cancel.isEmpty() && finished.isEmpty()
}

object AlertPlanner {
  /**
   * The asks that should be in the shade right now: waiting on the hub, not snoozed on this
   * phone, not already answered here and waiting to send, and of a kind the person asked to hear.
   */
  fun active(state: HubState, now: Instant, prefs: DevicePrefs): List<Ask> {
    val queued = state.outbox.filter { !it.refused }.map { it.askId }.toSet()
    return state.snapshot?.asks.orEmpty().filter { ask ->
      ask.waiting &&
        ask.id !in queued &&
        state.snoozes[ask.id]?.isAfter(now) != true &&
        when (ask.kind) {
          AskKind.Approval, AskKind.Question -> prefs.alertApprovals
          AskKind.Joining -> prefs.alertJoining
          // The person captured it themselves; it waits in Now and never alerts.
          AskKind.Idea -> false
        }
    }
  }

  /**
   * [notified] is what the shade already holds. [finishedSeen] is the finished-work events
   * already announced, or null on the first fetch, when they are taken as known and not posted.
   */
  fun plan(notified: Set<String>, state: HubState, now: Instant, prefs: DevicePrefs, finishedSeen: Set<String>?): AlertPlan {
    val active = active(state, now, prefs)
    val activeIds = active.map { it.id }.toSet()
    val finished =
      if (finishedSeen == null || !prefs.alertFinished) emptyList()
      else state.snapshot?.recent.orEmpty().filter { it.kind == EventKind.Published && it.id !in finishedSeen }
    return AlertPlan(
      post = active.filter { it.id !in notified }.sortedBy { it.askedAt },
      cancel = notified - activeIds,
      finished = finished,
    )
  }
}
