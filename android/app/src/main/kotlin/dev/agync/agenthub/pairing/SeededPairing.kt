package dev.agync.agenthub.pairing

import java.time.Clock
import java.time.Duration
import java.time.Instant
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.delay

/**
 * Pairing against the seeded hub: any scan or code with the right shape "works", the check
 * string is fixed, and confirmation arrives after a short wait. For the demo build and tests.
 */
class SeededPairing(
  private val clock: Clock = Clock.systemDefaultZone(),
  private val handshakeDelayMillis: Long = 600,
  private val confirmDelayMillis: Long = 1500,
) : Pairing {
  override fun read(raw: String): PairOffer? =
    if (raw.startsWith("tendril://pair")) PairOffer(raw, "home hub", "TND-4F9K-22XQ-8N3P", clock.instant().plus(Duration.ofMinutes(15))) else null

  override fun readTyped(code: String, address: String): PairOffer? {
    val c = code.trim().uppercase()
    return if (Regex("TND-[A-Z0-9]{4}-[A-Z0-9]{4}-[A-Z0-9]{4}").matches(c) && address.isNotBlank()) {
      PairOffer("$c@$address", "home hub", c, clock.instant().plus(Duration.ofMinutes(15)), address, typed = true)
    } else null
  }

  override suspend fun pair(offer: PairOffer, deviceName: String): Handshake {
    delay(handshakeDelayMillis)
    if (offer.codeLabel?.endsWith("0000") == true) throw PairingException.CodeFailed()
    return Handshake("seeded-hub", offer.hubName ?: "home hub", "k7m2 qwxa 4fbn r3td", "LAN", needsLocalConfirm = offer.typed)
  }

  override suspend fun confirm(handshake: Handshake) = Unit

  override suspend fun adopt(handshake: Handshake) = Unit

  override suspend fun abort(handshake: Handshake) = Unit

  override suspend fun awaitConfirmation(handshake: Handshake, checkNow: ReceiveChannel<Unit>?, onChecked: (Instant) -> Unit): Confirmation {
    delay(confirmDelayMillis)
    onChecked(clock.instant())
    return Confirmation.Confirmed
  }
}
