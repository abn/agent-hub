package dev.agync.agenthub.model

/** The words the app uses for an ask, in one place so rows, the lede, the shade and the widget agree. */
object AskText {
  private val numbers = listOf("No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten")

  fun word(n: Int): String = numbers.getOrNull(n) ?: n.toString()

  /** "wants a decision: Run the migration on staging", the row sentence after the bold agent. */
  fun rowTail(ask: Ask): String =
    when (ask.kind) {
      AskKind.Approval -> "wants a decision: ${ask.sentence}"
      AskKind.Question -> "asks: ${ask.sentence}"
      AskKind.Joining -> "wants to join the hub"
      AskKind.Idea -> "noted: ${ask.sentence}"
    }

  /** The clause the lede uses: agent in bold by the caller, then this. */
  fun ledeTail(ask: Ask): String =
    when (ask.kind) {
      AskKind.Approval -> "wants a decision on “${ask.sentence}”"
      AskKind.Question -> "asks “${ask.sentence}”"
      AskKind.Joining -> "wants to join the hub"
      AskKind.Idea -> "noted “${ask.sentence}”"
    }

  /** The notification title. Names the agent, because the shade shows the user's own device. */
  fun notificationTitle(ask: Ask): String =
    when (ask.kind) {
      AskKind.Approval -> "${ask.agent} wants a decision"
      AskKind.Question -> "${ask.agent} asks"
      AskKind.Joining -> "${ask.agent} wants to join"
      AskKind.Idea -> "An idea of yours waits"
    }

  fun notificationBody(ask: Ask): String =
    when (ask.kind) {
      AskKind.Approval -> buildString {
        append(ask.sentence)
        if (!ask.prose.isNullOrBlank()) append(". ").append(ask.prose.trim().trimEnd('.'))
        if (ask.onSilence != null) append(". ").append(ask.onSilence).append(" if silent")
        append('.')
      }.replace("..", ".")
      AskKind.Question -> ask.sentence
      AskKind.Joining -> ask.enrolment?.let { e ->
        listOfNotNull(e.firstAgent, e.system?.let { "on $it" }).joinToString(" ") +
          (e.from?.let { ", from $it" } ?: "")
      } ?: "A machine asks to join."
      AskKind.Idea -> ask.sentence
    }

  /** The lock screen names no agent and says nothing of the ask. */
  fun lockTitle(kind: AskKind): String =
    when (kind) {
      AskKind.Approval -> "An agent wants a decision"
      AskKind.Question -> "An agent asks a question"
      AskKind.Joining -> "A machine wants to join"
      AskKind.Idea -> "An idea of yours waits"
    }

  fun lockBody(kind: AskKind): String =
    when (kind) {
      AskKind.Approval -> "Unlock to see what it asks."
      AskKind.Question -> "Unlock to see it and reply."
      AskKind.Joining -> "Unlock to review it."
      AskKind.Idea -> "Unlock to see it."
    }

  fun kindLabel(kind: AskKind): String =
    when (kind) {
      AskKind.Approval -> "Approval"
      AskKind.Question -> "Question"
      AskKind.Joining -> "Joining"
      AskKind.Idea -> "Idea"
    }

  fun kindMark(kind: AskKind): String =
    when (kind) {
      AskKind.Approval -> "!"
      AskKind.Question -> "?"
      AskKind.Joining -> ""
      AskKind.Idea -> "*"
    }
}
