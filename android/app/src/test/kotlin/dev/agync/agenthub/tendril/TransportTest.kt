package dev.agync.agenthub.tendril

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.HubException
import dev.agync.agenthub.data.Refusal
import java.net.Socket
import java.util.Base64
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import okhttp3.Protocol
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** A Tendril session stood in for by a TCP connection to a local HTTP/2 server, speaking the session's event JSON. */
private class SocketPipe(port: Int) : TendrilPipe {
  private val socket = Socket("127.0.0.1", port)
  private val events = LinkedBlockingQueue<String>()
  val sent = mutableListOf<Int>()

  init {
    Thread {
      val buf = ByteArray(4096)
      try {
        while (true) {
          val n = socket.getInputStream().read(buf)
          if (n < 0) break
          events.put("""{"event":"data","data_b64":"${Base64.getUrlEncoder().withoutPadding().encodeToString(buf.copyOf(n))}"}""")
        }
      } catch (_: Exception) {
      }
      events.put("""{"status":"eof"}""")
    }.also { it.isDaemon = true }.start()
  }

  override fun send(bytes: ByteArray) { sent += bytes.size; socket.getOutputStream().write(bytes); socket.getOutputStream().flush() }

  override fun poll(timeoutMillis: Long): String = events.poll(timeoutMillis, TimeUnit.MILLISECONDS) ?: """{"status":"would_block"}"""

  override fun close() = socket.close()
}

class TransportTest {
  private val server = MockWebServer()
  private val seen = mutableListOf<RecordedRequest>()
  private var respond: (RecordedRequest) -> MockResponse = { MockResponse.Builder().code(404).body("{}").build() }
  private val pipes = mutableListOf<SocketPipe>()

  @Before fun start() {
    server.protocols = listOf(Protocol.H2_PRIOR_KNOWLEDGE)
    server.dispatcher = object : Dispatcher() {
      override fun dispatch(request: RecordedRequest): MockResponse { seen += request; return respond(request) }
    }
    server.start()
  }

  @After fun stop() = server.close()

  private fun json(code: Int = 200, body: String) = MockResponse.Builder().code(code).addHeader("Content-Type", "application/json").body(body).build()

  private fun client(): TendrilHubClient = TendrilHubClient(HubTransport { SocketPipe(server.port).also(pipes::add) }, Fixtures.clock())

  @Test fun `HTTP over the pipe sends no credential and reaches the right path`() = runBlocking {
    respond = { json(body = """{"device_id":"dev_1","name":"Pixel","state":"active","hub":{"name":"home-lab","version":"1.15.0"},"phone_api":1}""") }
    val info = client().info()
    assertEquals("home-lab", info.name)
    assertEquals("1.15.0", info.version)
    val r = seen.single()
    assertEquals("/api/v1/devices/me", r.url.encodedPath)
    assertNull(r.headers["Authorization"])
    assertNull(r.headers["Cookie"])
    assertTrue(pipes.flatMap { it.sent }.all { it <= PipeSocket.MAX_CHUNK })
  }

  @Test fun `a hub with no devices route, or an older phone api, needs an update`() {
    respond = { json(404, """{"code":"not_found"}""") }
    assertThrows(HubException.TooOld::class.java) { runBlocking { client().info() } }
    respond = { json(body = """{"state":"active","hub":{"name":"h","version":"1.2"},"phone_api":0}""") }
    assertThrows(HubException.TooOld::class.java) { runBlocking { client().info() } }
  }

  @Test fun `an unpaired answer is told apart and sticks`() {
    respond = { json(401, """{"code":"unauthenticated","details":{"reason":"device_unpaired"}}""") }
    val c = client()
    assertThrows(HubException.Unpaired::class.java) { runBlocking { c.answer("ev1", "yes") } }
    seen.clear()
    assertThrows(HubException.Unpaired::class.java) { runBlocking { c.projects() } }
    assertTrue("a revoked phone does not retry", seen.isEmpty())
  }

