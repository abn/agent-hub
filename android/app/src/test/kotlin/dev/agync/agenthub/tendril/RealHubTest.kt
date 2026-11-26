package dev.agync.agenthub.tendril

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.HubException
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.pairing.Confirmation
import dev.tendril.TendrilEndpoint
import java.io.File
import java.net.ServerSocket
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeTrue
import org.junit.Test

/**
 * The app's transport against the real hub binary, with a real Tendril initiator: pair, confirm,
 * `/devices/me`, `/home`, a decision, and unpair. It runs only when `AGENT_HUB_BIN` names a hub
 * binary and the host build of Tendril's JNI library is on `java.library.path` (`make
 * android/test-hub` arranges both); otherwise it is skipped. The hub runs on a free port with a
 * scratch data directory under the build tree, and is stopped by its own handle.
 */
class RealHubTest {
  private val binary = System.getenv("AGENT_HUB_BIN")?.takeIf { File(it).canExecute() }
  private val json = Json { ignoreUnknownKeys = true }
  private val http = OkHttpClient()
  private val jsonType = "application/json".toMediaType()

  private fun freePort(): Int = ServerSocket(0).use { it.localPort }

  private class Hub(val process: Process, val port: Int, val phonesPort: Int, val data: File) {
    fun admin(client: OkHttpClient, method: String, path: String, body: String? = null, sameOrigin: Boolean = false): Pair<Int, String> {
      val req = Request.Builder().url("http://127.0.0.1:$port$path").header("Authorization", "Bearer token")
        .apply { if (sameOrigin) header("Sec-Fetch-Site", "same-origin") }
        .method(method, if (method == "GET") null else (body ?: "{}").toRequestBody("application/json".toMediaType())).build()
      return client.newCall(req).execute().use { it.code to it.body.string() }
    }
  }

  private fun startHub(): Hub {
    val root = File("build/real-hub").also { it.mkdirs() }
    val data = File(root, "data-" + System.nanoTime()).also { it.mkdirs() }
    val port = freePort()
    val phones = freePort()
    require(port != 47318 && port != 47443 && phones != 47318 && phones != 47443)
    val pb = ProcessBuilder(binary, "serve").directory(root)
    pb.environment().putAll(
      mapOf(
        "HUB_DATA_DIR" to data.absolutePath, "HUB_BIND" to "127.0.0.1:$port", "HUB_ADMIN_TOKEN" to "token", "HUB_NODE_NAME" to "local",
        "HUB_PHONES_LISTEN" to "127.0.0.1:$phones", "HUB_PHONES_RELAY" to "off",
      ),
    )
    pb.redirectErrorStream(true).redirectOutput(File(root, "hub-$port.log"))
    val hub = Hub(pb.start(), port, phones, data)
    val until = System.currentTimeMillis() + 30_000
    while (System.currentTimeMillis() < until) {
      if (runCatching { hub.admin(http, "GET", "/api/v1/home").first == 200 }.getOrDefault(false)) return hub
      Thread.sleep(300)
    }
    hub.process.destroy()
    fail("the hub did not start")
    error("unreachable")
  }

  private fun obj(s: String): JsonObject = json.parseToJsonElement(s).jsonObject

