package dev.agync.agenthub.viewmodel

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.agync.agenthub.data.HubRegistry
import dev.agync.agenthub.data.PairedHub
import dev.agync.agenthub.data.PrefsRepository
import dev.agync.agenthub.model.PairWait
import dev.agync.agenthub.pairing.Confirmation
import dev.agync.agenthub.pairing.Handshake
import dev.agync.agenthub.pairing.PairOffer
import dev.agync.agenthub.pairing.Pairing
import dev.agync.agenthub.pairing.PairingException
import java.time.Clock
import java.time.Duration
import java.time.Instant
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

sealed interface PairStep {
  data object Welcome : PairStep

  data class Scan(val cameraDenied: Boolean = false) : PairStep

  data class Manual(val code: String = "", val address: String = "", val invalid: Boolean = false) : PairStep

  data class Confirm(val offer: PairOffer, val deviceName: String) : PairStep

  /** Handshake running: dialling, then comparing keys. */
  data class Connecting(val offer: PairOffer) : PairStep

  /**
   * The check string is up. With [needsYou] the hub's key was not pinned by a picture code, so
   * this phone waits for the person to say the two strings match; otherwise it waits for the
   * operator to confirm on the hub.
   */
  data class CheckString(val handshake: Handshake, val needsYou: Boolean = false, val wait: PairWait? = null) : PairStep

  /** The hub confirmed. The phone is registered; the flow moves to [Alerts] after a short beat, or on Continue. */
  data class Confirmed(val handshake: Handshake) : PairStep

  /** The wait ended without a pairing: the hub said no, or the time ran out. */
  data class Stopped(val reason: StopReason) : PairStep

  /** Paired. Choose how alerts reach you. */
  data class Alerts(val hubName: String, val stayConnected: Boolean = true, val notificationsDenied: Boolean = false) : PairStep

  data class CodeFailed(val unreachable: Boolean = false) : PairStep

  /** Everything is set up: leave the flow. */
  data object Done : PairStep
}

sealed interface StopReason {
  data object Refused : StopReason

  data class RanOut(val at: Instant) : StopReason
}

/**
 * The pairing flow of the board: scan or type, name the phone, compare the check string on the
 * hub, choose how alerts arrive. Tendril does the pairing; this holds the steps.
 */