  @Test fun `an answer is posted to its question with an idempotency key, and a retry reuses it`() = runBlocking {
    respond = { json(body = """{"event_id":"x"}""") }
    val c = client()
    c.answer("ev1", "Yes, tonight")
    c.answer("ev1", "Yes, tonight")
    val bodies = seen.map { it.body!!.utf8() }
    assertEquals("/api/v1/questions/ev1/answer", seen[0].url.encodedPath)
    assertTrue(bodies[0].contains("\"body\":\"Yes, tonight\""))
    val key = Regex("\"idempotency_key\":\"(\\w+)\"")
    assertEquals(key.find(bodies[0])!!.groupValues[1], key.find(bodies[1])!!.groupValues[1])
  }

  @Test fun `a decision carries approve or decline and the note`() = runBlocking {
    respond = { json(body = """{"event_id":"x"}""") }
    client().decide("ap1", false, "not tonight")
    val b = seen.single().body!!.utf8()
    assertEquals("/api/v1/approvals/ap1/decision", seen.single().url.encodedPath)
    assertTrue(b.contains("\"decision\":\"decline\"") && b.contains("\"note\":\"not tonight\""))
  }

  @Test fun `a 409 conflict is a refusal that says what was decided, from the hub's own item`() {
    respond = { req ->
      when {
        req.url.encodedPath.endsWith("/answer") -> json(409, """{"status":409,"code":"conflict","detail":"question ev1 was already answered"}""")
        else -> json(body = """{"items":[{"event_id":"ev1","kind":"question","status":"resolved","summary":"Roll?","answer":{"body":"Wait for the weekend","answered_at":"2026-11-24T14:36:00Z"}}]}""")
      }
    }
    val e = assertThrows(HubException.Refused::class.java) { runBlocking { client().answer("ev1", "x") } }
    val r = e.refusal as Refusal.AnsweredElsewhere
    assertEquals("Wait for the weekend", r.answer)
    assertEquals(java.time.Instant.parse("2026-11-24T14:36:00Z"), r.at)
  }

  @Test fun `a 409 that names the deadline is closed at its deadline`() {
    respond = { req ->
      if (req.url.encodedPath.endsWith("/decision")) json(409, """{"status":409,"code":"conflict","detail":"approval ev1 reached its deadline and closed unanswered"}""")
      else json(body = """{"items":[]}""")
    }
    val e = assertThrows(HubException.Refused::class.java) { runBlocking { client().decide("ev1", true, null) } }
    assertTrue(e.refusal is Refusal.ClosedAtDeadline)
  }

  @Test fun `a route the phone is not allowed is not an error state`() {
    respond = { json(403, """{"status":403,"code":"forbidden","details":{"reason":"operator_only"},"detail":"a phone does not reach this"}""") }
    assertThrows(HubException.NotAllowed::class.java) { runBlocking { client().pruneEnded() } }
  }

