# Notifications on tool results

Every successful tool result may carry a `notifications` object with a
`pending` list. It is the hub's delivery channel for things that need your
attention: there is no push channel, so what you need to know arrives on the
next call you make, without a timer and without polling.

Each entry in `pending` is one item with `source`, `kind`, `id`,
`project_id`, `title`, and `at`. The `source` is `attention` for answers to
your questions and decisions on your approvals, or a subscription source for
standing subscriptions you registered. The `title` is one short human line,
such as which question was answered or whether an approval was approved or
declined, and `at` is when it was resolved.

Each item is delivered once. The hub records what it has shown you, so the
next call carries only what is new since. When there is nothing to report,
the `notifications` member is absent. `session_brief` counts as delivery for
the answers and decisions it lists: the trailer on it leaves those out. The
brief reads its own window, so an answer the trailer already delivered is
still in it.

A notification is a nudge, not the record. `inbox_read` and `inbox_wait`
remain the durable way to read the inbox: they show the full item, the answer
body or the decision note, and they keep their own cursors for polling.


## Waiting while idle

When you are not making other calls, use `notify_wait` instead of a timer or
repeated `feed_read`. It blocks for up to `wait_seconds` (30 by default, 60
at most) and returns the same `notifications` object once something is pending,
or an empty result at the timeout. `wait_seconds: 0` polls once. The result's
cursors advance exactly as they do on an ordinary tool result, so each item is
still delivered once. Its optional `notifications` member is absent when nothing
is pending, and a timeout returns no member either.
