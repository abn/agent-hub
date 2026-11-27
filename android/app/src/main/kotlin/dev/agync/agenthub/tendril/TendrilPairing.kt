package dev.agync.agenthub.tendril

import dev.agync.agenthub.pairing.Confirmation
import dev.agync.agenthub.pairing.Handshake
import dev.agync.agenthub.pairing.PairOffer
import dev.agync.agenthub.pairing.Pairing
import dev.agync.agenthub.pairing.PairingException
import dev.tendril.TendrilEndpoint
import java.net.URLDecoder
import java.time.Clock
import java.time.Duration
import java.time.Instant
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** The calls of Tendril's endpoint that pairing uses, so the trust decisions are testable without the native library. */
enum class Standing { Pending, Active, Refused }

interface TendrilOps {
  fun pairUri(uri: String, deviceName: String): String

  fun pairInitiate(endpoint: String, code: String, deviceName: String): String

  fun confirmPeer(peerId: String, sas: String?)

  fun revokePeer(peerId: String)

  fun setPeerRoutes(peerId: String, routes: List<String>)

  /** Where this phone stands with the hub: waiting on the operator, confirmed by them, or turned away. */
  fun standing(hostId: String): Standing

  /** Where the pairing in progress keeps its identity. */
  val stateDir: String?

  /** The hub is registered: its state directory is no longer this attempt's to throw away. */
  fun adopt()

  /** Throws away the pairing in progress, unless it was adopted or its directory belongs to a paired hub. */
  fun discard()
}

/**
 * One sealed state directory (one Tendril endpoint) per paired hub. A pairing attempt gets a fresh
 * one; [adopt] keeps it for the hub it paired with and [delete] removes it, because Tendril never
 * lets a revoked key pair with the same hub again.
 */
class EndpointStore(private val root: java.io.File) {
  private val open = HashMap<String, TendrilEndpoint>()

  @Synchronized
  fun fresh(): Pair<String, TendrilEndpoint> {
    val name = "e" + java.util.UUID.randomUUID().toString().replace("-", "").take(16)
    val ep = TendrilEndpoint.create(java.io.File(root, name).also { it.mkdirs() }.absolutePath)
    open[name] = ep
    return name to ep
  }

  @Synchronized
  fun open(name: String): TendrilEndpoint =
    open.getOrPut(name) { TendrilEndpoint.create(java.io.File(root, name).absolutePath) }

  /** Deletes every state directory not in [keep]: attempts a killed process left behind. */
  @Synchronized
  fun sweep(keep: Set<String>) {
    root.listFiles()?.filter { it.isDirectory && it.name !in keep && it.name !in open.keys }?.forEach { it.deleteRecursively() }
  }

  @Synchronized
  fun delete(name: String) {
    open.remove(name)?.let { runCatching { it.close() } }
    java.io.File(root, name).deleteRecursively()
  }
}

/**
 * The state directory of the pairing in progress. It is the attempt's to delete only until the hub
 * is registered ([adopt]), and never while a paired hub owns it ([owned]): it is that hub's sealed
 * identity, and Tendril will not pair a lost key again.
 */
class AttemptDirs(private val create: () -> String, private val delete: (String) -> Unit, private val owned: (String) -> Boolean) {
  var current: String? = null
    private set

  /** Starts a new attempt, throwing away an unadopted one. */
  fun begin(): String {
    discard()
    return create().also { current = it }
  }

  /** The hub is registered; the directory is no longer the attempt's. */
  fun adopt() {
    current = null
  }

  fun discard() {
    current?.takeIf { !owned(it) }?.let(delete)
    current = null
  }
}

class EndpointOps(private val store: EndpointStore, owned: (String) -> Boolean = { false }) : TendrilOps {
  private val dirs = AttemptDirs({ store.fresh().first }, store::delete, owned)

  override val stateDir: String? get() = dirs.current

  private fun begin(): TendrilEndpoint = store.open(dirs.begin())

  private fun endpoint(): TendrilEndpoint = store.open(dirs.current ?: error("no pairing in progress"))

