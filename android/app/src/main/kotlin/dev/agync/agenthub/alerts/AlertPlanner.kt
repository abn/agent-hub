package dev.agync.agenthub.alerts

import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.HubState
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.EventKind
import dev.agync.agenthub.model.FeedEvent
import java.time.Instant

/** The one notification that stands for a hub's asks once there are many: [count] of them, under [groupKey]. */
data class AskSummary(val groupKey: String, val count: Int, val kinds: Set<AskKind> = emptySet())

/**
 * What to do to the shade after a refetch: post the new, cancel what no longer waits. [groupKey] is set while the asks
 * are shown as one group, and every ask in [post] then carries it. [summary] is the group's notification to post or
 * update; [dropSummary] takes it down when the group dissolves.
 */
data class AlertPlan(
  val post: List<Ask>,
  val cancel: Set<String>,
  val finished: List<FeedEvent>,
  val groupKey: String? = null,
  val summary: AskSummary? = null,
  val dropSummary: Boolean = false,
) {
  val isEmpty: Boolean get() = post.isEmpty() && cancel.isEmpty() && finished.isEmpty() && summary == null && !dropSummary
}

object AlertPlanner {
  /** More asks than this showing at once go into one group. */
  const val GROUP_OVER = 3

  fun groupKey(hubId: String) = "dev.agync.agenthub.asks.$hubId"

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
   * [summarized] is the count the group's summary shows now, or null when the shade holds no group.
   */
  fun plan(notified: Set<String>, state: HubState, now: Instant, prefs: DevicePrefs, finishedSeen: Set<String>?, summarized: Int? = null): AlertPlan {
    val active = active(state, now, prefs)
    val activeIds = active.map { it.id }.toSet()
    // Once grouped the asks stay grouped until none is left: moving a child out of a group and cancelling its summary
    // can cancel or alert the children again.
    val key = groupKey(state.hub.id).takeIf { active.size > GROUP_OVER || (summarized != null && active.isNotEmpty()) }
    // Forming the group changes every ask's notification, so all of them go up again.
    val regroup = key != null && summarized == null
    val finished =
      if (finishedSeen == null || !prefs.alertFinished) emptyList()
      else state.snapshot?.recent.orEmpty().filter { it.kind == EventKind.Published && it.id !in finishedSeen }
    return AlertPlan(
      post = active.filter { regroup || it.id !in notified }.sortedBy { it.askedAt },
      cancel = notified - activeIds,
      finished = finished,
      groupKey = key,
      summary = key?.takeIf { summarized != active.size }?.let { AskSummary(it, active.size, active.map { a -> a.kind }.toSet()) },
      dropSummary = key == null && summarized != null,
    )
  }
}