  @Test fun `now maps the hub's waiting items, joining requests and live sessions`() = runBlocking {
    respond = { req ->
      when (req.url.encodedPath) {
        "/api/v1/devices/me" -> json(body = """{"state":"active","hub":{"name":"home-lab","version":"1.15.0"},"phone_api":1}""")
        "/api/v1/home" -> json(
          body = """{"waiting_items":[
            {"event_id":"EV7HQ2KD","kind":"approval","status":"action","summary":"Run the migration on staging","payload":{"body":"Alters 3 tables."},"session_id":"s1","project_display_name":"billing-svc","created_at":"2026-11-24T14:21:00Z","expires_at":"2026-11-24T18:00:00Z","on_expiry":"decline","author":{"kind":"participant","member":"laptop","agent":"claude"}},
            {"event_id":"EV9MX4TB","kind":"question","status":"waiting","summary":"Upgrade k3s?","payload":{"options":["Yes","No"]},"session_id":"s2","project_display_name":"home-lab","created_at":"2026-11-24T14:08:00Z"},
            {"event_id":"EVJOIN1","kind":"approval","status":"action","summary":"studio-mac wants to join","payload":{"action":"enrol_request","member":"studio-mac","display_name":"studio-mac","why":"Design work","source":"192.168.1.42"},"created_at":"2026-11-24T13:50:00Z"},
            {"event_id":"EVIDEA","kind":"idea","status":"action","summary":"an idea"}],
            "recent":[]}""",
        )
        "/api/v1/sessions/s1" -> json(body = """{"id":"s1","session_name":"feat-invoices"}""")
        "/api/v1/sessions/s2" -> json(body = """{"id":"s2","session_name":"nightly"}""")
        else -> json(body = """{"sessions":[{"id":"s1","session_name":"feat-invoices","status":"active","live":true,"participants":[{"agent":"claude","member":"laptop"}]}]}""")
      }
    }
    val snap = client().now()
    assertEquals(4, snap.asks.size)
    val a = snap.asks.first { it.id == "EV7HQ2KD" }
    assertEquals("feat-invoices", a.agent)
    assertEquals("billing-svc", a.project)
    assertEquals("claude", a.tool)
    assertEquals("laptop", a.machine)
    assertTrue(a.deadline != null)
    assertEquals(listOf("Yes", "No"), snap.asks.first { it.id == "EV9MX4TB" }.answers)
    assertEquals("studio-mac", snap.asks.first { it.id == "EVJOIN1" }.enrolment!!.memberId)
    assertEquals(1, snap.live.size)
    assertEquals(dev.agync.agenthub.model.AskKind.Idea, snap.asks.first { it.id == "EVIDEA" }.kind)
  }

  @Test fun `an unreachable hub is not an unpairing`() {
    val c = TendrilHubClient(HubTransport { throw java.io.IOException("no route") }, Fixtures.clock())
    assertThrows(HubException.Unreachable::class.java) { runBlocking { c.info() } }
  }

  @Test fun `three sessions that end with nothing on a hub held active read as an unpairing`() {
    respond = { json(body = """{"state":"active","hub":{"name":"h","version":"1.15.0"},"phone_api":1}""") }
    val t = HubTransport { SocketPipe(server.port) }
    val c = TendrilHubClient(t, Fixtures.clock())
    runBlocking { c.info() }
    assertTrue(t.heldActive)
    val w = PipeSocketFactory({ throw java.io.IOException("refused") }, object : SessionWatch { override fun ended(reason: EndReason, sawData: Boolean) {} })
    assertTrue(w.createSocket() is PipeSocket)
  }

  @Test fun `a close with the revoked code marks the phone unpaired`() {
    val closing = object : TendrilPipe {
      override fun send(bytes: ByteArray) = Unit
      override fun poll(timeoutMillis: Long) = """{"event":"closed","code":4001}"""
      override fun close() = Unit
    }
    val t = HubTransport { closing }
    val s = PipeSocket({ closing }, object : SessionWatch { override fun ended(reason: EndReason, sawData: Boolean) { if (reason == EndReason.Revoked) { revoked = true } } })
    s.connect(null)
    Thread.sleep(200)
    assertTrue(revoked)
    assertFalse(t.unpaired)
  }

  private var revoked = false

  @Test fun `artifacts are listed by project because the all-artifacts route needs a session`() = runBlocking {
    respond = { req ->
      when (req.url.encodedPath) {
        "/api/v1/projects" -> json(body = """{"projects":[{"id":"checks","display_name":"Checks"}]}""")
        "/api/v1/projects/checks/artifacts" -> json(body = """{"artifacts":[{"id":"a1","title":"Check note","version":2,"protected":false,"size_bytes":7,"updated_at":"2026-11-24T14:00:00Z","comments_count":1,"live_version":null,"author":{"kind":"human"},"actor":"claude"}]}""")
        else -> json(404, "{}")
      }
    }
    val a = client().artifacts().single()
    assertEquals("Check note", a.name)
    assertEquals("Checks", a.project)
    assertEquals(2, a.version)
    assertEquals(false, a.live)
    assertEquals("claude", a.tool)
  }

