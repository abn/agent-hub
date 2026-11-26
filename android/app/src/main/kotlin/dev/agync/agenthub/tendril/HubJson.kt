package dev.agync.agenthub.tendril

import dev.agync.agenthub.model.AgentSeen
import dev.agync.agenthub.model.ArtifactComment
import dev.agync.agenthub.model.ArtifactSummary
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.AskStatus
import dev.agync.agenthub.model.Enrolment
import dev.agync.agenthub.model.EndedSession
import dev.agync.agenthub.model.EventKind
import dev.agync.agenthub.model.FeedEvent
import dev.agync.agenthub.model.Member
import dev.agync.agenthub.model.Participant
import dev.agync.agenthub.model.ProjectSummary
import dev.agync.agenthub.model.Receipts
import dev.agync.agenthub.model.SearchHit
import dev.agync.agenthub.model.SearchKind
import dev.agync.agenthub.model.SearchResults
import dev.agync.agenthub.model.SessionState
import dev.agync.agenthub.model.SessionSummary
import dev.agync.agenthub.model.Storage
import dev.agync.agenthub.model.StorageSegment
import java.time.Instant
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull

internal fun JsonElement?.obj(): JsonObject? = this as? JsonObject

internal fun JsonObject.str(k: String): String? = (this[k] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }

internal fun JsonObject.long(k: String): Long? = (this[k] as? JsonPrimitive)?.longOrNull

internal fun JsonObject.bool(k: String): Boolean? = (this[k] as? JsonPrimitive)?.booleanOrNull

internal fun JsonObject.time(k: String): Instant? = str(k)?.let { runCatching { Instant.parse(it) }.getOrNull() }

internal fun JsonObject.sub(k: String): JsonObject? = this[k] as? JsonObject

/** The objects of a list the hub may send bare or under one of [keys]. */
internal fun JsonElement.rows(vararg keys: String): List<JsonObject> {
  val arr: JsonArray? = this as? JsonArray ?: keys.firstNotNullOfOrNull { (this as? JsonObject)?.get(it) as? JsonArray }
  return arr.orEmpty().mapNotNull { it as? JsonObject }
}

private fun JsonObject.strings(k: String): List<String> =
  (this[k] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.contentOrNull?.trim()?.takeIf(String::isNotEmpty) }

/**
 * Maps what the hub sends onto the app's models, as the real handlers shape it (checked against a
 * running hub). A field the hub leaves out keeps the model's own default.
 */
internal object HubJson {
  /** An inbox item. [agent] is the asking session's name, which the item carries only as an id. */
  fun ask(item: JsonObject, now: Instant, agent: String? = null): Ask? {
    val payload = item.sub("payload")
    val isEnrol = payload?.str("action") == "enrol_request"
    val kind =
      when (item.str("kind")) {
        "approval" -> if (isEnrol) AskKind.Joining else AskKind.Approval
        "question" -> AskKind.Question
        "idea" -> AskKind.Idea
        else -> return null
      }
    val id = item.str("event_id") ?: return null
    val author = item.sub("author")
    val waiting = item.str("status") == "action"
    val decision = item.sub("decision")
    val answer = item.sub("answer")
    val outcome =
      when {
        waiting -> AskStatus.Waiting
        kind == AskKind.Idea -> if (item.str("status") == "cleared") AskStatus.Cleared else AskStatus.Resolved
        decision?.bool("expired") == true || answer?.bool("expired") == true -> AskStatus.ClosedAtDeadline
        decision?.str("decision")?.startsWith("approve") == true -> if (isEnrol) AskStatus.Admitted else AskStatus.Approved
        decision != null -> if (isEnrol) AskStatus.TurnedAway else AskStatus.Declined
        else -> AskStatus.Answered
      }
    val expiry = item.time("expires_at")
    // An idea is the person's own, captured on the web app or on this phone.
    val who = if (kind == AskKind.Idea) "You" else agent ?: author?.str("agent") ?: item.str("actor") ?: "an agent"
    val machineName = payload?.str("display_name") ?: payload?.str("member") ?: ""
    return Ask(
      id = id,
      kind = kind,
      agent = if (isEnrol) machineName.ifEmpty { who } else who,
      sentence = item.str("summary") ?: "",
      prose = if (kind == AskKind.Idea) payload?.str("url") ?: payload?.str("body") else payload?.str("body"),
      project = item.str("project_display_name") ?: item.str("project_id"),
      tool = author?.str("agent"),
      machine = author?.str("member"),
      askedAt = item.time("created_at") ?: now,
      deadline = expiry,
      onSilence = if (expiry != null) (if (item.str("on_expiry") == "approve") "Approved" else "Declined") + " at its deadline" else null,
      answers = payload?.strings("options").orEmpty(),
      receipts = Receipts(session = item.str("session_id"), sessionName = if (item.str("session_id") != null) agent else null, brainKey = payload?.str("brain_key"), artifact = payload?.str("artifact_id"), eventId = id.takeLast(6).uppercase()),
      enrolment = if (isEnrol) Enrolment(machine = machineName, memberId = payload.str("member"), firstAgent = author?.str("agent"), from = payload.str("source"), reason = payload.str("why"), requestedProjects = payload.strings("projects")) else null,
      status = outcome,
      decidedAt = (decision?.time("decided_at") ?: answer?.time("answered_at")),
      decidedWith = decision?.str("note") ?: answer?.str("body"),
    )
  }

