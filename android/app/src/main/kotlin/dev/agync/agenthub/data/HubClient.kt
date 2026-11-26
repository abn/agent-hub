package dev.agync.agenthub.data

import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.ArtifactSummary
import dev.agync.agenthub.model.ArtifactView
import dev.agync.agenthub.model.BrainEntry
import dev.agync.agenthub.model.FeedEvent
import dev.agync.agenthub.model.HubInfo
import dev.agync.agenthub.model.Member
import dev.agync.agenthub.model.SearchResults
import dev.agync.agenthub.model.ProjectSummary
import dev.agync.agenthub.model.SessionSummary
import dev.agync.agenthub.model.Storage
import java.time.Instant
import kotlinx.coroutines.flow.Flow

/** Why the hub turned an answer away. The hub tells the two apart; the phone says which. */
sealed interface Refusal {
  /** Someone else decided first. [answer] is what was decided, when the hub says. */
  data class AnsweredElsewhere(val at: Instant, val answer: String?) : Refusal

  /** The ask ran out at its deadline with no answer. */
  data class ClosedAtDeadline(val at: Instant) : Refusal
}

sealed class HubException(message: String) : Exception(message) {
  /** No route answered. Tendril decides routes; this is what it reports when none did. */
  class Unreachable(message: String = "unreachable") : HubException(message)

  /** The hub no longer accepts this phone: it was revoked. An immediate refusal, not a timeout. */
  class Unpaired : HubException("unpaired")

  /** The hub is older than this app can read. */
  class TooOld(val hubVersion: String, val needs: String) : HubException("hub $hubVersion is older than $needs")

  /** The hub refuses this call to a phone on purpose (operator_only). Not an error state: the control should not be shown. */
  class NotAllowed(val route: String) : HubException("not for a phone: $route")

  /** The hub understood the call and turned it down (a 4xx). The hub is fine and the call is not: it is never queued or retried. */
  class Rejected(val status: Int, val detail: String? = null) : HubException("rejected $status")

  /** The hub answered with a server error (5xx): not a transport failure, and the hub may have acted. */
  class ServerError(val status: Int) : HubException("hub error $status")

  /** The ask was closed before this decision arrived. */
  class Refused(val refusal: Refusal) : HubException("refused")
}

/** What a prune can be taken back with, until [expiresAt]. One token per session the hub removed. */
data class PruneUndo(val tokens: List<String>, val expiresAt: Instant?)

/** Everything Now shows, as the hub had it at [asOf]. */
data class NowSnapshot(
  val info: HubInfo,
  val asOf: Instant,
  val asks: List<Ask>,
  val recent: List<FeedEvent>,
  val live: List<SessionSummary>,
  val answeredToday: List<FeedEvent>,
)

/**
 * The hub as the app sees it. The real implementation speaks over a Tendril session; the app
 * never picks a route and never holds a token. Every call may throw a [HubException].
 */
interface HubClient {
  /** A freshness tick from the held session, and one on every reconnect. */
  val ticks: Flow<Unit>

  suspend fun info(): HubInfo

  suspend fun now(): NowSnapshot

  suspend fun ask(id: String): Ask

  suspend fun answer(askId: String, text: String)

  suspend fun decide(askId: String, approve: Boolean, note: String?)

  suspend fun admit(askId: String, approve: Boolean, name: String, grants: Set<String>)

  /** An idea acted on: it leaves Needs you. Both are safe to repeat. */
  suspend fun resolveIdea(id: String)

  suspend fun clearIdea(id: String)

  /** A note on the idea's thread. It does not close the idea. */
  suspend fun replyIdea(id: String, text: String)

  suspend fun projects(): List<ProjectSummary>

  suspend fun feed(project: String): List<FeedEvent>

  suspend fun sessions(project: String): List<SessionSummary>

  suspend fun session(id: String): SessionSummary

  suspend fun endSession(id: String)

  suspend fun artifacts(): List<ArtifactSummary>

  suspend fun artifact(id: String, version: Int? = null): ArtifactView

  suspend fun members(): List<Member>

  /** One entry of a session's brain, or null when the hub has none there or it is not text. */
  suspend fun brainEntry(session: String, path: String): BrainEntry?

  /** Full-text search over every feed, artifact, brain and knowledge base the phone may read. */
  suspend fun search(query: String): SearchResults

  suspend fun storage(): Storage

  /** Prunes the ended sessions. The hub keeps an undo for a short while: the tokens that bring them back. */
  suspend fun pruneEnded(): PruneUndo

  /** Prunes one ended session, with the same undo. */
  suspend fun pruneSession(id: String): PruneUndo

  suspend fun undoPrune(undo: PruneUndo)

  suspend fun captureIdea(project: String, text: String, link: String?)

  /** Marks the project's feed as seen up to now. */
  suspend fun markSeen()

  /** Moves the hub's cursor for a project's feed up to [eventId]. It never moves back. */
  suspend fun markFeedSeen(project: String, eventId: String)
}
