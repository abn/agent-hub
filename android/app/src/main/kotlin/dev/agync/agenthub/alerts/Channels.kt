package dev.agync.agenthub.alerts

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import dev.agync.agenthub.R

/** One notification channel per kind of ask, so sound and priority are the person's to set in Android. */
object Channels {
  const val APPROVALS = "approvals"
  const val QUESTIONS = "questions"
  const val JOINING = "joining"
  const val FINISHED = "finished"
  const val CONNECTION = "connection"

  fun create(context: Context) {
    val nm = context.getSystemService(NotificationManager::class.java)
    fun channel(id: String, name: Int, importance: Int) = NotificationChannel(id, context.getString(name), importance).also(nm::createNotificationChannel)
    channel(APPROVALS, R.string.channel_approvals, NotificationManager.IMPORTANCE_HIGH)
    channel(QUESTIONS, R.string.channel_questions, NotificationManager.IMPORTANCE_HIGH)
    channel(JOINING, R.string.channel_joining, NotificationManager.IMPORTANCE_DEFAULT)
    channel(FINISHED, R.string.channel_finished, NotificationManager.IMPORTANCE_LOW)
    channel(CONNECTION, R.string.channel_connection, NotificationManager.IMPORTANCE_MIN)
  }
}