  @Test fun aPhonePairsThroughTendrilAndTheAppsClientReadsAndDecidesAndIsUnpaired() {
    assumeTrue("AGENT_HUB_BIN names no hub binary", binary != null)
    val hub = startHub()
    val endpoint = TendrilEndpoint.create()
    try {
      // The operator's side: Add a phone issues a code and its signed URI (from the hub's own web app).
      val (status, issued) = hub.admin(http, "POST", "/api/v1/devices/pairing", sameOrigin = true)
      assertEquals(issued, 200, status)
      val uri = obj(issued)["uri"]!!.jsonPrimitive.content

      // The phone's side, through the app's own pairing code and Tendril's real endpoint.
      val ops = LiveOps(endpoint)
      val pairing = TendrilPairing(ops, pollEvery = java.time.Duration.ofMillis(300))
      val offer = pairing.read(uri)!!
      assertEquals("local", offer.hubName)
      assertNotNull("the URI pins the hub by its id", offer.pinnedHostId)
      val handshake = runBlocking { pairing.pair(offer, "Test phone") }
      assertEquals("a pinned scan needs no local confirm", false, handshake.needsLocalConfirm)
      assertEquals(offer.pinnedHostId, handshake.hubId)

      // The hub holds the phone pending; the operator compares the check string and confirms it.
      val devices = obj(hub.admin(http, "GET", "/api/v1/devices", sameOrigin = true).second)["devices"]!!.jsonArray.map { it.jsonObject }
      val pending = devices.single { it["state"]!!.jsonPrimitive.content == "pending" }
      assertEquals(handshake.checkString, pending["check_string"]!!.jsonPrimitive.content)
      runBlocking { pairing.confirm(handshake) }
      val id = pending["id"]!!.jsonPrimitive.content
      val confirmed = hub.admin(http, "POST", "/api/v1/devices/$id/confirm", """{"check_string":"${handshake.checkString}"}""", sameOrigin = true)
      assertEquals(confirmed.second, 200, confirmed.first)
      assertEquals(Confirmation.Confirmed, runBlocking { pairing.awaitConfirmation(handshake) })
      runBlocking { pairing.adopt(handshake) }

      // The app's client over the real session.
      val client = TendrilHubClient(HubTransport { SessionPipe(endpoint.connect(targetHostId = handshake.hubId)) }, Fixtures.clock())
      val info = runBlocking { client.info() }
      assertEquals("local", info.name)
      assertTrue(info.version.isNotEmpty())

      // A joining machine asks; the phone sees it as a joining ask and approves it with no token of its own.
      val enrol = http.newCall(
        Request.Builder().url("http://127.0.0.1:${hub.port}/api/v1/enrol")
          .post("""{"member":"studio-mac","display_name":"studio-mac","why":"Design work","agent":"opencode","os":"macOS 15","host":"studio-mac"}""".toRequestBody(jsonType)).build(),
      ).execute().use { it.code }
      assertEquals(202, enrol)
      val snap = runBlocking { client.now() }
      val ask = snap.asks.single { it.kind == AskKind.Joining }
      assertEquals("studio-mac", ask.enrolment!!.memberId)
      assertEquals("Design work", ask.enrolment.reason)
      runBlocking { client.admit(ask.id, true, "studio-mac", emptySet()) }
      val members = obj(hub.admin(http, "GET", "/api/v1/members").second)["members"]!!.jsonArray.map { it.jsonObject }
      assertEquals("active", members.single { it["id"]!!.jsonPrimitive.content == "studio-mac" }["state"]!!.jsonPrimitive.content)

      // The other reads the app makes, against the real handlers.
      assertTrue(runBlocking { client.projects() }.none { it.name.contains("personal") })
      assertTrue(runBlocking { client.storage() }.totalBytes > 0)
      assertTrue(runBlocking { client.members() }.any { it.name == "studio-mac" && !it.pending })

      // A route a phone may not reach is refused as operator_only and is not an error state.
      val refused = runCatching { runBlocking { client.pruneEnded() } }
      assertTrue("prune is allowed; this must not throw NotAllowed", refused.exceptionOrNull() !is HubException.NotAllowed)

      // Unpairing: the hub closes the session; the app's next call is told apart as unpaired.
      assertEquals(200, hub.admin(http, "DELETE", "/api/v1/devices/$id", sameOrigin = true).first)
      var unpaired = false
      for (i in 0 until 20) {
        try {
          runBlocking { client.info() }
        } catch (e: HubException.Unpaired) {
          unpaired = true
          break
        } catch (e: HubException) {
          // A session that ended on the revocation can read as unreachable until the next one is refused.
        }
        Thread.sleep(300)
      }
      assertTrue("the unpaired phone is told apart", unpaired)
    } finally {
      endpoint.close()
      hub.process.destroy()
      hub.process.waitFor(10, TimeUnit.SECONDS)
    }
  }

  /** Pairs [endpoint] with [hub] the way the first test does, and returns the app's client and the device id. */
  private fun pairClient(hub: Hub, endpoint: TendrilEndpoint): Pair<TendrilHubClient, String> {
    val issued = hub.admin(http, "POST", "/api/v1/devices/pairing", sameOrigin = true)
    assertEquals(issued.second, 200, issued.first)
    val pairing = TendrilPairing(LiveOps(endpoint), pollEvery = java.time.Duration.ofMillis(300))
    val handshake = runBlocking { pairing.pair(pairing.read(obj(issued.second)["uri"]!!.jsonPrimitive.content)!!, "Test phone") }
    val pending = obj(hub.admin(http, "GET", "/api/v1/devices", sameOrigin = true).second)["devices"]!!.jsonArray.map { it.jsonObject }.single { it["state"]!!.jsonPrimitive.content == "pending" }
    val id = pending["id"]!!.jsonPrimitive.content
    assertEquals(200, hub.admin(http, "POST", "/api/v1/devices/$id/confirm", """{"check_string":"${handshake.checkString}"}""", sameOrigin = true).first)
    assertEquals(Confirmation.Confirmed, runBlocking { pairing.awaitConfirmation(handshake) })
    runBlocking { pairing.adopt(handshake) }
    return TendrilHubClient(HubTransport { SessionPipe(endpoint.connect(targetHostId = handshake.hubId)) }, Fixtures.clock()) to id
  }

