package dev.agync.agenthub.model

import dev.agync.agenthub.data.HubState
import dev.agync.agenthub.data.QueuedDecision
import java.time.Instant

enum class NowFilter { All, NeedsYou, Since, WaitingToSend }

/** A run of text in the lede: the agent names are bold, the rest line is muted. */
data class Span(val text: String, val bold: Boolean = false, val muted: Boolean = false)

data class QueuedRow(val ask: Ask?, val queued: QueuedDecision)

/** What the Now screen shows, derived from the hub's snapshot, this phone's snoozes and its outbox. */
data class NowModel(
  val needsYou: List<Ask>,
  val canWait: List<Ask>,
  val waitingToSend: List<QueuedRow>,
  val since: List<FeedEvent>,
  val live: List<SessionSummary>,
  val answeredToday: List<FeedEvent>,
  val lede: List<Span>,
) {
  val badge: Int get() = needsYou.size
  val allCount: Int get() = needsYou.size + canWait.size + since.size
}

object NowBuilder {
  fun build(state: HubState, now: Instant, fmt: Fmt): NowModel {
    val snap = state.snapshot
    val queuedIds = state.outbox.filter { !it.refused }.map { it.askId }.toSet()
    val waiting = snap?.asks.orEmpty().filter { it.waiting }.sortedByDescending { it.askedAt }
    val snoozed = { a: Ask -> state.snoozes[a.id]?.isAfter(now) == true }
    val needs = waiting.filter { it.id !in queuedIds && !snoozed(it) }
    val wait = waiting.filter { it.id !in queuedIds && snoozed(it) }
    val toSend =
      state.outbox.filter { !it.refused }.sortedByDescending { it.queuedAtEpochMilli }
        .map { q -> QueuedRow(snap?.asks?.firstOrNull { it.id == q.askId }, q) }
    val seen = state.seenAt
    val since = snap?.recent.orEmpty().filter { seen == null || it.at.isAfter(seen) }.sortedByDescending { it.at }
    val live = snap?.live.orEmpty().filter { it.state == SessionState.Live }
    val answered = snap?.answeredToday.orEmpty().sortedByDescending { it.at }
    return NowModel(needs, wait, toSend, since, live, answered, lede(needs, wait, live, answered, fmt))
  }

  private fun lede(needs: List<Ask>, wait: List<Ask>, live: List<SessionSummary>, answered: List<FeedEvent>, fmt: Fmt): List<Span> {
    val out = mutableListOf<Span>()
    if (needs.isEmpty()) {
      out += Span("Nothing waits on you.")
      val rest = buildString {
        val working = live.count { it.parent == null }
        if (working > 0) {
          append(if (working == 1) "One session is working" else "${AskText.word(working)} sessions are working")
        }
        answered.firstOrNull()?.let {
          if (isNotEmpty()) append(", and the last ask was answered at ") else append("The last ask was answered at ")
          append(fmt.time(it.at))
        }
        if (isNotEmpty()) append('.')
      }
      if (rest.isNotEmpty()) out += Span(" $rest", muted = true)
      if (wait.isNotEmpty()) out += Span(" ${AskText.word(wait.size)} can wait.", muted = true)
      return out
    }
    val n = needs.size
    out += Span(if (n == 1) "One agent needs you. " else "${AskText.word(n)} agents need you. ")
    val first = needs[0]
    out += Span(first.agent, bold = true)
    out += Span(" ${AskText.ledeTail(first)}")
    if (n >= 2) {
      val second = needs[1]
      out += Span(", and ")
      out += Span(second.agent, bold = true)
      out += Span(" ${AskText.ledeTail(second)}")
    }
    if (n >= 3) out += Span(", and ${AskText.word(n - 2).lowercase()} more")
    out += Span(".")
    if (wait.isNotEmpty()) out += Span(" ${AskText.word(wait.size)} can wait.", muted = true)
    return out
  }

  fun filterLabel(f: NowFilter) =
    when (f) {
      NowFilter.All -> "All"
      NowFilter.NeedsYou -> "Needs you"
      NowFilter.Since -> "Since you looked"
      NowFilter.WaitingToSend -> "Waiting to send"
    }

  /** The verbs an ask's row offers: the whole set, on every row. */
  fun verbs(ask: Ask): List<String> =
    when (ask.kind) {
      AskKind.Approval -> listOf("Approve", "Decline")
      AskKind.Question -> ask.answers
      AskKind.Joining -> listOf("Review")
      AskKind.Idea -> listOf("Resolve", "Clear")
    }

}