  override fun pairUri(uri: String, deviceName: String): String = begin().pairUri(uri, deviceName = deviceName)

  override fun pairInitiate(endpoint: String, code: String, deviceName: String): String =
    begin().pairInitiate(endpoint = endpoint, code = code, deviceName = deviceName)

  override fun confirmPeer(peerId: String, sas: String?) = endpoint().confirmPeer(peerId, sas)

  override fun revokePeer(peerId: String) = endpoint().revokePeer(peerId)

  override fun setPeerRoutes(peerId: String, routes: List<String>) = endpoint().setPeerRoutes(peerId, routes)

  /** One client for the whole wait, so each poll does not open a session of its own. */
  private var probe: HubTransport? = null

  @Synchronized
  private fun probeFor(hostId: String): HubTransport =
    probe ?: run {
      val ep = store.open(dirs.current ?: error("no pairing in progress"))
      HubTransport { SessionPipe(ep.connect(targetHostId = hostId)) }.also { probe = it }
    }

  @Synchronized
  private fun closeProbe() {
    probe?.close()
    probe = null
  }

  override fun standing(hostId: String): Standing {
    if (dirs.current == null) return Standing.Pending
    val transport = probeFor(hostId)
    return try {
      when (kotlinx.coroutines.runBlocking { transport.json("GET", "/api/v1/devices/me") }.obj()?.str("state")) {
        "active" -> Standing.Active
        // The hub dropped the pending row: the operator refused this phone.
        "revoked", "refused" -> Standing.Refused
        else -> Standing.Pending
      }
    } catch (_: dev.agync.agenthub.data.HubException.Unpaired) {
      Standing.Refused
    } catch (e: Exception) {
      // Not reachable yet: the caller backs off and asks again, over a fresh session.
      android.util.Log.w("AgentHub", "waiting for the hub to confirm: ${e.message}")
      transport.http.connectionPool.evictAll()
      Standing.Pending
    }
  }

  override fun adopt() { closeProbe(); dirs.adopt() }

  override fun discard() { closeProbe(); dirs.discard() }
}

/**
 * Pairing through Tendril's Kotlin binding, as it ships. Keys live in Tendril's sealed,
 * Keystore-wrapped state; this class holds none. Routes are Tendril's to choose.
 *
 * Trust: a scanned code pins the hub's host id and key, so a hub that answers as anyone else is
 * dropped before anything is confirmed. A typed code pins nothing, so [pair] confirms nothing:
 * the peer stays pending until [confirm], which the person triggers after comparing the check
 * string on both screens.
 */