  /** The hub's search page: hits grouped by corpus, flattened here in the hub's order. */
  fun search(r: JsonObject): SearchResults {
    val hits =
      (r["groups"]?.rows().orEmpty()).flatMap { g -> g["hits"]?.rows().orEmpty() }.mapNotNull { h ->
        val kind =
          when (h.str("kind")) {
            "feed" -> SearchKind.Feed
            "artifact" -> SearchKind.Artifact
            "brain" -> SearchKind.Brain
            "kb" -> SearchKind.Knowledge
            else -> return@mapNotNull null
          }
        val ref = h.str("ref_id") ?: return@mapNotNull null
        SearchHit(
          kind = kind, ref = ref, project = h.str("project_display_name") ?: h.str("project_id"),
          title = h.str("title") ?: h.str("snippet").orEmpty(), snippet = h.str("snippet").orEmpty(), at = h.time("updated_at"),
          session = h.str("session_id"), sessionName = h.str("session_name"), by = h.sub("author")?.str("agent") ?: h.str("actor"),
        )
      }
    return SearchResults(hits, r.bool("truncated") == true)
  }

  fun event(e: JsonObject, now: Instant): FeedEvent? {
    val id = e.str("id") ?: e.str("event_id") ?: return null
    val kind =
      when (e.str("kind")) {
        "question", "approval" -> EventKind.Asked
        "finished" -> EventKind.Finished
        "page", "artifact" -> EventKind.Published
        "idea" -> EventKind.Captured
        "answer" -> EventKind.Answered
        "decision" -> EventKind.Approved
        else -> EventKind.Noted
      }
    val author = e.sub("author")
    val waiting = e.str("inbox_status") == "action" || e.str("status") == "action"
    return FeedEvent(
      id = id, kind = kind, who = e.str("actor") ?: author?.str("agent") ?: "someone", text = e.str("summary") ?: "",
      tool = author?.str("agent"), machine = author?.str("member"), at = e.time("created_at") ?: now,
      project = e.str("project_display_name") ?: e.str("project_id"), waitingOnYou = waiting,
      askId = if (kind == EventKind.Asked) (e.str("thread_id") ?: id) else null,
    )
  }

  /** A session. [nameOf] turns a parent id into the parent's name, which the hub does not send. */
  fun session(s: JsonObject, now: Instant, nameOf: (String) -> String? = { null }, projectName: String? = null): SessionSummary? {
    val id = s.str("id") ?: return null
    val name = s.str("session_name") ?: id
    val parentName = s.str("parent_id")?.let(nameOf)
    val state =
      when {
        s.str("status") == "ended" -> SessionState.Ended
        s.bool("live") == true -> SessionState.Live
        else -> SessionState.Idle
      }
    val parts = s.rows("participants").map { Participant(it.str("agent") ?: "agent", it.str("member") ?: "", it.time("last_seen_at")) }
    return SessionSummary(
      id = id, name = if (parentName != null && !name.contains('/')) "$parentName/$name" else name,
      project = projectName ?: s.str("project_display_name") ?: s.str("project_id") ?: "", parent = parentName,
      state = state, events = (s.long("events") ?: 0).toInt(), bytes = s.long("brain_bytes") ?: 0, participants = parts,
      lastActivity = s.time("last_activity") ?: now, startedAt = s.time("created_at"),
    )
  }

  fun project(p: JsonObject, bytes: Long, lastWrite: Instant?): ProjectSummary? {
    val id = p.str("id") ?: return null
    return ProjectSummary(
      name = p.str("display_name") ?: id, confidential = p.bool("confidential") == true,
      live = (p.long("sessions_live") ?: 0).toInt(), idle = 0, bytes = bytes, lastActivity = lastWrite, sessionCount = 0, artifactCount = 0, id = id,
    )
  }

  fun artifact(a: JsonObject, now: Instant, projectName: String? = null): ArtifactSummary? {
    val id = a.str("id") ?: return null
    return ArtifactSummary(
      id = id, name = a.str("title") ?: id, project = projectName ?: a.str("project_id") ?: "",
      tool = a.sub("author")?.str("agent") ?: a.str("actor") ?: "", version = (a.long("version") ?: 1).toInt(), bytes = a.long("size_bytes") ?: 0,
      at = a.time("updated_at") ?: now, comments = (a.long("comments_count") ?: 0).toInt(), live = a["live_version"].let { it != null && it !is kotlinx.serialization.json.JsonNull },
      protected = a.bool("protected") == true,
    )
  }

  fun comment(c: JsonObject, now: Instant): ArtifactComment? {
    val id = c.str("id") ?: return null
    return ArtifactComment(id, c.sub("author")?.str("agent") ?: c.str("actor") ?: "someone", c.time("created_at") ?: now, c.str("body") ?: "", c.sub("anchor")?.str("quote"), c.str("status") == "resolved")
  }

  fun member(m: JsonObject): Member? {
    val id = m.str("id") ?: return null
    val enrol = m.sub("enrol")
    return Member(
      name = m.str("display_name") ?: id, pending = m.str("state") == "pending", reason = enrol?.str("note"),
      tools = listOfNotNull(enrol?.str("agent")), live = (m.long("live_sessions") ?: 0).toInt(), firstSeen = m.time("token_issued_at"), lastUsed = m.time("last_seen_at"),
      agentsSeen = emptyList<AgentSeen>(), tokenLive = m.bool("has_live_token") != false,
    )
  }

  fun storage(s: JsonObject, ended: List<EndedSession>): Storage {
    val by = s.sub("by_kind")
    val segs =
      listOf("sessions" to "Session brains", "artifacts" to "Artifacts", "events" to "Feed and index", "knowledge" to "Knowledge bases")
        .mapIndexedNotNull { i, (k, label) -> by?.long(k)?.let { StorageSegment(label, it, i) } }
    return Storage(s.long("used_bytes") ?: s.long("total_bytes") ?: 0, segs, ended)
  }
}