  @Test fun `a markdown artifact is rendered here with raw html escaped, since the frame route refuses markdown`() = runBlocking {
    respond = { req ->
      when (req.url.encodedPath) {
        "/api/v1/projects" -> json(body = """{"projects":[{"id":"checks","display_name":"Checks"}]}""")
        "/api/v1/projects/checks/artifacts" -> json(body = """{"artifacts":[{"id":"a1","title":"Check note","version":1,"protected":false,"size_bytes":7}]}""")
        "/api/v1/artifacts/a1" -> json(body = """{"title":"Check note","kind":"markdown","protected":false,"envelope":null,"content":"# Hi\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n<script>x()</script>","version":1}""")
        "/api/v1/artifacts/a1/comments" -> json(body = """{"comments":[]}""")
        else -> json(404, "{}")
      }
    }
    val v = client().artifact("a1", null)
    assertTrue(v.html.contains("<h1>Hi</h1>"))
    assertTrue(v.html.contains("<table>"))
    assertTrue(v.html.contains("&lt;script&gt;"))
    assertFalse(v.html.contains("<script>x()"))
    assertTrue(seen.none { it.url.encodedPath.endsWith("/frame") })
  }

  @Test fun `a protected artifact is sealed with its envelope and no page`() = runBlocking {
    respond = { req ->
      when (req.url.encodedPath) {
        "/api/v1/projects" -> json(body = """{"projects":[{"id":"checks","display_name":"Checks"}]}""")
        "/api/v1/projects/checks/artifacts" -> json(body = """{"artifacts":[{"id":"a2","title":"Sealed","version":1,"protected":true,"size_bytes":164}]}""")
        "/api/v1/artifacts/a2" -> json(body = """{"title":"Sealed","kind":"markdown","protected":true,"envelope":{"alg":"AES-256-GCM","kdf":"PBKDF2-HMAC-SHA256","iterations":600000,"iv":"iv","salt":"salt"},"content":"CIPHER","version":1}""")
        "/api/v1/artifacts/a2/comments" -> json(body = """{"comments":[]}""")
        else -> json(404, "{}")
      }
    }
    val v = client().artifact("a2", null)
    assertEquals("CIPHER", v.sealed!!.ciphertext)
    assertEquals(600000, v.sealed.envelope.iterations)
    assertEquals("", v.html)
  }

  @Test fun `storage lists ended sessions only for projects the hub says have prunable ones`() = runBlocking {
    respond = { req ->
      when (req.url.encodedPath) {
        "/api/v1/storage" -> json(body = """{"used_bytes":2400,"by_kind":{"sessions":1100},"projects":[{"project_id":"a","prunable_sessions":1},{"project_id":"b","prunable_sessions":0}]}""")
        "/api/v1/projects" -> json(body = """{"projects":[{"id":"a","display_name":"A"},{"id":"b","display_name":"B"}]}""")
        "/api/v1/sessions" -> json(body = """{"sessions":[{"id":"s9","session_name":"old","status":"ended","brain_bytes":220}]}""")
        else -> json(404, "{}")
      }
    }
    val st = client().storage()
    assertEquals(listOf("old"), st.ended.map { it.name })
    assertEquals("A", st.ended.single().project)
    assertEquals(1, seen.count { it.url.encodedPath == "/api/v1/sessions" })
  }

  @Test fun `projects leave out member spaces and carry bytes and the last write`() = runBlocking {
    respond = { req ->
      when (req.url.encodedPath) {
        "/api/v1/projects" -> json(body = """{"projects":[{"id":"home-lab","display_name":"home-lab","confidential":false,"sessions_live":1,"is_personal":false},{"id":"space-x","display_name":"studio (personal)","is_personal":true}]}""")
        else -> json(body = """{"projects":[{"project_id":"home-lab","events_bytes":10,"session_bytes":20,"artifact_bytes":30,"kb_bytes":40,"last_write":"2026-11-24T14:00:00Z"}]}""")
      }
    }
    val p = client().projects().single()
    assertEquals("home-lab", p.name)
    assertEquals(100, p.bytes)
    assertEquals(1, p.live)
  }

