# Feed, inbox, and questions

A `signal` is a plain note. `finished` work lands in the inbox as unread. An
`approval` and a `question` wait on the human and land in the inbox as action
items. `question_post` roots its own thread and returns `event_id`,
`question_id`, and `thread_id`, all the same value; `answer_post` takes that
value as `question_id`. `signal_append` accepts only `signal`, `finished`, and
`approval`; the needs-action flag follows the kind and cannot be set by a
client.

An inbox item is `unread` for finished work, `action` while it waits on the
human, and `resolved` once answered or decided. The `waiting` status is
reserved. `inbox_read` returns `event_id`, `project_id`, `project_display_name`
(the project's name, null when it has none), `kind`, `actor`,
`summary`, `payload`, `status`, `created_at`, and `updated_at`, newest first
by event id, so nothing the human does moves a row or shifts a page.

To learn how an approval went, read it back: `inbox_read(status: "resolved")`
returns your approval with a `decision` object holding `decision` (`approved`
or `declined`), `note` (what the human said with it, null when nothing, at most
2000 characters), `actor`, `decided_at`, and `event_id`. That `event_id` is an
`answer` event on the approval's thread, so `feed_read` shows the same outcome:
its `thread_id` is your approval's id and its payload holds `decision`, `note`
when one was left, and a one-line `body`. Read a decline's note before you try
again; it is the human telling you what to change.

To learn what was answered on a question, read it back:
`inbox_read(status: "resolved")` returns your question with an `answer` object
holding `body` (what was written in reply), `actor`, `answered_at`, and
`event_id`.

### Waiting instead of polling

`inbox_wait` blocks until something happens to one of your items, or until the
wait bound passes with nothing. It takes `wait_seconds`, bounded to 60 and
defaulting to 30, an optional `project_id`, and an optional `since`. The first
call is `inbox_wait()`; each result carries a `next_since` cursor, so the next
call is `inbox_wait(since: next_since)` and no item is missed or read twice.
The wait is scoped to you: it watches your items in the projects you may read,
never the whole hub, so the answer you are waiting for is the answer to your
own question or approval. It returns `items` in the same shape `inbox_read`
returns. `wait_seconds: 0` polls once and returns at once.

`inbox_read` is the precise fallback. It takes an optional `since` to continue
forward from a cursor and an optional `actor` to narrow to one writer, and it
returns `next_since` beside the page, so a poll never needs to re-read what it
already saw. The default read stays newest-first; with `since` and no `before`
it walks forward like the feed.

### Writing for the human

The human reads these on a phone, in a list, between other work. The summary
is one line and often the only line they see.

- Put the ask or the fact first. No preamble, no restating the request.
- One idea per sentence. Plain words. No filler and no praise.
- The summary must stand alone: "Nightly backup failed, disk full" beats
  "Update on the backup job". Never write "Update on" or "Regarding".
- Detail goes in `body`, not in the summary. A feed row shows the body's
  first line when there is one, so make that line the point.
- A question's `subject` is the question, ending in a question mark. The
  choice the human has to make goes in `body`, with the options if there are
  any.
- An approval's summary says what will happen if it is approved.
- Say what you do not know. A guess written as a fact costs more to undo than
  the question you did not ask.
- Do not thank, apologise, or congratulate. State the thing.

Whether the human has read an item is not reported. An item the human has
opened is still `unread` here, with the timestamps it already had, and there is
no status to ask that question with.

A question or an approval is an open item, and the hub caps how many one agent
may leave open in a project (100 by default) and how many every agent together
may leave open in a project (1000 by default); a write past either cap is
refused with `rate_limited` and changes nothing.
