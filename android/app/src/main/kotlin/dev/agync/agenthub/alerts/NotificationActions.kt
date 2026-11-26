package dev.agync.agenthub.alerts

import dev.agync.agenthub.data.Connection
import dev.agync.agenthub.data.DecisionKind
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.model.Ask
import dev.agync.agenthub.model.AskKind
import kotlinx.coroutines.withTimeoutOrNull

sealed interface ActionOutcome {
  /** The verb was carried out. */
  data object Done : ActionOutcome

  /** The verb was not carried out; the person should look at the ask in the app. */
  data object OpenItem : ActionOutcome
}

/**
 * What a notification button may do. It acts only on the live ask: the hub, the ask and the option
 * must belong to one that still waits and takes that verb. Anything else, and every approval or
 * decline while the fingerprint opt-in is on, sends the person to the item instead of deciding.
 */
class NotificationActions(
  private val repoFor: (String) -> HubRepository?,
  private val prefs: () -> DevicePrefs,
  /** A broadcast receiver has about ten seconds: the check and the send each get a share of them. */
  private val checkMillis: Long = 4_000,
  private val sendMillis: Long = 3_000,
) {
  suspend fun act(hubId: String, askId: String, action: String, option: Int, typed: String?): ActionOutcome {
    val repo = repoFor(hubId) ?: return ActionOutcome.OpenItem
    // The ask is checked against the hub. If the hub cannot be asked in time, what the phone holds may be stale, so a
    // verb is queued (and judged by the hub when it is sent) rather than sent on that word.
    val fresh = withTimeoutOrNull(checkMillis) { repo.refresh(); repo.state.value.connection is Connection.Online } == true
    val ask = repo.state.value.snapshot?.asks?.firstOrNull { it.id == askId }
    if (ask == null || !ask.waiting) return ActionOutcome.OpenItem
    val gated = prefs().fingerprintBeforeApproving
    return when (action) {
      AlertIntents.ACTION_APPROVE, AlertIntents.ACTION_DECLINE -> {
        if (ask.kind != AskKind.Approval || gated) return ActionOutcome.OpenItem
        val approve = action == AlertIntents.ACTION_APPROVE
        send(repo, ask, fresh, if (approve) DecisionKind.Approve else DecisionKind.Decline, null) { repo.decide(ask, approve, null) }
        ActionOutcome.Done
      }
      AlertIntents.ACTION_SNOOZE -> {
        if (ask.kind == AskKind.Joining) return ActionOutcome.OpenItem
        repo.snooze(askId)
        ActionOutcome.Done
      }
      AlertIntents.ACTION_ANSWER -> answer(repo, ask, fresh, option, typed)
      else -> ActionOutcome.OpenItem
    }
  }

  /** Sends within its share of the time; past it, or when the check was stale, the verb waits in the outbox instead. */
  private suspend fun send(repo: HubRepository, ask: Ask, fresh: Boolean, kind: DecisionKind, text: String?, call: suspend () -> Unit) {
    if (fresh && withTimeoutOrNull(sendMillis) { call(); true } == true) return
    repo.queueAnswer(ask, kind, text)
  }

  private suspend fun answer(repo: HubRepository, ask: Ask, fresh: Boolean, option: Int, typed: String?): ActionOutcome {
    if (ask.kind != AskKind.Question) return ActionOutcome.OpenItem
    val text =
      when {
        option == AlertIntents.TYPED -> typed?.trim()?.takeIf { it.isNotEmpty() }
        option in ask.answers.indices -> ask.answers[option]
        else -> null
      } ?: return ActionOutcome.OpenItem
    send(repo, ask, fresh, DecisionKind.Answer, text) { repo.answer(ask, text) }
    return ActionOutcome.Done
  }
}
