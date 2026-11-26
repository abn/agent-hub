package dev.agync.agenthub.model

import java.time.Instant

/** An [Idea] is one the person captured: it waits in Needs you until it is resolved or cleared. */
enum class AskKind { Approval, Question, Joining, Idea }

/** Where an ask stands on the hub. Everything but [Waiting] is decided and needs nothing. */
enum class AskStatus { Waiting, Answered, Approved, Declined, Admitted, TurnedAway, ClosedAtDeadline, Resolved, Cleared }

/** The receipts beside an ask: the things it points at. */
data class Receipts(
  val session: String? = null,
  val sessionEvents: Int? = null,
  val brainKey: String? = null,
  val artifact: String? = null,
  val artifactVersion: Int? = null,
  val eventId: String? = null,
  /** The session's name, which is what a person reads; [session] is what the hub is asked for. */
  val sessionName: String? = null,
)

/** The families the hub's search finds things in. */
enum class SearchKind(val label: String) { Feed("Feed"), Artifact("Artifacts"), Brain("Brain"), Knowledge("Knowledge") }

/** One hit. [ref] is the id of what was found: an event, an artifact, a brain entry's session or a project. */
data class SearchHit(
  val kind: SearchKind,
  val ref: String,
  val project: String?,
  val title: String,
  val snippet: String,
  val at: Instant?,
  val session: String? = null,
  val sessionName: String? = null,
  val by: String? = null,
)

data class SearchResults(val hits: List<SearchHit>, val truncated: Boolean) {
  fun count(kind: SearchKind?) = if (kind == null) hits.size else hits.count { it.kind == kind }
}

/** One entry of a session's brain, as the hub reads it back. [path] is namespaced: `/kv/...` or `/fs/...`. */
data class BrainEntry(val path: String, val kind: String, val sizeBytes: Long, val content: String)

/** Where a receipt's brain key points. The hub keeps keys under `/kv/` and files under `/fs/`. */
object BrainPath {
  fun of(key: String): String {
    val k = key.trim()
    return when {
      k.startsWith("/kv/") || k.startsWith("/fs/") -> k
      k.startsWith("kv/") || k.startsWith("fs/") -> "/$k"
      else -> "/kv/" + k.trimStart('/')
    }
  }
}

/** What a machine asked when it wanted to join. Every field but the name may be missing. */
data class Enrolment(
  val machine: String,
  /** The pending member the enrolment decides, which the enrol routes take as their id. */
  val memberId: String? = null,
  val firstAgent: String? = null,
  val system: String? = null,
  val from: String? = null,
  val reason: String? = null,
  /** The projects the machine asked for. Approving may grant some of these and nothing else. */
  val requestedProjects: List<String> = emptyList(),
)

data class Ask(
  val id: String,
  val kind: AskKind,
  /** The session or agent that asks. For a joining ask, the machine. */
  val agent: String,
  val sentence: String,
  /** A question's detail, or the approval prose exactly as the agent wrote it. */
  val prose: String? = null,
  val project: String? = null,
  val tool: String? = null,
  val machine: String? = null,
  val askedAt: Instant,
  val deadline: Instant? = null,
  /** What the hub does if nobody answers, in the hub's words: "Declined at 18:00". */
  val onSilence: String? = null,
  val answers: List<String> = emptyList(),
  val receipts: Receipts = Receipts(),
  val enrolment: Enrolment? = null,
  val status: AskStatus = AskStatus.Waiting,
  val decidedAt: Instant? = null,
  val decidedWith: String? = null,
  val related: List<RelatedAsk> = emptyList(),
) {
  val waiting: Boolean get() = status == AskStatus.Waiting
}

data class RelatedAsk(val agent: String, val sentence: String, val outcome: String, val whenLabel: String)

enum class EventKind { Asked, Published, Noted, Started, Finished, Captured, Answered, Approved }

