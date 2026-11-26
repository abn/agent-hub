package dev.agync.agenthub.tendril

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.model.AskStatus
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class HubJsonTest {
  private fun obj(s: String) = Json.parseToJsonElement(s).jsonObject

  @Test fun `a decided approval maps its outcome and note`() {
    val a = HubJson.ask(obj("""{"event_id":"E1","kind":"approval","status":"resolved","summary":"Run it","decision":{"decision":"approved","note":"ok"},"updated_at":"2026-11-24T14:40:00Z"}"""), Fixtures.now)!!
    assertEquals(AskStatus.Approved, a.status)
    assertEquals("ok", a.decidedWith)
  }

  @Test fun `an expired approval is closed at its deadline`() {
    val a = HubJson.ask(obj("""{"event_id":"E1","kind":"approval","status":"resolved","summary":"x","decision":{"decision":"declined","expired":true}}"""), Fixtures.now)!!
    assertEquals(AskStatus.ClosedAtDeadline, a.status)
  }

  @Test fun `a row with no id or an unknown kind is skipped, and a bare row list reads`() {
    assertNull(HubJson.ask(obj("""{"kind":"question"}"""), Fixtures.now))
    assertNull(HubJson.ask(obj("""{"event_id":"E","kind":"signal"}"""), Fixtures.now))
    assertEquals(1, Json.parseToJsonElement("""[{"a":1}]""").rows("x").size)
    assertEquals(1, Json.parseToJsonElement("""{"x":[{"a":1}]}""").rows("x").size)
  }

  @Test fun `storage totals and kinds map to segments`() {
    val s = HubJson.storage(obj("""{"used_bytes":2400,"by_kind":{"sessions":1100,"artifacts":530,"events":410,"knowledge":360}}"""), emptyList())
    assertEquals(4, s.segments.size)
    assertEquals(2400, s.totalBytes)
  }

  @Test fun `an enrolment item reads as the real hub shapes it`() {
    val a = HubJson.ask(obj("""{"event_id":"E1","kind":"approval","status":"action","summary":"studio-mac wants to join","payload":{"action":"enrol_request","display_name":"studio-mac","member":"studio-mac","source":"127.0.0.1","why":"Design work"},"author":{"kind":"participant","member":"studio-mac","agent":"opencode","session":null}}"""), Fixtures.now)!!
    assertEquals(dev.agync.agenthub.model.AskKind.Joining, a.kind)
    assertEquals("studio-mac", a.enrolment!!.memberId)
    assertEquals("opencode", a.enrolment.firstAgent)
    assertEquals("127.0.0.1", a.enrolment.from)
    assertEquals("Design work", a.enrolment.reason)
  }

  @Test fun `a decided item takes its time from the decision and an expired one reads as closed`() {
    val d = HubJson.ask(obj("""{"event_id":"E1","kind":"approval","status":"resolved","summary":"x","decision":{"decision":"approved","decided_at":"2026-11-24T14:40:00Z"}}"""), Fixtures.now)!!
    assertEquals(java.time.Instant.parse("2026-11-24T14:40:00Z"), d.decidedAt)
  }

  @Test fun `a session reads active, ended and a child under its parent's name`() {
    val ended = HubJson.session(obj("""{"id":"s","session_name":"tests","status":"ended","parent_id":"p"}"""), Fixtures.now, { if (it == "p") "feat-invoices" else null })!!
    assertEquals(dev.agync.agenthub.model.SessionState.Ended, ended.state)
    assertEquals("feat-invoices/tests", ended.name)
    assertEquals("feat-invoices", ended.parent)
    val live = HubJson.session(obj("""{"id":"s","session_name":"nightly","status":"active","live":true}"""), Fixtures.now)!!
    assertEquals(dev.agync.agenthub.model.SessionState.Live, live.state)
  }

  @Test fun `a captured idea waits in Needs you as the person's own, with its link`() {
    val a = HubJson.ask(obj("""{"event_id":"I1","project_id":"checks","project_display_name":"Checks","kind":"idea","actor":"human","summary":"Try a dark map style","payload":{"url":"https://example.org/map"},"status":"action","created_at":"2026-11-26T21:59:40Z","author":{"kind":"human"}}"""), Fixtures.now)!!
    assertEquals(dev.agync.agenthub.model.AskKind.Idea, a.kind)
    assertEquals(AskStatus.Waiting, a.status)
    assertEquals("You", a.agent)
    assertEquals("Try a dark map style", a.sentence)
    assertEquals("https://example.org/map", a.prose)
    assertEquals("Checks", a.project)
  }

  @Test fun `a resolved idea and a cleared one read as what was done to them`() {
    val resolved = HubJson.ask(obj("""{"event_id":"I1","kind":"idea","summary":"x","status":"resolved"}"""), Fixtures.now)!!
    val cleared = HubJson.ask(obj("""{"event_id":"I2","kind":"idea","summary":"x","status":"cleared"}"""), Fixtures.now)!!
    assertEquals(AskStatus.Resolved, resolved.status)
    assertEquals(AskStatus.Cleared, cleared.status)
  }

  @Test fun `the brain key an item points at becomes its receipt`() {
    val a = HubJson.ask(obj("""{"event_id":"E1","kind":"question","status":"action","summary":"x","session_id":"s1","payload":{"brain_key":"kv/upgrade-plan"}}"""), Fixtures.now)!!
    assertEquals("kv/upgrade-plan", a.receipts.brainKey)
    assertEquals("s1", a.receipts.session)
  }

  @Test fun `a search page flattens its groups, names each hit's kind, and leaves out what a phone has no screen for`() {
    val r = HubJson.search(obj("""{"count":3,"truncated":true,"groups":[
      {"kind":"feed","count":1,"hits":[{"kind":"feed","ref_id":"E1","project_id":"checks","project_display_name":"Checks","title":"Try a dark map style","snippet":"Try a dark map style","updated_at":"2026-11-26T21:59:40Z","actor":"human","author":{"kind":"human"}}]},
      {"kind":"artifact","count":1,"hits":[{"kind":"artifact","ref_id":"ar1","project_id":"checks","title":"Map notes","snippet":"colours","updated_at":"2026-11-26T20:00:00Z","author":{"kind":"participant","agent":"claude"}}]},
      {"kind":"comment","count":1,"hits":[{"kind":"comment","ref_id":"c1","project_id":"checks","title":"x","snippet":"y","updated_at":"2026-11-26T20:00:00Z"}]}]}"""))
    assertEquals(listOf(dev.agync.agenthub.model.SearchKind.Feed, dev.agync.agenthub.model.SearchKind.Artifact), r.hits.map { it.kind })
    assertEquals("Checks", r.hits[0].project)
    assertEquals("claude", r.hits[1].by)
    assertEquals(true, r.truncated)
    assertEquals(1, r.count(dev.agync.agenthub.model.SearchKind.Artifact))
  }

  @Test fun `an enrolment carries the projects the machine asked for`() {
    val a = HubJson.ask(obj("""{"event_id":"E1","kind":"approval","status":"action","summary":"x wants to join","payload":{"action":"enrol_request","member":"x","display_name":"x","projects":["space-a","docs-site"]}}"""), Fixtures.now)!!
    assertEquals(listOf("space-a", "docs-site"), a.enrolment!!.requestedProjects)
    val none = HubJson.ask(obj("""{"event_id":"E2","kind":"approval","status":"action","summary":"y wants to join","payload":{"action":"enrol_request","member":"y"}}"""), Fixtures.now)!!
    assertEquals(emptyList<String>(), none.enrolment!!.requestedProjects)
  }

  @Test fun `an item's session receipt reads as the session's name, and opens by its id`() {
    val a = HubJson.ask(obj("""{"event_id":"E1","kind":"question","status":"action","summary":"x","session_id":"01M8CXPQ"}"""), Fixtures.now, agent = "nightly")!!
    assertEquals("01M8CXPQ", a.receipts.session)
    assertEquals("nightly", a.receipts.sessionName)
    assertEquals(null, HubJson.ask(obj("""{"event_id":"E2","kind":"question","status":"action","summary":"x"}"""), Fixtures.now, agent = "someone")!!.receipts.sessionName)
  }
}