  @Test fun aTypedThreeGroupCodeAndAddressPairAgainstTheRealHub() {
    assumeTrue("AGENT_HUB_BIN names no hub binary", binary != null)
    val hub = startHub()
    val endpoint = TendrilEndpoint.create()
    try {
      val issued = obj(hub.admin(http, "POST", "/api/v1/devices/pairing", sameOrigin = true).second)
      val code = issued["code"]!!.jsonPrimitive.content
      assertTrue("the hub issues three groups: $code", Regex("TND-[A-Z0-9]{4}-[A-Z0-9]{4}-[A-Z0-9]{4}").matches(code))
      val pairing = TendrilPairing(LiveOps(endpoint), pollEvery = java.time.Duration.ofMillis(300))
      val offer = pairing.readTyped(code, "127.0.0.1:${hub.phonesPort}")!!
      val handshake = runBlocking { pairing.pair(offer, "Typed phone") }
      assertTrue("a typed pairing has no pin, so the person confirms the check string", handshake.needsLocalConfirm)
      val pending = obj(hub.admin(http, "GET", "/api/v1/devices", sameOrigin = true).second)["devices"]!!.jsonArray.map { it.jsonObject }.single { it["state"]!!.jsonPrimitive.content == "pending" }
      assertEquals(handshake.checkString, pending["check_string"]!!.jsonPrimitive.content)
      runBlocking { pairing.confirm(handshake) }
      val id = pending["id"]!!.jsonPrimitive.content
      assertEquals(200, hub.admin(http, "POST", "/api/v1/devices/$id/confirm", """{"check_string":"${handshake.checkString}"}""", sameOrigin = true).first)
      assertEquals(Confirmation.Confirmed, runBlocking { pairing.awaitConfirmation(handshake) })
    } finally {
      endpoint.close()
      hub.process.destroy()
      hub.process.waitFor(10, TimeUnit.SECONDS)
    }
  }

  @Test fun anIdeaIsResolvedAndNotedAndAFeedIsMarkedSeenAgainstTheRealHub() {
    assumeTrue("AGENT_HUB_BIN names no hub binary", binary != null)
    val hub = startHub()
    val endpoint = TendrilEndpoint.create()
    try {
      val (client, _) = pairClient(hub, endpoint)
      val project = "checks"
      assertEquals(200, hub.admin(http, "POST", "/api/v1/projects", """{"id":"checks","display_name":"Checks"}""", sameOrigin = true).first)

      // The person captures an idea on the phone; it waits in Needs you as their own.
      runBlocking { client.captureIdea(project, "Try a dark map style", "https://example.org/map") }
      val idea = runBlocking { client.now() }.asks.single { it.kind == AskKind.Idea }
      assertEquals("Try a dark map style", idea.sentence)
      assertEquals("You", idea.agent)
      assertEquals("https://example.org/map", idea.prose)

      // A note keeps it open; resolving takes it out of Needs you and the hub reports it resolved.
      runBlocking { client.replyIdea(idea.id, "Check the contrast first") }
      assertTrue(runBlocking { client.now() }.asks.any { it.id == idea.id && it.waiting })
      runBlocking { client.resolveIdea(idea.id) }
      assertTrue(runBlocking { client.now() }.asks.none { it.id == idea.id })
      assertEquals(dev.agync.agenthub.model.AskStatus.Resolved, runBlocking { client.ask(idea.id) }.status)
      runBlocking { client.resolveIdea(idea.id) }

      // The hub's search finds the idea in the feed.
      val found = runBlocking { client.search("dark") }
      assertTrue(found.hits.any { it.kind == dev.agync.agenthub.model.SearchKind.Feed && it.title == "Try a dark map style" && it.project == "Checks" })

      // Pruning with nothing ended is an empty undo, not an error.
      assertTrue(runBlocking { client.pruneEnded() }.tokens.isEmpty())

      // Opening the project's feed moves the hub's cursor to its newest event.
      val feed = runBlocking { client.feed(project) }
      runBlocking { client.markFeedSeen(project, feed.maxByOrNull { it.at }!!.id) }
    } finally {
      endpoint.close()
      hub.process.destroy()
      hub.process.waitFor(10, TimeUnit.SECONDS)
    }
  }
}

/** [TendrilOps] over a live endpoint: the real calls the app makes, with the state directory left to the caller. */
class LiveOps(private val endpoint: TendrilEndpoint) : TendrilOps {
  override fun pairUri(uri: String, deviceName: String): String = endpoint.pairUri(uri, deviceName = deviceName)

  override fun pairInitiate(endpoint: String, code: String, deviceName: String): String = this.endpoint.pairInitiate(endpoint = endpoint, code = code, deviceName = deviceName)

  override fun confirmPeer(peerId: String, sas: String?) = endpoint.confirmPeer(peerId, sas)

  override fun revokePeer(peerId: String) = endpoint.revokePeer(peerId)

  override fun setPeerRoutes(peerId: String, routes: List<String>) = endpoint.setPeerRoutes(peerId, routes)

  override fun standing(hostId: String): Standing {
    val t = HubTransport { SessionPipe(endpoint.connect(targetHostId = hostId)) }
    return try {
      if (runBlocking { t.json("GET", "/api/v1/devices/me") }.let { (it as JsonObject)["state"]?.jsonPrimitive?.content == "active" }) Standing.Active else Standing.Pending
    } catch (_: dev.agync.agenthub.data.HubException.Unpaired) {
      Standing.Refused
    } catch (_: Exception) {
      Standing.Pending
    } finally {
      t.close()
    }
  }

  override val stateDir: String? = null

  override fun adopt() = Unit

  override fun discard() = Unit
}
