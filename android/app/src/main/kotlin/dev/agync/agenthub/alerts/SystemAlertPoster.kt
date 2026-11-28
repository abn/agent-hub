package dev.agync.agenthub.alerts

import android.annotation.SuppressLint
import android.app.Notification
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.Manifest
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.RemoteInput
import dev.agync.agenthub.MainActivity
import dev.agync.agenthub.R
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import dev.agync.agenthub.model.AskText
import dev.agync.agenthub.model.FeedEvent

/**
 * Posts the shade, the lock screen and the quiet Connected notification. Every post is behind
 * [allowed], which checks the notification permission, so lint's permission check is waived.
 */
@SuppressLint("MissingPermission")
class SystemAlertPoster(private val context: Context, private val prefs: () -> DevicePrefs, private val hubCount: () -> Int) : AlertPoster {
  private val nm = NotificationManagerCompat.from(context)
  private val accent = 0xFF84461D.toInt()

  private fun channelFor(kind: AskKind) =
    when (kind) { AskKind.Approval -> Channels.APPROVALS; AskKind.Question -> Channels.QUESTIONS; AskKind.Joining -> Channels.JOINING; AskKind.Idea -> Channels.QUESTIONS }

  private fun allowed() = context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

  private fun id(hubId: String, askId: String) = (hubId + "/" + askId).hashCode()

  /** A button that opens the item. The data URI keeps each one's PendingIntent its own. */
  private fun open(hubId: String, askId: String, verb: String = "open"): PendingIntent =
    PendingIntent.getActivity(
      context, 0,
      Intent(context, MainActivity::class.java).setData(AlertIntents.uri(hubId, askId, "dev.agync.agenthub.$verb"))
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP).putExtra(AlertIntents.EXTRA_ASK_ID, askId).putExtra(AlertIntents.EXTRA_HUB_ID, hubId),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

  /**
   * A button the receiver acts on. Its identity is hub, ask, verb and option index, all in the
   * data URI; the option is an index into the ask's answers, never text. Immutable, except the
   * typed reply, which the system must fill in.
   */
  private fun broadcast(hubId: String, askId: String, action: String, option: Int = AlertIntents.TYPED, mutable: Boolean = false): PendingIntent =
    PendingIntent.getBroadcast(
      context, 0,
      Intent(context, NotificationActionReceiver::class.java).setAction(action).setData(AlertIntents.uri(hubId, askId, action, option)),
      PendingIntent.FLAG_UPDATE_CURRENT or if (mutable) PendingIntent.FLAG_MUTABLE else PendingIntent.FLAG_IMMUTABLE,
    )

  private fun action(label: String, pi: PendingIntent) =
    // Every lock-screen verb needs an unlock first.
    NotificationCompat.Action.Builder(0, label, pi).setAuthenticationRequired(true).build()

  override fun post(hubId: String, hubName: String, ask: Ask, groupKey: String?) {
    if (!allowed()) return
    val p = prefs()
    val channel = channelFor(ask.kind)
    val channelName = AskText.kindLabel(ask.kind).let { if (ask.kind == AskKind.Approval) "Approvals" else if (ask.kind == AskKind.Question) "Questions" else it }
    val subText = if (hubCount() > 1) "$channelName · $hubName" else channelName
    val publicVersion =
      NotificationCompat.Builder(context, channel).setSmallIcon(R.drawable.ic_notification).setColor(accent)
        .setContentTitle(AskText.lockTitle(ask.kind)).setContentText(AskText.lockBody(ask.kind)).setSubText(subText).build()
    val b =
      NotificationCompat.Builder(context, channel)
        .setSmallIcon(R.drawable.ic_notification).setColor(accent)
        .setContentTitle(AskText.notificationTitle(ask)).setContentText(AskText.notificationBody(ask))
        .setStyle(NotificationCompat.BigTextStyle().bigText(AskText.notificationBody(ask)))
        .setSubText(subText).setWhen(ask.askedAt.toEpochMilli()).setShowWhen(true)
        .setContentIntent(open(hubId, ask.id)).setAutoCancel(false).setOnlyAlertOnce(true)
        .setCategory(NotificationCompat.CATEGORY_MESSAGE)
        .setVisibility(if (p.hideOnLockScreen) NotificationCompat.VISIBILITY_PRIVATE else NotificationCompat.VISIBILITY_PUBLIC)
        .setPublicVersion(publicVersion)
    if (groupKey != null) b.setGroup(groupKey).setGroupAlertBehavior(NotificationCompat.GROUP_ALERT_CHILDREN)
    // At most three actions. With the fingerprint opt-in on, a verb opens the app, where the prompt is.
    // Before API 31 a button cannot ask for an unlock, so there every verb opens the app.
    val viaApp = p.fingerprintBeforeApproving || Build.VERSION.SDK_INT < Build.VERSION_CODES.S
    when (ask.kind) {
      AskKind.Approval -> {
        b.addAction(if (viaApp) NotificationCompat.Action.Builder(0, "Approve", open(hubId, ask.id, "approve")).build() else action("Approve", broadcast(hubId, ask.id, AlertIntents.ACTION_APPROVE)))
        b.addAction(if (viaApp) NotificationCompat.Action.Builder(0, "Decline", open(hubId, ask.id, "decline")).build() else action("Decline", broadcast(hubId, ask.id, AlertIntents.ACTION_DECLINE)))
        b.addAction(action("Snooze 1h here", broadcast(hubId, ask.id, AlertIntents.ACTION_SNOOZE)))
      }
      AskKind.Question -> {
        val reply = NotificationCompat.Action.Builder(R.drawable.ic_notification, "Reply", if (viaApp) open(hubId, ask.id, "reply") else broadcast(hubId, ask.id, AlertIntents.ACTION_ANSWER, AlertIntents.TYPED, mutable = true))
          .addRemoteInput(RemoteInput.Builder(AlertIntents.KEY_REPLY).setLabel("Reply to ${ask.agent}").build()).setAuthenticationRequired(true).build()
        b.addAction(reply)
        if (ask.answers.size in 1..2) ask.answers.forEachIndexed { i, a -> b.addAction(if (viaApp) NotificationCompat.Action.Builder(0, a, open(hubId, ask.id, "answer$i")).build() else action(a, broadcast(hubId, ask.id, AlertIntents.ACTION_ANSWER, i))) }
        else b.addAction(NotificationCompat.Action.Builder(0, "Open", open(hubId, ask.id)).build())
      }
      AskKind.Joining -> b.addAction(NotificationCompat.Action.Builder(0, "Review", open(hubId, ask.id)).build())
      AskKind.Idea -> b.addAction(NotificationCompat.Action.Builder(0, "Open", open(hubId, ask.id)).build())
    }
    nm.notify(id(hubId, ask.id), b.build())
  }

