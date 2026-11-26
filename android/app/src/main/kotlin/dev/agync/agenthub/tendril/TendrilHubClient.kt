package dev.agync.agenthub.tendril

import dev.agync.agenthub.data.HubClient
import dev.agync.agenthub.data.HubException
import dev.agync.agenthub.data.PruneUndo
import dev.agync.agenthub.data.NowSnapshot
import dev.agync.agenthub.data.Refusal
import dev.agync.agenthub.model.ArtifactSummary
import dev.agync.agenthub.model.ArtifactView
import dev.agync.agenthub.model.BrainEntry
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskStatus
import dev.agync.agenthub.model.EndedSession
import dev.agync.agenthub.model.EventKind
import dev.agync.agenthub.model.FeedEvent
import dev.agync.agenthub.model.HubInfo
import dev.agync.agenthub.model.Member
import dev.agync.agenthub.model.WebAddress
import dev.agync.agenthub.model.SearchResults
import dev.agync.agenthub.model.ProjectSummary
import dev.agync.agenthub.model.SealedArtifact
import dev.agync.agenthub.model.SessionState
import dev.agync.agenthub.model.SessionSummary
import dev.agync.agenthub.model.Storage
import dev.agync.agenthub.crypto.Envelope
import java.security.MessageDigest
import java.time.Clock
import java.time.Instant
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.isActive
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import okhttp3.Request

/**
 * The hub over a Tendril session, as the hub's contract defines it: the REST API on the operator's
 * routes, carried as HTTP/2 over the session, with no credential. The app picks no route; Tendril
 * does when the session opens. Hub answers are mapped by [HubJson], leaving a model field at its
 * default where the hub sends nothing for it.
 */
