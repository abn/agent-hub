package dev.agync.agenthub.alerts

import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import dev.agync.agenthub.AgentHubApp

/**
 * The foreground service that keeps the Tendril sessions open so alerts arrive without a push
 * service. Type specialUse: Android 15 caps dataSync at six hours a day.
 */
class HoldService : Service() {
  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val app = application as AgentHubApp
    val hubName = app.container.registry.state.value.current?.name ?: "your hub"
    val notification = app.alerts.poster.currentConnection(hubName)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      startForeground(SystemAlertPoster.CONNECTION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
    } else {
      startForeground(SystemAlertPoster.CONNECTION_ID, notification)
    }
    app.alerts.serviceStarted()
    app.alerts.sync()
    return START_STICKY
  }

  override fun onDestroy() {
    val alerts = (application as AgentHubApp).alerts
    alerts.serviceStopped()
    alerts.stopAll()
    super.onDestroy()
  }
}
