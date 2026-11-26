package dev.agync.agenthub.alerts

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import dev.agync.agenthub.AgentHubApp
import dev.agync.agenthub.MainActivity
import dev.agync.agenthub.data.Connection
import dev.agync.agenthub.model.NowBuilder

/** The quick settings tile: connected is the system's ink, not copper, and the line says how many need you. */
class NeedsYouTile : TileService() {
  override fun onStartListening() {
    val tile = qsTile ?: return
    val app = application as AgentHubApp
    val hub = app.container.registry.state.value.current
    val state = hub?.let { app.container.repository(it).state.value }
    val needs = state?.let { NowBuilder.build(it, app.container.clock.instant(), app.container.fmt).badge } ?: 0
    tile.label = "Agent Hub"
    val line = when {
      hub == null -> "Not paired"
      needs > 0 -> "$needs need you"
      state?.connection == Connection.Online -> "Connected"
      else -> "Not connected"
    }
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) tile.subtitle = line else tile.label = "Agent Hub: $line"
    tile.state = if (state?.connection == Connection.Online) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
    tile.updateTile()
  }

  @SuppressLint("StartActivityAndCollapseDeprecated")
  override fun onClick() {
    val intent = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    if (Build.VERSION.SDK_INT >= 34) {
      startActivityAndCollapse(PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE))
    } else {
      @Suppress("DEPRECATION") startActivityAndCollapse(intent)
    }
  }
}
