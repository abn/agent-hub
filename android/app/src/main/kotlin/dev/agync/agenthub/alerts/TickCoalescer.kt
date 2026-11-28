package dev.agync.agenthub.alerts

import android.app.Activity
import android.app.Application
import android.os.Bundle
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.delay
import kotlinx.coroutines.withTimeoutOrNull

/** How long ticks are gathered into one refresh: short while the app is open, long while only the service runs. */
object TickWindow {
  const val FOREGROUND_MILLIS = 2_000L
  const val BACKGROUND_MILLIS = 15_000L
}

/**
 * The hub ticks on every store write, so a busy hub ticks several times a second. A tick after a quiet spell goes
 * through at once, so an alert is not late. After each refresh a window runs, and the ticks that come in it are
 * gathered into one more refresh at its end: the newest state is fetched, never skipped, and two refreshes are never
 * closer than the window. A tick that comes while a refresh runs waits for it and makes the next. When the app comes
 * to the front during a long window, the wait is cut to the short window.
 */
fun Flow<Unit>.coalesced(
  foreground: StateFlow<Boolean>,
  foregroundMillis: Long = TickWindow.FOREGROUND_MILLIS,
  backgroundMillis: Long = TickWindow.BACKGROUND_MILLIS,
): Flow<Unit> {
  val source = this
  return channelFlow {
    val pending = Channel<Unit>(Channel.CONFLATED)
    launch {
      source.collect { pending.trySend(Unit) }
      pending.close()
    }
    while (pending.receiveCatching().isSuccess) {
      send(Unit)
      do {
        if (foreground.value) {
          delay(foregroundMillis)
        } else {
          val cameForward = withTimeoutOrNull(backgroundMillis) { foreground.first { it } } != null
          if (cameForward) delay(foregroundMillis)
        }
        // What came during the window is answered by one refresh at its end, and a quiet window ends the run.
        val more = pending.tryReceive().isSuccess
        if (more) send(Unit)
      } while (more)
    }
  }.buffer(Channel.RENDEZVOUS)
}

/** Whether any activity of the app is started, fed by the activity callbacks of the application. */
class ForegroundTracker : Application.ActivityLifecycleCallbacks {
  private var count = 0
  private val _state = MutableStateFlow(false)
  val state: StateFlow<Boolean> = _state

  @Synchronized
  fun started() {
    count++
    _state.value = true
  }

  @Synchronized
  fun stopped() {
    count = maxOf(0, count - 1)
    _state.value = count > 0
  }

  override fun onActivityStarted(activity: Activity) = started()

  override fun onActivityStopped(activity: Activity) = stopped()

  override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit

  override fun onActivityResumed(activity: Activity) = Unit

  override fun onActivityPaused(activity: Activity) = Unit

  override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit

  override fun onActivityDestroyed(activity: Activity) = Unit

  companion object {
    /** The one the app registers; the alert engines read it. */
    val app = ForegroundTracker()
  }
}
