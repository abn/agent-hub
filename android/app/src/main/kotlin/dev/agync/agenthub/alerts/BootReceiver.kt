package dev.agync.agenthub.alerts

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import dev.agync.agenthub.AgentHubApp

/** Starts the held sessions again after the phone restarts, when the person chose to stay connected. */
class BootReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
    (context.applicationContext as? AgentHubApp)?.alerts?.sync()
  }
}
