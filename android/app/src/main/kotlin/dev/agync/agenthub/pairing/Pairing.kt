package dev.agync.agenthub.pairing

import java.time.Instant

/** What a scanned or typed code says before anything is dialled. Fields the code does not carry are null. */
data class PairOffer(
  /** The exact scanned string, or the typed code plus address, handed to Tendril as it is. */
  val raw: String,
  val hubName: String? = null,
  val codeLabel: String? = null,
  val worksUntil: Instant? = null,
  /** For a typed code: the hub's address, shown under the code on the hub. */
  val address: String? = null,
  val typed: Boolean = false,
  /** The host id the scanned code pins. Null for a typed code, which pins nothing. */
  val pinnedHostId: String? = null,
)

/**
 * The handshake is done and the peer is stored as pending. [needsLocalConfirm] is true when
 * nothing pinned the hub's key ahead of time (a typed code), so only a person comparing the
 * check string on both screens can say this is the right hub.
 */
data class Handshake(
  val hubId: String,
  val hubName: String,
  val checkString: String,
  val via: String?,
  val needsLocalConfirm: Boolean = false,
  /** Where Tendril keeps this hub's identity on the phone: one sealed state directory per paired hub. */
  val stateDir: String? = null,
  /** The address a typed code was claimed at, stored as the hub's route once confirmed. */
  val address: String? = null,
)

sealed class PairingException(message: String, cause: Throwable? = null) : Exception(message, cause) {
  /** Used, expired or mistyped: Tendril answers all three the same, so the app does not say which. */
  class CodeFailed(cause: Throwable? = null) : PairingException("code did not work", cause)

  /** The code could not be read as a pairing code at all. */
  class NotACode : PairingException("not a pairing code")

  /** Nothing answered at the address in the code. */
  class Unreachable(cause: Throwable? = null) : PairingException("unreachable", cause)

  /** The hub that answered is not the one the scanned code names. Nothing was kept. */
  class KeyMismatch : PairingException("the hub does not match the code")
}

enum class Confirmation { Confirmed, Refused, TimedOut }

/**
 * Pairing, as Tendril does it. The app holds no keys and picks no route: this is a thin seam
 * over Tendril's pairing, identity and connection, and the only place the app touches them.
 */
interface Pairing {
  /** Reads a scanned `tendril://pair` string. Null when it is not one. */
  fun read(raw: String): PairOffer?

  /** Reads a typed code and address. Null when the code is not shaped like a code. */
  fun readTyped(code: String, address: String): PairOffer?

  /**
   * Runs the handshake and returns the check string, with the peer stored as pending and not
   * yet trusted by this phone. Throws [PairingException]; a failure leaves no peer behind.
   */
  suspend fun pair(offer: PairOffer, deviceName: String): Handshake

  /** This phone's own acceptance of the peer. Called only for a pinned hub, or after the person says the check strings match. */
  suspend fun confirm(handshake: Handshake)

  /**
   * The pairing is confirmed and its hub is registered: its state directory now belongs to that
   * hub and is no longer the attempt's to discard.
   */
  suspend fun adopt(handshake: Handshake)

  /** Drops the pending peer: the check strings differed, or the person cancelled. */
  suspend fun abort(handshake: Handshake)

  /** Waits until the operator confirms or refuses the check string on the hub. */
  suspend fun awaitConfirmation(handshake: Handshake): Confirmation
}