  override fun cancel(hubId: String, askId: String) = nm.cancel(id(hubId, askId))

  override fun summary(hubId: String, hubName: String, summary: AskSummary): Boolean {
    if (!allowed()) return false
    val n =
      NotificationCompat.Builder(context, summary.kinds.singleOrNull()?.let(::channelFor) ?: Channels.QUESTIONS).setSmallIcon(R.drawable.ic_notification).setColor(accent)
        .setContentTitle(AskText.summaryTitle(summary.count)).setSubText(if (hubCount() > 1) hubName else null)
        .setGroup(summary.groupKey).setGroupSummary(true).setGroupAlertBehavior(NotificationCompat.GROUP_ALERT_CHILDREN)
        .setContentIntent(open(hubId, "")).setOnlyAlertOnce(true).setAutoCancel(false)
        .setCategory(NotificationCompat.CATEGORY_MESSAGE).setVisibility(if (prefs().hideOnLockScreen) NotificationCompat.VISIBILITY_PRIVATE else NotificationCompat.VISIBILITY_PUBLIC)
        .build()
    nm.notify(summaryId(hubId), n)
    return true
  }

  override fun cancelSummary(hubId: String) = nm.cancel(summaryId(hubId))

  private fun summaryId(hubId: String) = (hubId + "/asks").hashCode()

  override fun postFinished(hubId: String, hubName: String, event: FeedEvent) {
    if (!allowed()) return
    val n =
      NotificationCompat.Builder(context, Channels.FINISHED).setSmallIcon(R.drawable.ic_notification)
        .setContentTitle("${event.who} ${event.text}").setSubText(if (hubCount() > 1) "Finished work · $hubName" else "Finished work")
        .setContentIntent(open(hubId, "")).setSilent(true).setAutoCancel(true).build()
    nm.notify(id(hubId, event.id), n)
  }

  override fun connection(hubName: String, status: String): Boolean {
    if (!allowed()) return false
    shown = hubName to status
    nm.notify(CONNECTION_ID, connectionNotification(hubName, status))
    return true
  }

  /** What the Connected notification last said. */
  @Volatile private var shown: Pair<String, String>? = null

  /**
   * The Connected notification as it stands, for the service to hold when it starts again: the engine posts only when
   * the words change, so a service that put up "Connecting" over a settled line would leave it there.
   */
  fun currentConnection(hubName: String): Notification {
    val last = shown?.takeIf { it.first == hubName }
    return connectionNotification(hubName, last?.second ?: "Connecting")
  }

  /** Takes the quiet "Connected" notification down, when the held sessions are stopped. */
  fun clearConnection() {
    shown = null
    nm.cancel(CONNECTION_ID)
  }

  fun connectionNotification(hubName: String, status: String): Notification =
    NotificationCompat.Builder(context, Channels.CONNECTION).setSmallIcon(R.drawable.ic_notification)
      .setContentTitle("Connected to $hubName".takeIf { status.startsWith("via") || status == "Connected" } ?: hubName).setContentText(status)
      .setOngoing(true).setSilent(true).setPriority(NotificationCompat.PRIORITY_MIN).setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setContentIntent(PendingIntent.getActivity(context, 0, Intent(context, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)).build()

  companion object {
    const val CONNECTION_ID = 4701
  }
}
