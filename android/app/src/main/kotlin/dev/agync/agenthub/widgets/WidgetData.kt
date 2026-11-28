package dev.agync.agenthub.widgets

import dev.agync.agenthub.data.Connection
import dev.agync.agenthub.data.HubState
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.NowBuilder
import dev.agync.agenthub.model.SessionState
import java.time.Instant

data class LiveLine(val name: String, val subagents: Int)

/** What the launcher widgets show: counts and a few rows, read from the cache, never from a decision. */
data class WidgetData(
  val hubId: String?,
  val hubName: String,
  val syncedAt: String,
  /** "Not paired", "Cannot reach", or null when the cache is current. */
  val note: String?,
  val needsYou: List<Ask>,
  val canWait: Int,
  val live: List<LiveLine>,
) {
  val needsCount: Int get() = needsYou.size
  val liveCount: Int get() = live.size

  companion object {
    const val ROWS = 3

    fun empty(note: String) = WidgetData(null, "Agent Hub", "", note, emptyList(), 0, emptyList())

    fun from(hubId: String, hubName: String, state: HubState, now: Instant, fmt: Fmt, syncedAt: Instant? = null): WidgetData {
      val m = NowBuilder.build(state, now, fmt)
      val all = state.snapshot?.live.orEmpty().filter { it.state == SessionState.Live }
      val tops = all.filter { it.parent == null }
      val lines = tops.map { t -> LiveLine(t.name, all.count { it.parent == t.name }) }
      val note =
        when (state.connection) {
          Connection.Unpaired -> "This phone was unpaired"
          is Connection.Offline -> if (state.snapshot == null) "Cannot reach $hubName" else null
          else -> null
        }
      return WidgetData(hubId, hubName, (syncedAt ?: state.snapshot?.asOf)?.let(fmt::time).orEmpty(), note, m.needsYou, m.canWait.size, lines)
    }
  }
}

/** The mark that opens a row: "!" for a decision, "?" for a question, empty for a machine (drawn as a word). */
fun Ask.mark(): String =
  when (kind) {
    AskKind.Approval -> "!"
    AskKind.Question -> "?"
    AskKind.Joining -> "+"
    AskKind.Idea -> "*"
  }