class TendrilPairing(
  private val ops: TendrilOps,
  private val clock: Clock = Clock.systemDefaultZone(),
  private val confirmTimeout: Duration = Duration.ofMinutes(15),
  private val pollEvery: Duration = Duration.ofSeconds(2),
  private val slowAfter: Duration = Duration.ofMinutes(5),
  private val slowPollEvery: Duration = Duration.ofSeconds(5),
  private val io: CoroutineDispatcher = Dispatchers.IO,
) : Pairing {
  override val confirmWindow: Duration get() = confirmTimeout

  override fun read(raw: String): PairOffer? {
    // The exact scanned string goes to Tendril: not trimmed, decoded or reordered, since the URI carries a signature.
    val s = raw
    if (!s.startsWith("tendril://pair?")) return null
    // `host` is the hub's name, shown before anything is dialled. `id` is the host id the URI pins:
    // the hub that answers must be that one. A URI with no `id` pins nothing and is treated like a typed code.
    val q = queryOf(s)
    return PairOffer(raw = s, hubName = q["host"]?.takeIf { it.isNotBlank() }, pinnedHostId = q["id"]?.takeIf { it.isNotBlank() })
  }

  override fun readTyped(code: String, address: String): PairOffer? {
    val c = code.trim().uppercase()
    if (!CODE.matches(c) || address.isBlank()) return null
    return PairOffer(raw = c, codeLabel = c, address = address.trim(), typed = true)
  }

  override suspend fun pair(offer: PairOffer, deviceName: String): Handshake =
    withContext(Dispatchers.IO) {
      val json =
        try {
          if (offer.typed) ops.pairInitiate(offer.address.orEmpty(), offer.raw, deviceName) else ops.pairUri(offer.raw, deviceName)
        } catch (e: Exception) {
          throw map(e)
        }
      val r = JSONObject(json)
      val hostId = r.getString("host_id")
      val pinned = !offer.typed && offer.pinnedHostId != null
      if (pinned && (r.optBoolean("host_confirmed", false).not() || hostId != offer.pinnedHostId)) {
        // Fail closed: the answer is not the hub the code names, or Tendril did not verify the pin.
        runCatching { ops.revokePeer(hostId) }
        ops.discard()
        throw PairingException.KeyMismatch()
      }
      Handshake(
        hostId, offer.hubName ?: "hub", r.optString("sas"), viaOf(r.optString("provider")), needsLocalConfirm = !pinned,
        stateDir = ops.stateDir, address = offer.address.takeIf { offer.typed },
      )
    }

  override suspend fun confirm(handshake: Handshake) {
    // A pinned scan is already confirmed on this side: the URI's key pin was verified in the handshake
    // and the hub holds the phone pending until the operator confirms. Only a typed code, which pins
    // nothing, has a pending peer here for the person to confirm.
    if (!handshake.needsLocalConfirm) return
    withContext(Dispatchers.IO) {
      try {
        ops.confirmPeer(handshake.hubId, handshake.checkString.takeIf { it.isNotEmpty() })
        // A typed pairing stores no signed routes: keep the address it was claimed at until the hub announces its own.
        handshake.address?.let { ops.setPeerRoutes(handshake.hubId, listOf(it)) }
      } catch (e: Exception) {
        throw map(e)
      }
    }
  }

  override suspend fun adopt(handshake: Handshake) {
    ops.adopt()
  }

  override suspend fun abort(handshake: Handshake) {
    withContext(Dispatchers.IO) {
      runCatching { ops.revokePeer(handshake.hubId) }
      ops.discard()
    }
  }

  override suspend fun awaitConfirmation(handshake: Handshake, checkNow: ReceiveChannel<Unit>?, onChecked: (Instant) -> Unit): Confirmation {
    val start = clock.instant()
    val deadline = start.plus(confirmTimeout)
    // Steady and short: the operator is usually on the hub already, and each ask is a small request.
    // After the first few minutes the person has likely gone away, so it eases to a longer interval.
    while (clock.instant().isBefore(deadline)) {
      val standing = withContext(io) { ops.standing(handshake.hubId) }
      val now = clock.instant()
      onChecked(now)
      when (standing) {
        Standing.Active -> return Confirmation.Confirmed
        Standing.Refused -> return Confirmation.Refused
        Standing.Pending -> Unit
      }
      val every = if (Duration.between(start, now) < slowAfter) pollEvery else slowPollEvery
      // A nudge cuts the wait short: the app came back to the front.
      withTimeoutOrNull(every.toMillis()) { checkNow?.receive() ?: awaitCancellation() }
    }
    return Confirmation.TimedOut
  }

  private fun map(e: Exception): PairingException {
    val m = e.message.orEmpty().lowercase()
    return when {
      "invalid" in m || "code" in m || "pin" in m -> PairingException.CodeFailed(e)
      else -> PairingException.Unreachable(e)
    }
  }

  companion object {
    private val CODE = Regex("TND-[A-Z0-9]{4}-[A-Z0-9]{4}-[A-Z0-9]{4}")

    fun queryOf(uri: String): Map<String, String> =
      uri.substringAfter('?', "").split('&').mapNotNull { part ->
        val i = part.indexOf('=')
        if (i <= 0) null else part.substring(0, i) to URLDecoder.decode(part.substring(i + 1), "UTF-8")
      }.toMap()

    /** Names Tendril's provider for the path it used. Anything else is left unsaid. */
    fun viaOf(provider: String): String? =
      when (provider.lowercase()) {
        "direct", "local", "lan" -> "LAN"
        "overlay", "broker" -> "Tailnet"
        "relay", "p2p" -> "Relay"
        else -> null
      }
  }
}
