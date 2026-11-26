package dev.agync.agenthub.alerts

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.app.RemoteInput
import dev.agync.agenthub.AgentHubApp
import dev.agync.agenthub.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Receives the shade's verbs. The system has already asked for an unlock on the lock screen. The
 * button's identity comes from its data URI, and [NotificationActions] checks it against the live
 * ask before anything is decided.
 */
class NotificationActionReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    val target = AlertIntents.parse(intent.data) ?: return
    if (intent.action != target.action) return
    val typed = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(AlertIntents.KEY_REPLY)?.toString()
    val center = (context.applicationContext as AgentHubApp).alerts
    val pending = goAsync()
    CoroutineScope(Dispatchers.Default).launch {
      try {
        // goAsync gives about ten seconds. Past eight the verb is handed to the app, which opens the item.
        val outcome = withTimeoutOrNull(8_000) { center.act(target.hubId, target.askId, target.action, target.option, typed) } ?: ActionOutcome.OpenItem
        if (outcome == ActionOutcome.OpenItem) {
          // Allowed where the system lets a notification start the app; the notification stays either way.
          runCatching {
            context.startActivity(
              Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .putExtra(AlertIntents.EXTRA_ASK_ID, target.askId).putExtra(AlertIntents.EXTRA_HUB_ID, target.hubId),
            )
          }
        }
      } finally {
        pending.finish()
      }
    }
  }
}
