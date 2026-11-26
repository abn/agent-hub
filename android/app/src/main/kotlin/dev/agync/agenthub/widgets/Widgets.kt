package dev.agync.agenthub.widgets

import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.GlanceTheme
import androidx.glance.action.Action
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.appWidgetBackground
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.appwidget.updateAll
import androidx.glance.background
import androidx.glance.color.ColorProvider
import androidx.glance.layout.Alignment
import androidx.glance.layout.Box
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.height
import androidx.glance.layout.padding
import androidx.glance.layout.width
import androidx.glance.semantics.contentDescription
import androidx.glance.semantics.semantics
import androidx.glance.text.FontFamily
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import dev.agync.agenthub.AgentHubApp
import dev.agync.agenthub.MainActivity
import dev.agync.agenthub.alerts.AlertIntents
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskText
import dev.agync.agenthub.ui.theme.DarkColors
import dev.agync.agenthub.ui.theme.LightColors

private fun c(day: Color, night: Color) = ColorProvider(day = day, night = night)

private val raised = c(LightColors.raised, DarkColors.raised)
private val ink = c(LightColors.ink, DarkColors.ink)
private val ink2 = c(LightColors.ink2, DarkColors.ink2)
private val line = c(LightColors.line, DarkColors.line)
private val accent = c(LightColors.accent, DarkColors.accent)
private val ok = c(LightColors.ok, DarkColors.ok)

private val mono = FontFamily.Monospace

/**
 * A row or a whole widget opens the app. Nothing here decides anything: deciding stays where the
 * blast radius is shown, and the fingerprint opt-in and the unlock rules live there. Each tap has
 * its own data URI so no widget's PendingIntent is ever another's.
 */
private fun open(context: Context, hubId: String?, askId: String?): Action {
  val intent = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
  if (hubId != null && askId != null) {
    intent.setData(AlertIntents.uri(hubId, askId, "dev.agync.agenthub.open")).putExtra(AlertIntents.EXTRA_ASK_ID, askId).putExtra(AlertIntents.EXTRA_HUB_ID, hubId)
  }
  return actionStartActivity(intent)
}

@Composable
private fun Frame(data: WidgetData, context: Context, content: @Composable () -> Unit) {
  Box(
    GlanceModifier.fillMaxSize().appWidgetBackground().background(raised).cornerRadius(24.dp).padding(16.dp).clickable(open(context, null, null)),
  ) { content() }
}

@Composable
private fun Header(data: WidgetData, showTime: Boolean) {
  Row(GlanceModifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
    Text(data.hubName, style = TextStyle(color = ink2, fontSize = 13.sp, fontWeight = FontWeight.Bold), maxLines = 1, modifier = GlanceModifier.defaultWeight())
    if (showTime && data.syncedAt.isNotEmpty()) Text(data.syncedAt, style = TextStyle(color = ink2, fontSize = 12.sp, fontFamily = mono))
  }
}

@Composable
private fun LiveWord(count: Int) {
  Row(verticalAlignment = Alignment.CenterVertically) {
    Text("● ", style = TextStyle(color = ok, fontSize = 12.sp))
    Text("$count live", style = TextStyle(color = ok, fontSize = 13.sp, fontWeight = FontWeight.Medium))
  }
}

/** The list widget: count, up to three asks, and what can wait. */
@Composable
fun ListContent(data: WidgetData, context: Context) {
  Frame(data, context) {
    Column(GlanceModifier.fillMaxSize()) {
      Header(data, showTime = true)
      if (data.note != null) {
        Spacer(GlanceModifier.height(12.dp))
        Text(data.note, style = TextStyle(color = ink, fontSize = 15.sp, fontWeight = FontWeight.Medium))
      } else {
        Row(GlanceModifier.fillMaxWidth().padding(top = 8.dp, bottom = 6.dp), verticalAlignment = Alignment.Bottom) {
          Text(data.needsCount.toString(), style = TextStyle(color = if (data.needsCount > 0) accent else ink, fontSize = 28.sp, fontFamily = mono, fontWeight = FontWeight.Medium))
          Text("  need you", style = TextStyle(color = ink, fontSize = 15.sp, fontWeight = FontWeight.Bold), modifier = GlanceModifier.defaultWeight())
          if (data.liveCount > 0) LiveWord(data.liveCount)
        }
        data.needsYou.take(WidgetData.ROWS).forEach { ask -> AskLine(ask, data, context) }
        val more = data.needsCount - WidgetData.ROWS
        val tail = listOfNotNull(more.takeIf { it > 0 }?.let { "and $it more" }, data.canWait.takeIf { it > 0 }?.let { "and $it can wait" }).joinToString(", ")
        if (tail.isNotEmpty()) Text(tail, style = TextStyle(color = ink2, fontSize = 13.sp), modifier = GlanceModifier.fillMaxWidth().padding(top = 8.dp))
      }
    }
  }
}