data class FeedEvent(
  val id: String,
  val kind: EventKind,
  val who: String,
  val text: String,
  val tool: String? = null,
  val machine: String? = null,
  val at: Instant,
  val project: String? = null,
  val waitingOnYou: Boolean = false,
  val askId: String? = null,
)

enum class SessionState { Live, Idle, Ended }

data class Participant(val tool: String, val machine: String, val lastWrite: Instant? = null)

data class SessionSummary(
  val id: String,
  val name: String,
  val project: String,
  val parent: String? = null,
  val state: SessionState,
  val events: Int,
  val bytes: Long,
  val participants: List<Participant>,
  val lastActivity: Instant,
  val startedAt: Instant? = null,
  val anchor: Anchor? = null,
  val brainEntries: Int = 0,
)

data class Anchor(val phase: String, val by: String, val at: Instant, val summary: String, val steps: List<String>)

data class ProjectSummary(
  val name: String,
  val confidential: Boolean,
  val live: Int,
  val idle: Int,
  val bytes: Long,
  val lastActivity: Instant?,
  val sessionCount: Int,
  val artifactCount: Int,
  /** What the hub calls it in routes. Defaults to the name. */
  val id: String = name,
)

data class ArtifactSummary(
  val id: String,
  val name: String,
  val project: String,
  val tool: String,
  val version: Int,
  val bytes: Long,
  val at: Instant,
  val comments: Int = 0,
  val live: Boolean = false,
  val protected: Boolean = false,
)

data class ArtifactComment(val id: String, val by: String, val at: Instant, val text: String, val anchorText: String?, val resolved: Boolean = false)

/** What the viewer needs: the hub's frame route for the page, and the version facts for the bar. */
data class ArtifactView(
  val summary: ArtifactSummary,
  val shownVersion: Int,
  val latestVersion: Int,
  val latestAt: Instant,
  val html: String,
  val comments: List<ArtifactComment>,
  /** Set for a protected artifact: the hub holds only this, and the phone opens it with the project password. */
  val sealed: SealedArtifact? = null,
)

data class SealedArtifact(val envelope: dev.agync.agenthub.crypto.Envelope, val ciphertext: String)

data class Member(
  val name: String,
  val pending: Boolean = false,
  val reason: String? = null,
  val tools: List<String> = emptyList(),
  val live: Int = 0,
  val idle: Int = 0,
  val firstSeen: Instant? = null,
  val lastUsed: Instant? = null,
  val agentsSeen: List<AgentSeen> = emptyList(),
  val grants: List<Grant> = emptyList(),
  val tokenLive: Boolean = true,
)

data class AgentSeen(val tool: String, val sessions: String, val last: Instant?)

data class Grant(val project: String, val since: Instant)

data class StorageSegment(val label: String, val bytes: Long, val tone: Int)

data class EndedSession(val name: String, val project: String, val bytes: Long)

data class Storage(val totalBytes: Long, val segments: List<StorageSegment>, val ended: List<EndedSession>) {
  val endedBytes: Long get() = ended.sumOf { it.bytes }
}

data class PhoneDevice(val name: String, val thisPhone: Boolean)

data class HubInfo(
  val name: String,
  val version: String,
  /** How Tendril says this session is connected ("LAN", "Tailnet", "Relay"), or null when it does not say. */
  val via: String?,
  val phones: List<PhoneDevice> = emptyList(),
  /** The hub's web address, when the hub says it: where "Open web app" goes. Null when it does not. */
  val webUrl: String? = null,
)

/** A hub's web address, kept only if it is one a browser may be sent to: http or https, with a host and no credentials. */
object WebAddress {
  fun parse(raw: String?): String? {
    val uri = try { java.net.URI(raw?.trim().orEmpty()) } catch (_: java.net.URISyntaxException) { return null }
    if (uri.scheme?.lowercase() !in setOf("http", "https") || uri.host.isNullOrEmpty() || uri.userInfo != null) return null
    return uri.toString().trimEnd('/')
  }
}