  /** Tendril reports a dead session as an IllegalStateException; OkHttp only copes with IOException, and the crash took the app down. */
  @Test fun `a session that breaks while writing is unreachable, not a crash`() {
    val broken = object : TendrilPipe {
      override fun send(bytes: ByteArray) = throw IllegalStateException("tendril ffi error (-6): transport i/o failed: Broken pipe (os error 32)")
      override fun poll(timeoutMillis: Long) = """{"status":"would_block"}"""
      override fun close() = Unit
    }
    val c = TendrilHubClient(HubTransport { broken }, Fixtures.clock())
    assertThrows(HubException.Unreachable::class.java) { runBlocking { c.info() } }
  }

  @Test fun `a session that cannot be opened is unreachable, not a crash`() {
    val c = TendrilHubClient(HubTransport { throw IllegalStateException("tendril ffi error (-3): no route") }, Fixtures.clock())
    assertThrows(HubException.Unreachable::class.java) { runBlocking { c.info() } }
  }

  @Test fun `an idea is resolved, cleared and replied to on its own routes`() = runBlocking {
    respond = { json(body = """{"id":"x"}""") }
    val c = client()
    c.resolveIdea("I1")
    c.clearIdea("I2")
    c.replyIdea("I1", "Check the contrast first")
    assertEquals(listOf("/api/v1/ideas/I1/resolve", "/api/v1/ideas/I2/clear", "/api/v1/ideas/I1/reply"), seen.map { it.url.encodedPath })
    assertEquals("POST", seen[0].method)
    assertTrue(seen[2].body!!.utf8().contains("\"summary\":\"Check the contrast first\""))
  }

  @Test fun `pruning keeps the undo tokens and undoing sends each one back`() = runBlocking {
    respond = { req ->
      if (req.method == "DELETE") json(body = """{"sessions":[{"session_id":"s1","undo_token":"tok-a"},{"session_id":"s2","undo_token":"tok-b"}],"undo_expires_at":"2026-11-24T14:33:00Z"}""")
      else json(body = """{"ok":true}""")
    }
    val c = client()
    val undo = c.pruneEnded()
    assertEquals(listOf("tok-a", "tok-b"), undo.tokens)
    assertEquals(java.time.Instant.parse("2026-11-24T14:33:00Z"), undo.expiresAt)
    c.undoPrune(undo)
    assertEquals(listOf("DELETE /api/v1/storage/sessions", "POST /api/v1/prune/undo/tok-a", "POST /api/v1/prune/undo/tok-b"), seen.map { it.method + " " + it.url.encodedPath })
  }

  @Test fun `a project's feed is marked seen up to an event`() = runBlocking {
    respond = { json(body = """{"advanced":true}""") }
    client().markFeedSeen("home-lab", "EV9")
    assertEquals("/api/v1/projects/home-lab/feed/seen", seen.single().url.encodedPath)
    assertTrue(seen.single().body!!.utf8().contains("\"event_id\":\"EV9\""))
  }

  @Test fun `one brain entry is read by session and path, and none there is null, not an outage`() = runBlocking {
    respond = { req ->
      if (req.url.queryParameter("path") == "/kv/plan") json(body = """{"path":"/kv/plan","type":"key","size_bytes":9,"content":"drain one","written_at":null}""")
      else json(404, """{"status":404,"code":"not_found","detail":"no brain entry"}""")
    }
    val c = client()
    val e = c.brainEntry("s 1", "/kv/plan")!!
    assertEquals("drain one", e.content)
    assertEquals(9, e.sizeBytes)
    assertEquals("/api/v1/sessions/s%201/brain/entry", seen[0].url.encodedPath)
    assertNull(c.brainEntry("s 1", "/kv/gone"))
  }