@Composable
private fun AskLine(ask: Ask, data: WidgetData, context: Context) {
  Row(
    GlanceModifier.fillMaxWidth().padding(vertical = 8.dp).clickable(open(context, data.hubId, ask.id)).semantics { contentDescription = "${ask.agent}: ${AskText.rowTail(ask)}" },
    verticalAlignment = Alignment.Top,
  ) {
    Text(ask.mark(), style = TextStyle(color = if (ask.kind == dev.agync.agenthub.model.AskKind.Approval) accent else ink2, fontSize = 15.sp, fontFamily = mono, fontWeight = FontWeight.Bold), modifier = GlanceModifier.width(24.dp))
    Text("${ask.agent} ${AskText.rowTail(ask)}", style = TextStyle(color = ink, fontSize = 14.sp), maxLines = 2, modifier = GlanceModifier.defaultWeight())
  }
  Box(GlanceModifier.fillMaxWidth().height(1.dp).background(line)) {}
}

/** The 2x2 count: how many need you, and how many are live. */
@Composable
fun CountContent(data: WidgetData, context: Context) {
  Frame(data, context) {
    Column(GlanceModifier.fillMaxSize()) {
      Header(data, showTime = false)
      Spacer(GlanceModifier.defaultWeight())
      if (data.note != null) {
        Text(data.note, style = TextStyle(color = ink, fontSize = 14.sp, fontWeight = FontWeight.Medium))
      } else {
        Text(data.needsCount.toString(), style = TextStyle(color = if (data.needsCount > 0) accent else ink, fontSize = 44.sp, fontFamily = mono, fontWeight = FontWeight.Medium))
        Text("need you", style = TextStyle(color = ink, fontSize = 14.sp, fontWeight = FontWeight.Bold))
        if (data.liveCount > 0) Row(GlanceModifier.padding(top = 4.dp)) { LiveWord(data.liveCount) }
      }
    }
  }
}

/** The 2x2 live list: the running sessions, and how many subagents each has. */
@Composable
fun LiveContent(data: WidgetData, context: Context) {
  Frame(data, context) {
    Column(GlanceModifier.fillMaxSize()) {
      Text("Live", style = TextStyle(color = ink2, fontSize = 13.sp, fontWeight = FontWeight.Bold))
      if (data.note != null) {
        Text(data.note, style = TextStyle(color = ink, fontSize = 14.sp), modifier = GlanceModifier.padding(top = 8.dp))
      } else if (data.live.isEmpty()) {
        Text("Nothing is running", style = TextStyle(color = ink2, fontSize = 14.sp), modifier = GlanceModifier.padding(top = 8.dp))
      } else {
        data.live.take(3).forEach { l ->
          Column(GlanceModifier.padding(top = 8.dp)) {
            Text(l.name, style = TextStyle(color = ink, fontSize = 14.sp, fontWeight = FontWeight.Bold), maxLines = 1)
            Row {
              Text("● live", style = TextStyle(color = ok, fontSize = 12.sp))
              if (l.subagents > 0) Text(" · ${l.subagents} subagent" + if (l.subagents > 1) "s" else "", style = TextStyle(color = ink2, fontSize = 12.sp))
            }
          }
        }
      }
    }
  }
}

private suspend fun load(context: Context): WidgetData {
  val app = context.applicationContext as AgentHubApp
  val container = app.container
  val hub = container.registry.state.value.current ?: return WidgetData.empty("Not paired")
  return WidgetData.from(hub.id, hub.name, container.repository(hub).state.value, container.clock.instant(), container.fmt)
}

class NeedsYouListWidget : GlanceAppWidget() {
  override val sizeMode = SizeMode.Single

  override suspend fun provideGlance(context: Context, id: GlanceId) {
    val data = load(context)
    provideContent { GlanceTheme { ListContent(data, context) } }
  }
}

class NeedsYouCountWidget : GlanceAppWidget() {
  override val sizeMode = SizeMode.Single

  override suspend fun provideGlance(context: Context, id: GlanceId) {
    val data = load(context)
    provideContent { GlanceTheme { CountContent(data, context) } }
  }
}

class LiveWidget : GlanceAppWidget() {
  override val sizeMode = SizeMode.Single

  override suspend fun provideGlance(context: Context, id: GlanceId) {
    val data = load(context)
    provideContent { GlanceTheme { LiveContent(data, context) } }
  }
}

class NeedsYouListReceiver : GlanceAppWidgetReceiver() { override val glanceAppWidget: GlanceAppWidget = NeedsYouListWidget() }

class NeedsYouCountReceiver : GlanceAppWidgetReceiver() { override val glanceAppWidget: GlanceAppWidget = NeedsYouCountWidget() }

class LiveReceiver : GlanceAppWidgetReceiver() { override val glanceAppWidget: GlanceAppWidget = LiveWidget() }

object WidgetRefresh {
  suspend fun updateAll(context: Context) {
    NeedsYouListWidget().updateAll(context)
    NeedsYouCountWidget().updateAll(context)
    LiveWidget().updateAll(context)
  }
}
