package dev.agync.agenthub.data

import dev.agync.agenthub.model.AgentSeen
import dev.agync.agenthub.model.Anchor
import dev.agync.agenthub.model.ArtifactComment
import dev.agync.agenthub.model.BrainEntry
import dev.agync.agenthub.model.SearchHit
import dev.agync.agenthub.model.SearchKind
import dev.agync.agenthub.model.SearchResults
import dev.agync.agenthub.model.ArtifactSummary
import dev.agync.agenthub.model.ArtifactView
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.AskStatus
import dev.agync.agenthub.model.Enrolment
import dev.agync.agenthub.model.EndedSession
import dev.agync.agenthub.model.EventKind
import dev.agync.agenthub.model.FeedEvent
import dev.agync.agenthub.model.Grant
import dev.agync.agenthub.model.HubInfo
import dev.agync.agenthub.model.Member
import dev.agync.agenthub.model.Participant
import dev.agync.agenthub.model.PhoneDevice
import dev.agync.agenthub.model.ProjectSummary
import dev.agync.agenthub.model.Receipts
import dev.agync.agenthub.model.RelatedAsk
import dev.agync.agenthub.model.SealedArtifact
import dev.agync.agenthub.crypto.Envelope
import dev.agync.agenthub.model.SessionState
import dev.agync.agenthub.model.SessionSummary
import dev.agync.agenthub.model.Storage
import dev.agync.agenthub.model.StorageSegment
import java.time.Clock
import java.time.Duration
import java.time.Instant
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow

/**
 * The hub the design board shows, in memory. It stands in for the real transport until the hub
 * contract lands, drives the screenshots and the tests, and runs the app with no hub at all.
 * Times are minutes before [base], so the board's 14:32 reads the same on any day.
 */
