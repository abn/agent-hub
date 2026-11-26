package dev.agync.agenthub.tendril

import dev.agync.agenthub.data.HubException
import java.io.IOException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response

/** A non-2xx answer from the hub, as RFC 9457 problem details. */
class HttpProblem(val status: Int, val code: String?, val reason: String?, val detail: String?) : Exception("HTTP $status ${code.orEmpty()}")

/**
 * REST over a Tendril session: HTTP/2 with prior knowledge on a byte pipe, no credentials (the
 * Tendril peer is the credential). Opens sessions on demand, tells an unpairing from an
 * unreachable hub, and keeps no route logic: Tendril picks the route when the session opens.
 */
class HubTransport(
  private val firstReadMillis: Long = 12_000,
  private val idleMillis: Long = 8_000,
  connectPipe: () -> TendrilPipe,
) {
  /** When the hub last answered, on the monotonic clock. A session idle for longer than [idleMillis] is replaced, not trusted. */
  @Volatile private var lastAnswerNanos = 0L

  private val emptySessions = AtomicInteger()

  @Volatile var unpaired = false
    private set

  /** Set once the hub has answered as active; only then do empty sessions count as an unpairing. */
  @Volatile var heldActive = false

  private val watch =
    object : SessionWatch {
      override fun ended(reason: EndReason, sawData: Boolean) {
        when {
          reason == EndReason.Revoked -> unpaired = true
          sawData -> emptySessions.set(0)
          // Tendril 0.1 sends no close frame to a revoked peer: its session just ends. Three in a
          // row on a hub we hold as active read as an unpairing.
          heldActive && emptySessions.incrementAndGet() >= EMPTY_LIMIT -> unpaired = true
        }
      }
    }

  val http: OkHttpClient =
    OkHttpClient.Builder()
      .protocols(listOf(Protocol.H2_PRIOR_KNOWLEDGE))
      // The session is the only route: no second attempt behind the caller's back (a write could happen twice), and no proxy.
      .retryOnConnectionFailure(false)
      .proxy(java.net.Proxy.NO_PROXY)
      // The host name is a placeholder: nothing is dialled, the session is the connection.
      .dns { listOf(java.net.InetAddress.getLoopbackAddress()) }
      .socketFactory(PipeSocketFactory(connectPipe, watch))
      .connectTimeout(20, TimeUnit.SECONDS)
      .callTimeout(40, TimeUnit.SECONDS)
      .build()

  fun markActive() {
    heldActive = true
    emptySessions.set(0)
  }

  private fun request(method: String, path: String, body: JsonElement?): Request =
    Request.Builder().url("http://hub$path")
      .method(method, if (body != null || method != "GET") (body?.toString() ?: "{}").toRequestBody(JSON_TYPE) else null)
      .header("Accept", "application/json").build()

  /** Runs a request and returns the response body, or throws [HubException]. */
  suspend fun call(method: String, path: String, body: JsonElement? = null, accept: String = "application/json"): String =
    withContext(Dispatchers.IO) {
      if (unpaired) throw HubException.Unpaired()
      // Sessions die quietly while the phone idles, and nothing says so until a request waits on one. After a pause, start fresh.
      val idleFor = System.nanoTime() - lastAnswerNanos
      if (lastAnswerNanos != 0L && idleFor > TimeUnit.MILLISECONDS.toNanos(idleMillis)) http.connectionPool.evictAll()
      try {
        // A read gets a short first try: a session that died while idle gives no error, only silence.
        attempt(if (method == "GET") http.newBuilder().callTimeout(firstReadMillis, TimeUnit.MILLISECONDS).build() else http, method, path, body, accept)
      } catch (e: HttpProblem) {
        if (e.status == 401 && e.reason == "device_unpaired") unpaired = true
        throw e
      } catch (e: IOException) {
        if (unpaired) throw HubException.Unpaired()
        // A session that died while the phone was idle looks alive until a write fails. A read is safe to
        // repeat, so it goes once more over a fresh session; a write is not repeated here.
        if (method == "GET") {
          http.connectionPool.evictAll()
          try {
            return@withContext attempt(http, method, path, body, accept)
          } catch (again: HttpProblem) {
            if (again.status == 401 && again.reason == "device_unpaired") unpaired = true
            throw again
          } catch (again: IOException) {
            throw if (unpaired) HubException.Unpaired() else HubException.Unreachable(again.message ?: "unreachable")
          }
        }
        throw HubException.Unreachable(e.message ?: "unreachable")
      }
    }

  private fun attempt(client: OkHttpClient, method: String, path: String, body: JsonElement?, accept: String): String =
    client.newCall(request(method, path, body).newBuilder().header("Accept", accept).build()).execute().use(::read).also { lastAnswerNanos = System.nanoTime() }

  private fun read(r: Response): String {
    val text = r.body.string()
    if (r.isSuccessful) return text
    val p = runCatching { Json.parseToJsonElement(text).jsonObject }.getOrNull()
    throw HttpProblem(
      r.code, p?.get("code")?.jsonPrimitive?.contentOrNull,
      (p?.get("details") as? JsonObject)?.get("reason")?.jsonPrimitive?.contentOrNull, p?.get("detail")?.jsonPrimitive?.contentOrNull,
    )
  }

  suspend fun json(method: String, path: String, body: JsonElement? = null): JsonElement {
    val text = call(method, path, body)
    return runCatching { Json.parseToJsonElement(text) }.getOrElse { throw HubException.Unreachable("the hub sent something unreadable") }
  }

  fun close() {
    http.dispatcher.executorService.shutdown()
    http.connectionPool.evictAll()
  }

  companion object {
    private val JSON_TYPE = "application/json".toMediaType()
    private const val EMPTY_LIMIT = 3
  }
}
