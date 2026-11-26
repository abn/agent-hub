package dev.agync.agenthub.data

import java.time.Instant
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

internal val StoreJson = Json { ignoreUnknownKeys = true; encodeDefaults = true }

/** A hub this phone is paired with. The id is Tendril's peer id for the hub, opaque to the app. */
@Serializable
data class PairedHub(
  val id: String,
  val name: String,
  val deviceName: String,
  val stateDir: String? = null,
  /** The hub revoked this phone. Kept across restarts, so the app does not keep asking a hub that said no. */
  val unpaired: Boolean = false,
  /** The hub has answered as active at least once, so a session that ends with nothing back reads as an unpairing. */
  val wasActive: Boolean = false,
)

enum class DecisionKind { Answer, Approve, Decline, Admit, TurnAway, Resolve, Clear }

/** Why a queued decision was not accepted, kept so the text can be copied. */
@Serializable
data class StoredRefusal(val kind: String, val atEpochMilli: Long, val answer: String? = null) {
  fun toRefusal(): Refusal =
    if (kind == "deadline") Refusal.ClosedAtDeadline(Instant.ofEpochMilli(atEpochMilli))
    else Refusal.AnsweredElsewhere(Instant.ofEpochMilli(atEpochMilli), answer)

  companion object {
    fun of(r: Refusal): StoredRefusal =
      when (r) {
        is Refusal.ClosedAtDeadline -> StoredRefusal("deadline", r.at.toEpochMilli())
        is Refusal.AnsweredElsewhere -> StoredRefusal("answered", r.at.toEpochMilli(), r.answer)
      }
  }
}

/** An answer or decision given while the hub could not be reached. It waits on this phone. */
@Serializable
data class QueuedDecision(
  val askId: String,
  val agent: String,
  val sentence: String,
  val kind: DecisionKind,
  val text: String? = null,
  val note: String? = null,
  val name: String? = null,
  val grants: Set<String> = emptySet(),
  val queuedAtEpochMilli: Long,
  val refusal: StoredRefusal? = null,
) {
  val queuedAt: Instant get() = Instant.ofEpochMilli(queuedAtEpochMilli)
  val refused: Boolean get() = refusal != null

  /** What the person wrote or chose, for display and for copying. */
  val display: String
    get() = when (kind) {
      DecisionKind.Answer -> text.orEmpty()
      DecisionKind.Approve -> listOfNotNull("Approve", note?.takeIf { it.isNotBlank() }).joinToString(": ")
      DecisionKind.Decline -> listOfNotNull("Decline", note?.takeIf { it.isNotBlank() }).joinToString(": ")
      DecisionKind.Admit -> "Approve ${name.orEmpty()}"
      DecisionKind.TurnAway -> "Refuse ${name.orEmpty()}"
      DecisionKind.Resolve -> "Resolve"
      DecisionKind.Clear -> "Clear"
    }
}

/** Device-local snoozes: per phone, never sent to the hub. */
@Serializable
data class Snoozes(val untilEpochMilli: Map<String, Long> = emptyMap())

@Serializable
data class DevicePrefs(
  val stayConnected: Boolean = true,
  val alertApprovals: Boolean = true,
  val alertJoining: Boolean = true,
  val alertFinished: Boolean = false,
  val fingerprintBeforeApproving: Boolean = false,
  val hideOnLockScreen: Boolean = true,
  val theme: String = "system",
  val seenAtEpochMilli: Long = 0,
)