  @Test fun `one session is pruned on its own route and keeps its undo token`() = runBlocking {
    respond = { json(body = """{"undo_token":"tok-1","undo_expires_at":"2026-11-24T14:33:00Z"}""") }
    val undo = client().pruneSession("s 1")
    assertEquals("DELETE /api/v1/storage/sessions/s%201", seen.single().method + " " + seen.single().url.encodedPath)
    assertEquals(listOf("tok-1"), undo.tokens)
  }

  @Test fun `a search sends the query encoded, with a limit`() = runBlocking {
    respond = { json(body = """{"count":0,"truncated":false,"groups":[]}""") }
    val r = client().search("dark map & more")
    assertEquals("/api/v1/search", seen.single().url.encodedPath)
    assertEquals("dark map & more", seen.single().url.queryParameter("q"))
    assertEquals("50", seen.single().url.queryParameter("limit"))
    assertTrue(r.hits.isEmpty())
  }

  private fun enrolRespond(): (RecordedRequest) -> MockResponse = { req ->
    when {
      req.url.encodedPath == "/api/v1/inbox" -> json(body = """{"items":[{"event_id":"J1","kind":"approval","status":"action","summary":"studio-mac wants to join","payload":{"action":"enrol_request","member":"studio-mac","display_name":"studio-mac","projects":["space-a","docs-site"]}}]}""")
      req.url.encodedPath.endsWith("/approve") -> json(body = """{"ok":true}""")
      else -> json(404, "{}")
    }
  }

  @Test fun `approving a joining machine sends only requested projects, never a member or share override`() = runBlocking {
    respond = enrolRespond()
    client().admit("J1", true, "a different name", setOf("space-a", "billing-svc"))
    val approve = seen.single { it.url.encodedPath.endsWith("/approve") }
    assertEquals("/api/v1/enrol/studio-mac/approve", approve.url.encodedPath)
    val body = approve.body!!.utf8()
    assertEquals("""{"projects":["space-a"]}""", body)
    assertFalse(body.contains("member") || body.contains("share") || body.contains("display_name"))
  }

  @Test fun `approving with nothing ticked sends no project list`() = runBlocking {
    respond = enrolRespond()
    client().admit("J1", true, "studio-mac", emptySet())
    assertEquals("{}", seen.single { it.url.encodedPath.endsWith("/approve") }.body!!.utf8())
  }

  @Test fun `a phone the hub refuses an approval is told it is for the web app, not that the hub is down`() {
    respond = { req ->
      if (req.url.encodedPath.endsWith("/approve")) json(403, """{"status":403,"code":"forbidden","details":{"reason":"operator_only"},"detail":"approve from the web app"}""") else enrolRespond()(req)
    }
    assertThrows(HubException.NotAllowed::class.java) { runBlocking { client().admit("J1", true, "studio-mac", emptySet()) } }
  }

  @Test fun `the hub's web address is read from the device route and only if a browser may be sent there`() = runBlocking {
    fun me(url: String) = json(body = """{"state":"active","hub":{"name":"home-lab","version":"1.15.0","web_url":"$url"},"phone_api":1}""")
    respond = { me("https://hub.example/") }
    assertEquals("https://hub.example", client().info().webUrl)
    respond = { me("javascript:alert(1)") }
    assertNull(client().info().webUrl)
    respond = { me("https://user:pw@hub.example/") }
    assertNull(client().info().webUrl)
  }

