package dev.agync.agenthub.alerts

import android.net.Uri

object AlertIntents {
  const val EXTRA_ASK_ID = "dev.agync.agenthub.ASK_ID"
  const val EXTRA_HUB_ID = "dev.agync.agenthub.HUB_ID"
  const val ACTION_APPROVE = "dev.agync.agenthub.APPROVE"
  const val ACTION_DECLINE = "dev.agync.agenthub.DECLINE"
  const val ACTION_SNOOZE = "dev.agync.agenthub.SNOOZE"
  const val ACTION_ANSWER = "dev.agync.agenthub.ANSWER"
  const val KEY_REPLY = "reply"

  /** The option index of a typed reply, which is not one of the ask's suggested answers. */
  const val TYPED = -1

  /**
   * A URI that names exactly one button: hub, ask, verb and option. A PendingIntent is told
   * apart from another by its data, so no button can ever be handed another's intent or extras.
   */
  fun uri(hubId: String, askId: String, action: String, option: Int = TYPED): Uri =
    Uri.Builder().scheme("agenthub").authority("ask").appendPath(hubId).appendPath(askId).appendPath(action.substringAfterLast('.')).appendPath(option.toString()).build()

  /** What a button's URI says it is for. Null when the URI is not one of ours. */
  data class Target(val hubId: String, val askId: String, val action: String, val option: Int)

  fun parse(uri: Uri?): Target? {
    if (uri == null || uri.scheme != "agenthub" || uri.authority != "ask") return null
    val p = uri.pathSegments
    if (p.size != 4) return null
    return Target(p[0], p[1], "dev.agync.agenthub." + p[2], p[3].toIntOrNull() ?: return null)
  }
}
