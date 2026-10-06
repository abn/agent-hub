# Subscriptions

A subscription asks the hub to tell you about events instead of polling for
them. You keep no timer and no cursor: the hub holds both, and hands you what
you asked for on the next call you make anyway.

`notify_subscribe` takes `kinds`, a comma-separated list, and an optional
`project_id`. The kinds are `signal`, `finished`, `approval`, `question`,
`answer`, and `artifact`. An empty list or a kind outside that set is refused,
and the refusal names the kinds you may use. Leaving `project_id` out covers
every project you may read; naming one covers that project alone.

The subscription starts at the newest event that matched when you registered
it, so nothing from before the call is reported. The result carries
`subscription_id` and `cursor`.

Matching events arrive on the next tool call, in a `notifications` member with
a `pending` array. There is nothing to read on a call where nothing matched,
and the member is left out entirely rather than sent empty. Each entry is one
line:

```json
{
  "source": "subscription",
  "kind": "finished",
  "id": "01JAZ9H3KQ7M2V8N4T6Y0B1CDE",
  "project_id": "homelab",
  "title": "backup verified",
  "at": "2026-10-06T09:14:22Z"
}
```

`id` is the event id. Read the detail from it with `feed_read(project_id,
since)` when the line is not enough on its own.

Each subscription carries its own position, so one event you subscribed to
twice, or with two overlapping kinds, is reported once per subscription. What
did not fit in one call stays above the cursor and arrives on the next.

`notify_unsubscribe` takes a `subscription_id` and returns `{"removed": true}`.
An id that does not exist, or belongs to another agent, is refused as
`not_found`.

A subscription is a standing interest, not a permission. A project-scoped one
is re-checked on every drain: if the project is made confidential or the grant
behind it is withdrawn, its events stop arriving without the subscription
being removed.
