package dev.agync.agenthub.model

import java.time.Duration
import java.time.Instant

/** What the waiting screen says about the wait right now. */
data class WaitView(
  /** "waiting 2:05 · checked just now". Ticks every second. */
  val meta: String,
  val stillWaiting: Boolean,
  /** What a screen reader announces. Changes with the state, never with the seconds. */
  val status: String,
)

/** The wait for the operator's confirmation: when it began, when it runs out, when the hub was last asked. */
data class PairWait(val since: Instant, val deadline: Instant, val checkedAt: Instant? = null) {
  fun view(now: Instant): WaitView {
    val elapsed = Duration.between(since, now).coerceAtLeast(Duration.ZERO)
    val still = elapsed >= STILL_WAITING_AFTER
    val secs = elapsed.seconds
    val meta = buildString {
      append("waiting ${secs / 60}:${(secs % 60).toString().padStart(2, '0')}")
      if (still && checkedAt != null) {
        val ago = Duration.between(checkedAt, now).seconds.coerceAtLeast(0)
        append(" · ")
        append(if (ago < JUST_NOW_SECONDS) "checked just now" else "checked ${ago}s ago")
      }
    }
    return WaitView(meta, still, if (still) "Still waiting for you to confirm on the hub" else "Waiting for you to confirm on the hub")
  }

  companion object {
    val STILL_WAITING_AFTER: Duration = Duration.ofMinutes(2)
    private const val JUST_NOW_SECONDS = 5
  }
}