class TendrilHubClient(
  private val transport: HubTransport,
  private val clock: Clock = Clock.systemDefaultZone(),
  private val retryMillis: Long = 3_000,
  private val maxRetryMillis: Long = 30_000,
  /** The hub sends a keep-alive every 20 s; a stream silent for longer than this is taken as dead. */
  private val streamReadMillis: Long = 45_000,
) : HubClient {
  private fun at(): Instant = clock.instant()

  /** Routes take project ids; the screens carry display names. */
  private val projectIds = java.util.concurrent.ConcurrentHashMap<String, String>()
  private val projectNames = java.util.concurrent.ConcurrentHashMap<String, String>()
  private val sessionNames = java.util.concurrent.ConcurrentHashMap<String, String>()
  private val unknownSessions: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()
  private val artifactIndex = java.util.concurrent.ConcurrentHashMap<String, ArtifactSummary>()

  private fun pid(name: String) = enc(projectIds[name] ?: name)

  /** The freshness stream: a tick when the stream opens and on every event, reconnecting with backoff. */
  override val ticks: Flow<Unit> =
    channelFlow {
      var wait = retryMillis
      // The stream has its own session and keeps none idle: when it dies, nothing reuses it, and a reconnect is a new session.
      val streamHttp =
        transport.http.newBuilder()
          .readTimeout(streamReadMillis, java.util.concurrent.TimeUnit.MILLISECONDS).callTimeout(0, java.util.concurrent.TimeUnit.MILLISECONDS)
          .connectionPool(okhttp3.ConnectionPool(0, 1, java.util.concurrent.TimeUnit.MINUTES)).build()
      while (currentCoroutineContext().isActive && !transport.unpaired) {
        try {
          val req = Request.Builder().url("http://hub/api/v1/stream").header("Accept", "text/event-stream").build()
          streamHttp.newCall(req).execute().use { r ->
            if (!r.isSuccessful) throw java.io.IOException("stream ${r.code}")
            wait = retryMillis
            send(Unit)
            val source = r.body.source()
            while (currentCoroutineContext().isActive) {
              val line = source.readUtf8Line() ?: break
              if (line.startsWith("data:") || line.startsWith("event:")) send(Unit)
            }
          }
        } catch (_: java.io.IOException) {
          // The session dropped or went silent: reconnect over a new one, and the first tick refetches.
          streamHttp.connectionPool.evictAll()
        }
        delay(wait)
        wait = minOf(wait * 2, maxRetryMillis)
      }
    }.flowOn(Dispatchers.IO)

  private suspend fun <T> mapped(block: suspend () -> T): T =
    try {
      block()
    } catch (e: HttpProblem) {
      throw mapProblem(e)
    }

  private fun mapProblem(e: HttpProblem): HubException =
    when {
      e.status == 401 && e.reason == "device_unpaired" -> HubException.Unpaired()
      // A phone may not call this route: the control should not exist, so this is not an error state.
      e.status == 403 && e.reason == "operator_only" -> HubException.NotAllowed(e.detail ?: "")
      // The hub answered, so it is reachable: a 4xx is a call it turned down, a 5xx is its own trouble.
      e.status in 400..499 -> HubException.Rejected(e.status, e.detail)
      e.status in 500..599 -> HubException.ServerError(e.status)
      else -> HubException.Unreachable(e.message.orEmpty())
    }

  override suspend fun info(): HubInfo =
    try {
      val me = transport.json("GET", "/api/v1/devices/me").obj() ?: throw HubException.Unreachable("unreadable")
      val hub = me.sub("hub")
      val version = hub?.str("version") ?: ""
      if ((me.long("phone_api") ?: 0) < PHONE_API) throw HubException.TooOld(version, "phone API $PHONE_API")
      if (me.str("state") == "active") transport.markActive()
      HubInfo(hub?.str("name") ?: "hub", version, null, webUrl = WebAddress.parse(hub?.str("web_url")))
    } catch (e: HttpProblem) {
      if (e.status == 404) throw HubException.TooOld("", "phone API $PHONE_API")
      throw mapProblem(e)
    }

  /** A session's name, which an inbox item carries only as an id. Cached; an unknown id falls back to the author. */
  private suspend fun sessionName(id: String?): String? {
    if (id == null) return null
    sessionNames[id]?.let { return it }
    if (id in unknownSessions) return null
    val name = runCatching { transport.json("GET", "/api/v1/sessions/${enc(id)}").obj()?.str("session_name") }.getOrNull()
    // A failed lookup is remembered too, so each refresh does not ask again for a session the hub no longer has.
    if (name != null) sessionNames[id] = name else unknownSessions += id
    return name
  }

  private suspend fun asks(items: List<JsonObject>): List<Ask> = items.mapNotNull { HubJson.ask(it, at(), sessionName(it.str("session_id"))) }

  override suspend fun now(): NowSnapshot = mapped {
    val info = info()
    val home = transport.json("GET", "/api/v1/home").obj() ?: throw HubException.Unreachable("unreadable")
    val n = at()
    val asksMapped = asks((home["waiting_items"] ?: JsonNull).rows())
    val recent = (home["recent"] ?: JsonNull).rows().mapNotNull { HubJson.event(it, n) }
    val liveRows = transport.json("GET", "/api/v1/sessions?live=1").rows("sessions", "items")
    liveRows.forEach { r -> r.str("id")?.let { id -> sessionNames[id] = r.str("session_name") ?: id } }
    val live = liveRows.mapNotNull { HubJson.session(it, n, { id -> sessionNames[id] }) }
    NowSnapshot(
      info = info, asOf = n, asks = asksMapped,
      recent = recent.filter { it.kind == EventKind.Published || it.kind == EventKind.Finished || it.kind == EventKind.Captured },
      live = live.map { it.copy(state = SessionState.Live) },
      answeredToday = recent.filter { it.kind == EventKind.Answered || it.kind == EventKind.Approved },
    )
  }

  override suspend fun ask(id: String): Ask = mapped {
    findItem(id, "action") ?: findItem(id, "resolved") ?: throw HubException.Rejected(404, "no ask $id")
  }

  private suspend fun findItem(id: String, status: String): Ask? =
    asks(transport.json("GET", "/api/v1/inbox?status=$status&limit=200").rows("items").filter { it.str("event_id") == id }).firstOrNull()

  /** A stable key so a retried send returns the original instead of deciding twice. */
  private fun key(vararg parts: String): String = MessageDigest.getInstance("SHA-256").digest(parts.joinToString("\u0000").toByteArray()).take(12).joinToString("") { "%02x".format(it) }

  /**
   * A 409 `conflict` is the hub saying the ask is already settled. The detail says whether it was
   * decided or its deadline closed it; the item says what was decided and when.
   */
  private suspend fun <T> deciding(askId: String, block: suspend () -> T): T =
    try {
      block()
    } catch (e: HttpProblem) {
      if (e.status != 409) throw mapProblem(e)
      throw HubException.Refused(refusalFor(askId, e))
    }

  private suspend fun refusalFor(askId: String, e: HttpProblem): Refusal {
    val settled = runCatching { findItem(askId, "resolved") }.getOrNull()
    val deadline = e.detail?.contains("deadline") == true || settled?.status == AskStatus.ClosedAtDeadline
    return if (deadline) Refusal.ClosedAtDeadline(settled?.deadline ?: at())
    else Refusal.AnsweredElsewhere(settled?.decidedAt ?: at(), settled?.decidedWith)
  }

  override suspend fun answer(askId: String, text: String) {
    deciding(askId) { transport.json("POST", "/api/v1/questions/${enc(askId)}/answer", buildJsonObject { put("body", text); put("idempotency_key", key("answer", askId, text)) }) }
  }

  override suspend fun decide(askId: String, approve: Boolean, note: String?) {
    deciding(askId) {
      transport.json(
        "POST", "/api/v1/approvals/${enc(askId)}/decision",
        buildJsonObject {
          put("decision", if (approve) "approve" else "decline")
          if (!note.isNullOrBlank()) put("note", note)
          put("idempotency_key", key("decision", askId, approve.toString(), note.orEmpty()))
        },
      )
    }
  }

  override suspend fun admit(askId: String, approve: Boolean, name: String, grants: Set<String>) {
    val enrolment = ask(askId).enrolment
    val member = enrolment?.memberId ?: throw HubException.Refused(Refusal.AnsweredElsewhere(at(), null))
    // A phone may grant some of the projects the machine asked for, and nothing else: no other name, no sharing. [name] is only a label.
    val asked = grants.filter { it in enrolment.requestedProjects }
    deciding(askId) {
      if (approve) {
        transport.json(
          "POST", "/api/v1/enrol/${enc(member)}/approve",
          buildJsonObject { if (asked.isNotEmpty()) putJsonArray("projects") { asked.forEach { add(JsonPrimitive(it)) } } },
        )
      } else {
        transport.json("POST", "/api/v1/enrol/${enc(member)}/refuse", buildJsonObject { })
      }
    }
  }

  override suspend fun resolveIdea(id: String) {
    mapped { transport.json("POST", "/api/v1/ideas/${enc(id)}/resolve", buildJsonObject { }) }
  }

  override suspend fun clearIdea(id: String) {
    mapped { transport.json("POST", "/api/v1/ideas/${enc(id)}/clear", buildJsonObject { }) }
  }

  override suspend fun replyIdea(id: String, text: String) {
    mapped { transport.json("POST", "/api/v1/ideas/${enc(id)}/reply", buildJsonObject { put("summary", text) }) }
  }

  private suspend fun projectRows(): List<JsonObject> {
    val rows = transport.json("GET", "/api/v1/projects").rows("projects", "items")
    rows.forEach { r -> r.str("id")?.let { id -> projectNames[id] = r.str("display_name") ?: id; projectIds[r.str("display_name") ?: id] = id } }
    return rows
  }

  /** Projects, not member spaces: a personal space is an agent's own and is not listed with them. */
  override suspend fun projects(): List<ProjectSummary> = mapped {
    val rows = projectRows().filter { it.bool("is_personal") != true }
    val storage = transport.json("GET", "/api/v1/storage").obj()?.get("projects")?.rows().orEmpty().associateBy { it.str("project_id") ?: "" }
    rows.mapNotNull { p ->
      val s = storage[p.str("id")]
      val bytes = listOf("events_bytes", "session_bytes", "artifact_bytes", "kb_bytes").sumOf { s?.long(it) ?: 0 }
      HubJson.project(p, bytes, s?.time("last_write"))
    }
  }

  override suspend fun feed(project: String): List<FeedEvent> = mapped {
    transport.json("GET", "/api/v1/projects/${pid(project)}/feed").rows("events", "items").mapNotNull { HubJson.event(it, at()) }
  }

  override suspend fun sessions(project: String): List<SessionSummary> = mapped {
    val rows = transport.json("GET", "/api/v1/sessions?project=${pid(project)}").rows("sessions", "items")
    rows.forEach { r -> r.str("id")?.let { id -> sessionNames[id] = r.str("session_name") ?: id } }
    rows.mapNotNull { HubJson.session(it, at(), { id -> sessionNames[id] }, project) }
  }

  override suspend fun session(id: String): SessionSummary = mapped {
    transport.json("GET", "/api/v1/sessions/${enc(id)}").obj()?.let { HubJson.session(it, at(), { p -> sessionNames[p] }) } ?: throw HubException.Unreachable("no session")
  }

  override suspend fun endSession(id: String) {
    mapped { transport.json("POST", "/api/v1/sessions/${enc(id)}/end", buildJsonObject { }) }
  }

  /** The hub lists artifacts by project (the all-artifacts route needs a session), so this walks the projects. */
  override suspend fun artifacts(): List<ArtifactSummary> = mapped {
    projectRows().flatMap { p ->
      val id = p.str("id") ?: return@flatMap emptyList()
      transport.json("GET", "/api/v1/projects/${enc(id)}/artifacts").rows("artifacts", "items").mapNotNull { HubJson.artifact(it, at(), projectNames[id]) }
    }.also { l -> l.forEach { artifactIndex[it.id] = it } }
  }

  override suspend fun artifact(id: String, version: Int?): ArtifactView = mapped {
    val summary = artifactIndex[id] ?: artifacts().firstOrNull { it.id == id } ?: throw HubException.Unreachable("no artifact $id")
    val detail = transport.json("GET", "/api/v1/artifacts/${enc(id)}" + (version?.let { "?version=$it" } ?: "")).obj() ?: throw HubException.Unreachable("unreadable")
    val comments = runCatching { transport.json("GET", "/api/v1/artifacts/${enc(id)}/comments").rows("comments", "items").mapNotNull { HubJson.comment(it, at()) } }.getOrDefault(emptyList())
    val shown = (detail.long("version") ?: summary.version.toLong()).toInt()
    val env = detail.sub("envelope")
    when {
      detail.bool("protected") == true && env != null && detail.str("content") != null ->
        // A protected artifact comes as ciphertext with its envelope; the phone opens it with the project password.
        ArtifactView(summary.copy(protected = true), shown, summary.version, summary.at, "", emptyList(), SealedArtifact(Envelope(env.str("alg") ?: "", env.str("kdf") ?: "", (env.long("iterations") ?: 0).toInt(), env.str("salt") ?: "", env.str("iv") ?: ""), detail.str("content")!!))
      detail.str("kind") == "markdown" ->
        // The frame route refuses markdown (the web app renders it in its own page), so it is rendered here, with raw HTML escaped.
        ArtifactView(summary, shown, summary.version, summary.at, Markdown.page(detail.str("title") ?: summary.name, detail.str("content").orEmpty()), comments)
      else -> {
        val html = transport.call("GET", "/artifacts/${enc(id)}/frame" + (version?.let { "?version=$it" } ?: ""), accept = "text/html")
        ArtifactView(summary, shown, summary.version, summary.at, html, comments)
      }
    }
  }

  override suspend fun members(): List<Member> = mapped {
    transport.json("GET", "/api/v1/members").rows("members", "items").mapNotNull(HubJson::member)
  }

  override suspend fun search(query: String): SearchResults = mapped {
    HubJson.search(transport.json("GET", "/api/v1/search?q=${enc(query)}&limit=50").obj() ?: throw HubException.Unreachable("unreadable"))
  }

  override suspend fun brainEntry(session: String, path: String): BrainEntry? =
    try {
      val e = transport.json("GET", "/api/v1/sessions/${enc(session)}/brain/entry?path=${enc(path)}").obj() ?: return null
      BrainEntry(e.str("path") ?: path, e.str("type") ?: "key", e.long("size_bytes") ?: 0, e.str("content").orEmpty())
    } catch (e: HttpProblem) {
      // No entry there, or one that is not text: the hub is fine, so this is not a connection state.
      if (e.status == 404 || e.status == 422) null else throw mapProblem(e)
    }

  override suspend fun storage(): Storage = mapped {
    val s = transport.json("GET", "/api/v1/storage").obj() ?: throw HubException.Unreachable("unreadable")
    projectRows()
    // Ended sessions are listed per project; only a project the hub says has prunable ones is asked.
    val ended =
      s["projects"]?.rows().orEmpty().filter { (it.long("prunable_sessions") ?: 0) > 0 }.flatMap { row ->
        val id = row.str("project_id") ?: return@flatMap emptyList()
        transport.json("GET", "/api/v1/sessions?project=${enc(id)}&status=ended").rows("sessions", "items")
          .mapNotNull { HubJson.session(it, at(), projectName = projectNames[id]) }.map { EndedSession(it.name, it.project, it.bytes) }
      }
    HubJson.storage(s, ended)
  }

  override suspend fun pruneEnded(): PruneUndo = mapped {
    val r = transport.json("DELETE", "/api/v1/storage/sessions").obj()
    PruneUndo(r?.get("sessions")?.rows().orEmpty().mapNotNull { it.str("undo_token") }, r?.time("undo_expires_at"))
  }

  override suspend fun pruneSession(id: String): PruneUndo = mapped {
    val r = transport.json("DELETE", "/api/v1/storage/sessions/${enc(id)}").obj()
    PruneUndo(listOfNotNull(r?.str("undo_token")), r?.time("undo_expires_at"))
  }

  override suspend fun undoPrune(undo: PruneUndo) {
    mapped { undo.tokens.forEach { transport.json("POST", "/api/v1/prune/undo/${enc(it)}", buildJsonObject { }) } }
  }

  override suspend fun captureIdea(project: String, text: String, link: String?) {
    mapped {
      transport.json(
        "POST", "/api/v1/projects/${pid(project)}/ideas",
        buildJsonObject {
          put("summary", text.take(200))
          if (link != null) put("payload", buildJsonObject { put("url", link) })
        },
      )
    }
  }

  override suspend fun markFeedSeen(project: String, eventId: String) {
    mapped { transport.json("POST", "/api/v1/projects/${pid(project)}/feed/seen", buildJsonObject { put("event_id", eventId) }) }
  }

  override suspend fun markSeen() {
    // Per-project cursors move when a feed is opened; there is no hub-wide cursor to move.
  }

  private fun enc(s: String) = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")

  companion object {
    const val PHONE_API = 1
  }
}