class SeededHub(
  private val clock: Clock,
  private val base: Instant = clock.instant(),
  private val thisPhone: String = "pixel-phone",
) : HubClient {
  /** Switches for the failure states the board draws. */
  @Volatile var reachable: Boolean = true
  @Volatile var revoked: Boolean = false
  @Volatile var version: String = "1.13.0"
  @Volatile var via: String? = "LAN"

  /** The refusal the next decision gets, as when someone else answered first. */
  @Volatile var refuseNext: Refusal? = null

  /** A hub that takes the call and never answers: for the callers that must not wait on it. */
  @Volatile var hangNow: Boolean = false
  @Volatile var hangDecide: Boolean = false

  /** Asks the hub turns down with a 4xx whenever they are answered, decided or admitted. */
  val rejectAsks: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

  private fun rejectIfMarked(id: String) {
    if (id in rejectAsks) throw HubException.Rejected(422, "not acceptable")
  }

  /** A hub that does not let a phone approve a joining machine, as when the operator reserves it for the web app. */
  @Volatile var refuseAdmitFromPhone: Boolean = false

  private val tickFlow = MutableSharedFlow<Unit>(extraBufferCapacity = 8, replay = 0)
  override val ticks: Flow<Unit> = tickFlow.asSharedFlow()

  fun emitTick() = tickFlow.tryEmit(Unit)

  private fun ago(minutes: Long) = base.minus(Duration.ofMinutes(minutes))

  private val asks = mutableListOf(
    Ask(
      id = "7HQ2KD", kind = AskKind.Approval, agent = "feat-invoices", sentence = "Run the migration on staging",
      prose = "Applies 0042_invoice_totals to billing_staging. Alters 3 tables, about 18,000 rows. The down migration reverses it.",
      project = "billing-svc", tool = "claude", machine = "laptop", askedAt = ago(11),
      deadline = base.plus(Duration.ofMinutes(208)), onSilence = "Declined at 18:00",
      receipts = Receipts("feat-invoices", 48, null, "migration-plan.md", 3, "7HQ2KD"),
    ),
    Ask(
      id = "9MX4TB", kind = AskKind.Question, agent = "nightly", sentence = "Upgrade k3s to 1.31 tonight?",
      prose = "The upgrade drains each node in turn, so expect about 20 minutes of reduced capacity. I read the release notes; none of the removed APIs appear in our manifests.",
      project = "home-lab", tool = "gemini", machine = "build-box", askedAt = ago(24),
      answers = listOf("Yes, tonight", "Wait for the weekend"),
      receipts = Receipts("nightly", 12, "kv/upgrade-plan", null, null, "9MX4TB"),
      related = listOf(RelatedAsk("nightly", "Rotate the backup key?", "Answered: Yes", "Mon")),
    ),
    Ask(
      id = "JN3STU", kind = AskKind.Joining, agent = "studio-mac", sentence = "studio-mac wants to join the hub",
      askedAt = ago(42), tool = "opencode",
      enrolment = Enrolment("studio-mac", "studio-mac", "opencode", "macOS 15", "192.168.1.42", "Design work in space-a", listOf("space-a", "docs-site")),
      receipts = Receipts(eventId = "JN3STU"),
    ),
    Ask(
      id = "DS5REF", kind = AskKind.Question, agent = "docs-sweep", sentence = "Which section should the API reference live under?",
      prose = "Both fit. Reference keeps it next to the CLI pages; Guides puts it beside the tutorials.",
      project = "docs-site", tool = "opencode", machine = "desktop", askedAt = ago(112),
      answers = listOf("Reference", "Guides"), receipts = Receipts("docs-sweep", 9, null, null, null, "DS5REF"),
    ),
  )

  private val answered = mutableListOf<FeedEvent>()

  private val feed = mutableListOf(
    FeedEvent("E1", EventKind.Asked, "nightly", "asks: Upgrade k3s to 1.31 tonight?", "gemini", "build-box", ago(24), "home-lab", waitingOnYou = true, askId = "9MX4TB"),
    FeedEvent("E2", EventKind.Published, "nightly", "published Backup report", "gemini", "build-box", ago(52), "home-lab"),
    FeedEvent("E3", EventKind.Noted, "nightly/verify", "noted: 14 of 14 snapshots restore cleanly", "gemini", "build-box", ago(61), "home-lab"),
    FeedEvent("E4", EventKind.Started, "nightly", "started nightly/verify", "gemini", "build-box", ago(90), "home-lab"),
    FeedEvent("E5", EventKind.Finished, "upgrade-dns", "finished: Pi-hole moved to the new node", "claude", "laptop", ago(197), "home-lab"),
    FeedEvent("E6", EventKind.Captured, "You", "captured: Try Longhorn for the media volume", at = ago(330), project = "home-lab"),
    FeedEvent("E7", EventKind.Published, "docs-sweep", "published Docs sweep report", "opencode", "desktop", ago(80), "docs-site"),
  )

  private fun session(
    id: String, name: String, project: String, state: SessionState, events: Int, bytes: Long, minutes: Long,
    parent: String? = null, who: List<Participant>, started: Long = minutes + 60, anchor: Anchor? = null, brain: Int = 0,
  ) = SessionSummary(id, name, project, parent, state, events, bytes, who, ago(minutes), ago(started), anchor, brain)

  private val claudeLaptop = Participant("claude", "laptop", ago(11))
  private val sessions = mutableListOf(
    session(
      "01JDQ7RT", "feat-invoices", "billing-svc", SessionState.Live, 48, 4_100_000, 14, who = listOf(claudeLaptop, Participant("opencode", "desktop", ago(36))),
      started = 74, brain = 7,
      anchor = Anchor("implement", "claude", ago(30), "Totals are computed in the service; the migration adds the columns. Tests pass against a copy of staging.", listOf("Get approval for the staging migration", "Run it, then the smoke tests")),
    ),
    session("01JDQ8AA", "feat-invoices/tests", "billing-svc", SessionState.Live, 9, 300_000, 6, "feat-invoices", listOf(claudeLaptop)),
    session("01JDQ8AB", "feat-invoices/migrations", "billing-svc", SessionState.Ended, 14, 220_000_000, 40, "feat-invoices", listOf(claudeLaptop)),
    session("01JDQ5FR", "fix-rounding", "billing-svc", SessionState.Idle, 22, 610_000, 120, who = listOf(claudeLaptop)),
    session("01JDM1IE", "invoice-export", "billing-svc", SessionState.Ended, 6, 90_000, 60L * 24 * 1, who = listOf(Participant("gemini", "build-box"))),
    session("01JDQ9NI", "nightly", "home-lab", SessionState.Live, 12, 1_200_000, 31, who = listOf(Participant("gemini", "build-box", ago(24)))),
    session("01JDQ9NV", "nightly/verify", "home-lab", SessionState.Ended, 5, 80_000, 8, "nightly", listOf(Participant("gemini", "build-box"))),
    session("01JDP2UD", "upgrade-dns", "home-lab", SessionState.Ended, 31, 70_000_000, 197, who = listOf(claudeLaptop)),
    session("01JDP3DC", "dns-cleanup", "home-lab", SessionState.Ended, 11, 120_000_000, 60L * 30, who = listOf(claudeLaptop)),
    session("01JDQ2DS", "docs-sweep", "docs-site", SessionState.Idle, 9, 940_000, 80, who = listOf(Participant("opencode", "desktop"))),
  )

  private fun artifact(id: String, name: String, project: String, tool: String, version: Int, kb: Long, minutes: Long, comments: Int = 0, live: Boolean = false, protectedFlag: Boolean = false) =
    ArtifactSummary(id, name, project, tool, version, kb * 1000, ago(minutes), comments, live, protectedFlag)

  private val artifacts =
    mutableListOf(
      artifact("ar1", "migration-plan.md", "billing-svc", "claude", 4, 4, 1, 2),
      artifact("ar2", "Backup report", "home-lab", "gemini", 1, 18, 52, live = true),
      artifact("ar3", "Docs sweep report", "docs-site", "opencode", 1, 22, 80),
      artifact("ar4", "q3-roadmap.html", "space-a", "claude", 2, 31, 60L * 24, protectedFlag = true),
      artifact("ar5", "k3s upgrade plan", "home-lab", "gemini", 2, 6, 60L * 24 * 2),
    ) + (1..9).map {
      artifact("ar-old$it", listOf("dns notes", "node inventory", "invoice schema", "restore drill", "tax export", "runbook", "style guide", "release notes", "cost review")[it - 1],
        listOf("home-lab", "home-lab", "billing-svc", "home-lab", "billing-svc", "home-lab", "docs-site", "docs-site", "infra-notes")[it - 1], "claude", 1 + it % 3, 3L + it * 2, 60L * 24 * (3 + it), protectedFlag = it == 6)
    }

  private val projectsList = listOf(
    ProjectSummary("home-lab", false, 1, 0, 1_800_000, ago(13), 4, 6),
    ProjectSummary("billing-svc", false, 1, 1, 6_200_000, ago(11), 5, 3),
    ProjectSummary("docs-site", false, 0, 1, 940_000, ago(60), 1, 2),
    ProjectSummary("space-a", true, 0, 0, 120_000, ago(60L * 24), 0, 1),
    ProjectSummary("infra-notes", false, 0, 0, 48_000, ago(60L * 24 * 12), 0, 1),
  )

  private fun gate() {
    if (!reachable) throw HubException.Unreachable()
    if (revoked) throw HubException.Unpaired()
  }

  /** The web address the hub reports, when it reports one. */
  @Volatile var webUrl: String? = null

  private fun infoNow() = HubInfo("home hub", version, via, listOf(PhoneDevice(thisPhone, true), PhoneDevice("tablet", false)), webUrl)

  override suspend fun info(): HubInfo = gate().let { infoNow() }

  override suspend fun now(): NowSnapshot {
    gate()
    if (hangNow) kotlinx.coroutines.awaitCancellation()
    val live = sessions.filter { it.state == SessionState.Live }
    return NowSnapshot(
      info = infoNow(),
      asOf = clock.instant(),
      asks = asks.toList(),
      recent = feed.filter { it.kind != EventKind.Asked && it.kind != EventKind.Started }.take(5),
      live = live,
      answeredToday = answered.toList(),
    )
  }

  override suspend fun ask(id: String): Ask {
    gate()
    return asks.firstOrNull { it.id == id } ?: throw HubException.Unreachable("no ask $id")
  }

  private fun close(id: String, status: AskStatus, text: String, kind: EventKind) {
    val i = asks.indexOfFirst { it.id == id }
    if (i < 0) return
    val a = asks[i]
    asks[i] = a.copy(status = status, decidedAt = clock.instant(), decidedWith = text)
    answered += FeedEvent("done-$id", kind, "You", text, at = clock.instant(), project = a.project, askId = id)
  }

  private fun refuseIfSet() {
    refuseNext?.let { r -> refuseNext = null; throw HubException.Refused(r) }
  }

  override suspend fun answer(askId: String, text: String) {
    gate()
    refuseIfSet()
    rejectIfMarked(askId)
    val a = asks.firstOrNull { it.id == askId }
    if (a == null || !a.waiting) throw HubException.Refused(Refusal.AnsweredElsewhere(clock.instant(), null))
    close(askId, AskStatus.Answered, "answered ${a.agent}: $text", EventKind.Answered)
  }

  override suspend fun decide(askId: String, approve: Boolean, note: String?) {
    gate()
    if (hangDecide) kotlinx.coroutines.awaitCancellation()
    refuseIfSet()
    rejectIfMarked(askId)
    val a = asks.firstOrNull { it.id == askId }
    if (a == null || !a.waiting) throw HubException.Refused(Refusal.AnsweredElsewhere(clock.instant(), null))
    close(askId, if (approve) AskStatus.Approved else AskStatus.Declined, "${if (approve) "approved" else "declined"} ${a.agent}: ${a.sentence}", EventKind.Approved)
  }

  override suspend fun admit(askId: String, approve: Boolean, name: String, grants: Set<String>) {
    gate()
    refuseIfSet()
    rejectIfMarked(askId)
    if (refuseAdmitFromPhone) throw HubException.NotAllowed("enrol approve")
    val a = asks.firstOrNull { it.id == askId } ?: throw HubException.Refused(Refusal.AnsweredElsewhere(clock.instant(), null))
    close(askId, if (approve) AskStatus.Admitted else AskStatus.TurnedAway, "${if (approve) "admitted" else "turned away"} $name", EventKind.Approved)
    if (approve) members.add(Member(name, tools = listOfNotNull(a.enrolment?.firstAgent), firstSeen = clock.instant(), grants = grants.map { Grant(it, clock.instant()) }))
    members.removeAll { it.pending && it.name == a.enrolment?.machine }
  }

  override suspend fun projects(): List<ProjectSummary> = gate().let { projectsList }

  override suspend fun feed(project: String): List<FeedEvent> = gate().let { feed.filter { it.project == project }.sortedByDescending { it.at } }

  override suspend fun sessions(project: String): List<SessionSummary> = gate().let { sessions.filter { it.project == project } }

  override suspend fun session(id: String): SessionSummary = gate().let { sessions.first { it.id == id || it.name == id } }

  override suspend fun endSession(id: String) {
    gate()
    val i = sessions.indexOfFirst { it.id == id }
    if (i >= 0) sessions[i] = sessions[i].copy(state = SessionState.Ended)
  }

  override suspend fun artifacts(): List<ArtifactSummary> = gate().let { artifacts.sortedByDescending { it.at } }

  override suspend fun artifact(id: String, version: Int?): ArtifactView {
    gate()
    val s = artifacts.first { it.id == id }
    if (s.protected) {
      return ArtifactView(s, s.version, s.version, s.at, "", emptyList(), SealedArtifact(PROTECTED_ENVELOPE, PROTECTED_CIPHERTEXT))
    }
    return ArtifactView(
      summary = s,
      shownVersion = version ?: (s.version - 1).coerceAtLeast(1),
      latestVersion = s.version,
      latestAt = s.at,
      html = SAMPLE_HTML,
      comments = listOf(
        ArtifactComment("c1", "you", ago(8), "Do the batches pause between runs? Staging shares the disk with the search index.", "backfills them from line items in batches of 2,000"),
        ArtifactComment("c2", "claude", ago(5), "Yes, 200 ms between batches.", null),
      ),
    )
  }

  private val members = mutableListOf(
    Member("studio-mac", pending = true, reason = "Design work in space-a", tools = listOf("opencode"), lastUsed = ago(42)),
    Member("laptop", tools = listOf("claude", "opencode"), live = 1, idle = 1, firstSeen = ago(60L * 24 * 53), lastUsed = ago(11),
      agentsSeen = listOf(AgentSeen("claude", "feat-invoices (live), fix-rounding (idle)", ago(11)), AgentSeen("opencode", "no session now", ago(60L * 24 * 2))),
      grants = listOf(Grant("space-a", ago(60L * 24 * 12)))),
    Member("desktop", tools = listOf("opencode"), live = 1, idle = 1),
    Member("build-box", tools = listOf("gemini"), live = 1),
  )

  override suspend fun members(): List<Member> = gate().let { members.toList() }

  override suspend fun storage(): Storage {
    gate()
    return Storage(
      2_400_000_000,
      listOf(
        StorageSegment("Session brains", 1_100_000_000, 0), StorageSegment("Artifacts", 530_000_000, 1),
        StorageSegment("Feed and index", 410_000_000, 2), StorageSegment("Knowledge bases", 360_000_000, 3),
      ),
      sessions.filter { it.state == SessionState.Ended }.map { EndedSession(it.name, it.project, it.bytes) },
    )
  }

  private val pruned = mutableMapOf<String, SessionSummary>()

  override suspend fun pruneEnded(): PruneUndo {
    gate()
    val gone = sessions.filter { it.state == SessionState.Ended }
    sessions.removeAll(gone)
    gone.forEach { pruned["undo-${it.id}"] = it }
    return PruneUndo(gone.map { "undo-${it.id}" }, clock.instant().plusSeconds(30))
  }

  override suspend fun pruneSession(id: String): PruneUndo {
    gate()
    val s = sessions.firstOrNull { it.id == id && it.state == SessionState.Ended } ?: return PruneUndo(emptyList(), null)
    sessions.remove(s)
    pruned["undo-${s.id}"] = s
    return PruneUndo(listOf("undo-${s.id}"), clock.instant().plusSeconds(30))
  }

  override suspend fun undoPrune(undo: PruneUndo) {
    gate()
    undo.tokens.forEach { t -> pruned.remove(t)?.let { sessions += it } }
  }

  override suspend fun captureIdea(project: String, text: String, link: String?) {
    gate()
    val id = "idea-${feed.size}"
    feed.add(0, FeedEvent(id, EventKind.Captured, "You", "captured: ${text.ifBlank { link.orEmpty() }}", at = clock.instant(), project = project))
    asks += Ask(id, AskKind.Idea, "You", text.ifBlank { link.orEmpty() }, prose = link, project = project, askedAt = clock.instant(), receipts = Receipts(eventId = id))
  }

  private fun closeIdea(id: String, status: AskStatus) {
    gate()
    refuseIfSet()
    val i = asks.indexOfFirst { it.id == id && it.kind == AskKind.Idea }
    if (i >= 0) asks[i] = asks[i].copy(status = status, decidedAt = clock.instant())
  }

  override suspend fun resolveIdea(id: String) = closeIdea(id, AskStatus.Resolved)

  override suspend fun clearIdea(id: String) = closeIdea(id, AskStatus.Cleared)

  /** The notes added to ideas, newest last, for tests and the debug build. */
  val ideaNotes = mutableListOf<Pair<String, String>>()

  override suspend fun replyIdea(id: String, text: String) {
    gate()
    ideaNotes += id to text
  }

  private val brain = mapOf(
    ("nightly" to "/kv/upgrade-plan") to "plan: upgrade k3s to 1.31\nnodes: drain one at a time, about 20 minutes of reduced capacity\nchecked: none of the removed APIs appear in our manifests\nrollback: the 1.30 snapshot taken at 22:00",
  )

  override suspend fun search(query: String): SearchResults {
    gate()
    val q = query.trim().lowercase()
    if (q.isEmpty()) return SearchResults(emptyList(), false)
    val feedHits = feed.filter { it.text.lowercase().contains(q) }.map { e ->
      SearchHit(SearchKind.Feed, e.askId ?: e.id, e.project, e.text, e.text, e.at, by = e.who)
    }
    val artifactHits = artifacts.filter { it.name.lowercase().contains(q) && !it.protected }.map { a ->
      SearchHit(SearchKind.Artifact, a.id, a.project, a.name, "${a.name}, version ${a.version}", a.at, by = a.tool)
    }
    val brainHits = brain.filter { (_, v) -> v.lowercase().contains(q) }.map { (k, v) ->
      SearchHit(SearchKind.Brain, k.second, null, k.second, v.lineSequence().first(), null, session = k.first, sessionName = k.first)
    }
    return SearchResults(feedHits + artifactHits + brainHits, false)
  }

  override suspend fun brainEntry(session: String, path: String): BrainEntry? {
    gate()
    return brain[session to path]?.let { BrainEntry(path, "key", it.length.toLong(), it) }
  }

  override suspend fun markSeen() = gate()

  /** The newest event each project's feed was marked seen up to. */
  val feedSeen = mutableMapOf<String, String>()

  /** A hub that turns the cursor away, for the test that the feed still shows. */
  @Volatile var refuseSeen: Boolean = false

  override suspend fun markFeedSeen(project: String, eventId: String) {
    gate()
    if (refuseSeen) throw HubException.NotAllowed("feed/seen")
    feedSeen[project] = eventId
  }

  companion object {
    /** Sealed by the web app's own crypto module with [PROTECTED_PASSWORD]: a cross-implementation vector. */
    const val PROTECTED_PASSWORD = "correct horse battery"
    val PROTECTED_ENVELOPE = Envelope("AES-256-GCM", "PBKDF2-HMAC-SHA256", 600000, "4lflaJENhdz7CimJUxk1jg==", "Leh1Ru+RyJYwvLS/")
    const val PROTECTED_CIPHERTEXT = "pIRdNLp+Cyigh5bjda8kUBz3L4lfZD9o/svINTWnB+JGcwamxz6x8+M2GG5+FFyOyMwRW/R07I0meOg7gYM7I2LtWTwOJUJaClTPJg=="

    val SAMPLE_HTML = """
      <!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1">
      <style>body{font:16px/1.6 system-ui,sans-serif;margin:0;padding:4px 56px 24px 16px;color:#151B17;background:#FBFCFA}
      h1{font-size:24px;margin:12px 0 8px}h2{font-size:18px;margin:20px 0 6px}table{width:100%;border-collapse:collapse;font-size:14px}
      th,td{text-align:left;padding:8px 6px 8px 0;border-bottom:1px solid #D3DAD2}@media(prefers-color-scheme:dark){body{background:#161B18;color:#E4EAE5}th,td{border-color:#29312C}}</style></head>
      <body><h1>Staging migration: invoice totals</h1><p>Version 3</p><h2>What changes</h2>
      <p>Migration 0042 adds total_cents and tax_cents to invoices, backfills them from line items in batches of 2,000, and drops the old view.</p>
      <h2>Tables touched</h2><table><tr><th>Table</th><th>Change</th><th>Rows</th></tr>
      <tr><td>invoices</td><td>2 columns</td><td>11,204</td></tr><tr><td>line_items</td><td>index</td><td>6,881</td></tr>
      <tr><td>invoice_view</td><td>dropped</td><td>0</td></tr></table>
      <h2>Rollback</h2><p>The down migration restores the view and drops the columns. It takes under a minute on staging.</p></body></html>
    """.trimIndent()
  }
}