class PairViewModel(
  private val pairing: Pairing,
  private val registry: HubRegistry,
  private val prefs: PrefsRepository,
  defaultDeviceName: String,
  private val clock: Clock = Clock.systemDefaultZone(),
) : ViewModel() {
  private val _step = MutableStateFlow<PairStep>(PairStep.Welcome)
  val step: StateFlow<PairStep> = _step

  private val _notice = MutableStateFlow<String?>(null)
  val notice: StateFlow<String?> = _notice

  private val _deviceName = MutableStateFlow(defaultDeviceName)
  val deviceName: StateFlow<String> = _deviceName

  private var job: Job? = null
  private var pairedHub: PairedHub? = null
  private var pending: Handshake? = null
  private var localAnswer: CompletableDeferred<Boolean>? = null
  private val checkNow = Channel<Unit>(Channel.CONFLATED)
  private var leftAt: Instant? = null
  private var leftForSettings = false

  fun startScan() {
    _step.value = PairStep.Scan()
  }

  fun cameraDenied() {
    _step.value = PairStep.Scan(cameraDenied = true)
  }

  fun startManual() {
    _step.value = PairStep.Manual()
  }

  fun editManual(code: String, address: String) {
    _step.value = PairStep.Manual(code, address)
  }

  fun back() {
    val s = _step.value
    _step.value =
      when (s) {
        is PairStep.Scan, is PairStep.Manual, is PairStep.CodeFailed, is PairStep.Stopped -> PairStep.Welcome
        is PairStep.Confirm -> if (s.offer.typed) PairStep.Manual(s.offer.codeLabel.orEmpty(), s.offer.address.orEmpty()) else PairStep.Scan()
        else -> s
      }
  }

  /** A scanned string. Anything that is not a pairing code is ignored, so the camera keeps looking. */
  fun scanned(raw: String) {
    if (_step.value !is PairStep.Scan) return
    val offer = pairing.read(raw) ?: return
    _step.value = PairStep.Confirm(offer, _deviceName.value)
  }

  fun submitManual() {
    val s = _step.value as? PairStep.Manual ?: return
    val offer = pairing.readTyped(s.code, s.address)
    _step.value = if (offer == null) s.copy(invalid = true) else PairStep.Confirm(offer, _deviceName.value)
  }

  fun setDeviceName(name: String) {
    _deviceName.value = name
    _step.update { if (it is PairStep.Confirm) it.copy(deviceName = name) else it }
  }

  fun pair() {
    val s = _step.value as? PairStep.Confirm ?: return
    val name = s.deviceName.trim().ifEmpty { _deviceName.value }
    _step.value = PairStep.Connecting(s.offer)
    job =
      viewModelScope.launch {
        val handshake =
          try {
            pairing.pair(s.offer, name)
          } catch (e: PairingException.KeyMismatch) {
            _notice.value = "The hub that answered is not the one the code names. Nothing was paired."
            _step.value = PairStep.Welcome
            return@launch
          } catch (e: PairingException) {
            _step.value = PairStep.CodeFailed(unreachable = e is PairingException.Unreachable)
            return@launch
          }
        pending = handshake
        if (handshake.needsLocalConfirm) {
          // Nothing pinned the hub's key, so the person is the check: this phone trusts the peer
          // only once they say the two strings match.
          _step.value = PairStep.CheckString(handshake, needsYou = true)
          val answer = CompletableDeferred<Boolean>().also { localAnswer = it }
          if (!answer.await()) {
            pairing.abort(handshake)
            pending = null
            _notice.value = "Pairing was cancelled because the check strings differ."
            _step.value = PairStep.Welcome
            return@launch
          }
        }
        try {
          pairing.confirm(handshake)
        } catch (e: PairingException) {
          pairing.abort(handshake)
          pending = null
          _step.value = PairStep.CodeFailed(unreachable = e is PairingException.Unreachable)
          return@launch
        }
        val since = clock.instant()
        val wait = PairWait(since, since.plus(pairing.confirmWindow))
        checkNow.tryReceive()
        _step.value = PairStep.CheckString(handshake, needsYou = false, wait = wait)
        val outcome =
          pairing.awaitConfirmation(handshake, checkNow) { at ->
            _step.update { if (it is PairStep.CheckString && it.wait != null) it.copy(wait = it.wait.copy(checkedAt = at)) else it }
          }
        when (outcome) {
          Confirmation.Confirmed -> {
            pending = null
            pairedHub = PairedHub(handshake.hubId, handshake.hubName, name, handshake.stateDir)
            registry.add(pairedHub!!)
            pairing.adopt(handshake)
            _step.value = PairStep.Confirmed(handshake)
            delay(CONFIRMED_BEAT_MILLIS)
            continueToAlerts()
          }
          Confirmation.Refused -> {
            pairing.abort(handshake)
            pending = null
            _step.value = PairStep.Stopped(StopReason.Refused)
          }
          Confirmation.TimedOut -> {
            pairing.abort(handshake)
            pending = null
            _step.value = PairStep.Stopped(StopReason.RanOut(wait.deadline))
          }
        }
      }
  }

  /** The person compared the check strings on both screens. */
  fun checkStringsMatch() {
    localAnswer?.complete(true)
  }

  /** The strings differ: something stands between this phone and the hub. */
  fun checkStringsDiffer() {
    localAnswer?.complete(false)
  }

  /** Cancels a pairing in flight, drops the pending peer, and goes back to the start. */
  fun cancelPairing() {
    job?.cancel()
    val h = pending
    pending = null
    if (h != null) viewModelScope.launch { pairing.abort(h) }
    _step.value = PairStep.Welcome
  }

  fun setStayConnected(stay: Boolean) {
    _step.update { if (it is PairStep.Alerts) it.copy(stayConnected = stay) else it }
  }

  /** The system answered the notification permission request. */
  fun notificationsAnswered(granted: Boolean) {
    val s = _step.value as? PairStep.Alerts ?: return
    if (granted) finish(s.stayConnected) else _step.value = s.copy(notificationsDenied = true)
  }

  /** "Not now" or "Continue without alerts": paired, with no alerts. */
  fun skipAlerts() = finish(stay = false)

  private fun finish(stay: Boolean) {
    prefs.update { it.copy(stayConnected = stay) }
    _step.value = PairStep.Done
  }

  /** The app went to the background. */
  fun appLeft() {
    leftAt = clock.instant()
  }

  /** The person is leaving through the app's own open-settings action, so the scanner is not stale when they return. */
  fun leavingForSettings() {
    leftForSettings = true
  }

  /**
   * The app is back in front. While the hub is being waited on, ask it now. A scanner left for
   * longer than [SCANNER_STALE_SECONDS] goes back to the start, so the camera stays off until the
   * person taps Scan again.
   */
  fun appReturned() {
    val away = leftAt?.let { Duration.between(it, clock.instant()) }
    val viaSettings = leftForSettings
    leftAt = null
    leftForSettings = false
    when (val s = _step.value) {
      is PairStep.Scan -> if (!viaSettings && away != null && away.seconds > SCANNER_STALE_SECONDS) _step.value = PairStep.Welcome
      is PairStep.CheckString -> if (!s.needsYou && s.wait != null) checkNow.trySend(Unit)
      else -> Unit
    }
  }

  /** Leaves the confirmed screen for the alerts choice. Once only: the beat and the button both land here. */
  fun continueToAlerts() {
    val s = _step.value as? PairStep.Confirmed ?: return
    _step.value = PairStep.Alerts(s.handshake.hubName)
  }

  /** From the stopped screen back to the start of pairing. */
  fun startAgain() {
    if (_step.value is PairStep.Stopped) _step.value = PairStep.Welcome
  }

  fun clearNotice() {
    _notice.value = null
  }

  private companion object {
    const val CONFIRMED_BEAT_MILLIS = 800L
    const val SCANNER_STALE_SECONDS = 30L
  }
}