  /** A session that died while the phone was idle looks alive until the next write fails; the read goes on over a new one. */
  @Test fun `a read over a session that died while idle is retried once on a new session`() = runBlocking {
    respond = { json(body = """{"state":"active","hub":{"name":"home-lab","version":"1.15.0"},"phone_api":1}""") }
    var opened = 0
    var first: SocketPipe? = null
    val deadAfterFirst = object : TendrilPipe {
      override fun send(bytes: ByteArray) {
        if (first != null && opened == 1 && dead) throw IllegalStateException("tendril ffi error (-6): transport i/o failed: Broken pipe (os error 32)")
        first!!.send(bytes)
      }

      var dead = false

      override fun poll(timeoutMillis: Long) = if (dead) """{"status":"would_block"}""" else first!!.poll(timeoutMillis)

      override fun close() { first?.close() }
    }
    val c = TendrilHubClient(
      HubTransport(firstReadMillis = 1_000) {
        opened++
        if (opened == 1) { first = SocketPipe(server.port); deadAfterFirst } else SocketPipe(server.port).also(pipes::add)
      },
      Fixtures.clock(),
    )
    assertEquals("home-lab", c.info().name)
    deadAfterFirst.dead = true
    assertEquals("home-lab", c.info().name)
    assertEquals(2, opened)
  }

  @Test fun `after a pause the next request starts on a new session instead of waiting on a dead one`() = runBlocking {
    respond = { json(body = """{"state":"active","hub":{"name":"home-lab","version":"1.15.0"},"phone_api":1}""") }
    var opened = 0
    var first: SocketPipe? = null
    var dead = false
    val dying = object : TendrilPipe {
      override fun send(bytes: ByteArray) { if (dead) throw IllegalStateException("Broken pipe") else first!!.send(bytes) }

      override fun poll(timeoutMillis: Long) = if (dead) """{"status":"would_block"}""" else first!!.poll(timeoutMillis)

      override fun close() { first?.close() }
    }
    val c = TendrilHubClient(
      HubTransport(firstReadMillis = 30_000, idleMillis = 50) {
        opened++
        if (opened == 1) { first = SocketPipe(server.port); dying } else SocketPipe(server.port).also(pipes::add)
      },
      Fixtures.clock(),
    )
    c.info()
    dead = true
    Thread.sleep(120)
    val started = System.nanoTime()
    assertEquals("home-lab", c.info().name)
    assertEquals(2, opened)
    assertTrue("took ${(System.nanoTime() - started) / 1_000_000} ms", (System.nanoTime() - started) < 3_000_000_000L)
  }

  @Test fun `a 4xx is a call the hub rejected and a 5xx its own trouble, and neither reads as unreachable`() {
    for (code in listOf(400, 404, 422)) {
      respond = { json(code, """{"status":$code,"code":"invalid","detail":"nope"}""") }
      val e = assertThrows(HubException.Rejected::class.java) { runBlocking { client().answer("ev1", "x") } }
      assertEquals(code, e.status)
    }
    respond = { json(500, """{"status":500,"code":"internal"}""") }
    assertEquals(500, assertThrows(HubException.ServerError::class.java) { runBlocking { client().answer("ev1", "x") } }.status)
  }

  @Test fun `an ask the hub does not have is a rejection, not an outage`() {
    respond = { json(body = """{"items":[]}""") }
    assertThrows(HubException.Rejected::class.java) { runBlocking { client().ask("gone") } }
  }

  @Test fun `ids are encoded in the paths, and requested projects go as the hub named them`() = runBlocking {
    respond = { req ->
      when {
        req.url.encodedPath == "/api/v1/inbox" -> json(body = """{"items":[{"event_id":"J 1","kind":"approval","status":"action","summary":"x wants to join","payload":{"action":"enrol_request","member":"a/b","display_name":"x","projects":["Space A"]}}]}""")
        else -> json(body = """{"ok":true}""")
      }
    }
    val c = client()
    c.admit("J 1", true, "x", setOf("Space A"))
    c.answer("q/1", "yes")
    c.decide("a?b", true, null)
    val paths = seen.map { it.url.encodedPath }
    assertTrue(paths.toString(), "/api/v1/enrol/a%2Fb/approve" in paths)
    assertTrue(paths.toString(), "/api/v1/questions/q%2F1/answer" in paths)
    assertTrue(paths.toString(), "/api/v1/approvals/a%3Fb/decision" in paths)
    assertTrue(seen.first { it.url.encodedPath.endsWith("/approve") }.body!!.utf8().contains("\"Space A\""))
  }

  @Test fun `a refresh asks the hub who it is once, and a session it no longer has is not asked about again`() = runBlocking {
    respond = { req ->
      when (req.url.encodedPath) {
        "/api/v1/devices/me" -> json(body = """{"state":"active","hub":{"name":"home-lab","version":"1.15.0"},"phone_api":1}""")
        "/api/v1/home" -> json(body = """{"waiting_items":[{"event_id":"E1","kind":"question","status":"action","summary":"x","session_id":"gone"}],"recent":[]}""")
        "/api/v1/sessions/gone" -> json(404, "{}")
        else -> json(body = """{"sessions":[]}""")
      }
    }
    val rig = Fixtures.Rig()
    val repo = dev.agync.agenthub.data.HubRepository(Fixtures.hub, client(), rig.store, rig.clock)
    repo.refresh()
    repo.refresh()
    assertEquals(2, seen.count { it.url.encodedPath == "/api/v1/devices/me" })
    assertEquals(1, seen.count { it.url.encodedPath == "/api/v1/sessions/gone" })
  }

  @Test fun `a stream that goes silent is dropped and reconnects over a new session, and ticks again`() = runBlocking {
    var opens = 0
    respond = { req ->
      if (req.url.encodedPath == "/api/v1/stream") {
        opens++
        if (opens == 1) MockResponse.Builder().code(200).addHeader("Content-Type", "text/event-stream").body("data: 1\n\n").bodyDelay(30, TimeUnit.SECONDS).build()
        else MockResponse.Builder().code(200).addHeader("Content-Type", "text/event-stream").body("data: 2\n\n").build()
      } else json(404, "{}")
    }
    val c = TendrilHubClient(HubTransport { SocketPipe(server.port).also(pipes::add) }, Fixtures.clock(), retryMillis = 20, maxRetryMillis = 40, streamReadMillis = 300)
    val ticks = kotlinx.coroutines.withTimeoutOrNull(6_000) { c.ticks.take(3).toList() }
    assertTrue("only ${ticks?.size} ticks, the silent stream was never dropped", ticks != null && ticks.size == 3)
    assertTrue("a second stream was opened", opens >= 2)
    assertTrue("the reconnect used its own session", pipes.size >= 2)
  }

  /** A session that died while idle breaks the next write. A read may go again; a write must not be sent behind the caller's back. */
  @Test fun `a write over a session that has just died is not sent again on another session`() {
    respond = { req -> if (req.url.encodedPath == "/api/v1/devices/me") json(body = """{"state":"active","hub":{"name":"home-lab","version":"1.15.0"},"phone_api":1}""") else json(body = """{"ok":true}""") }
    var opened = 0
    val broken = java.util.concurrent.atomic.AtomicBoolean(false)
    val c = TendrilHubClient(
      HubTransport {
        opened++
        val real = SocketPipe(server.port).also(pipes::add)
        if (opened > 1) real
        else object : TendrilPipe {
          override fun send(bytes: ByteArray) {
            if (broken.get()) throw IllegalStateException("tendril ffi error (-6): Broken pipe")
            real.send(bytes)
          }

          override fun poll(timeoutMillis: Long): String = if (broken.get()) """{"status":"would_block"}""" else real.poll(timeoutMillis)

          override fun close() = real.close()
        }
      },
      Fixtures.clock(),
    )
    runBlocking { c.info() }
    broken.set(true)
    assertThrows(HubException.Unreachable::class.java) { runBlocking { c.answer("ev1", "yes") } }
    assertEquals("a second session was opened to send the write again", 1, opened)
    assertEquals(0, seen.count { it.method == "POST" })
  }
}
