# Tools

| Tool | What it does |
|---|---|
| `session_start` | Start or resume your own session by project and session name; the agent is the authenticated identity. Resuming the same name reuses your brain, and the result carries the handoff note the previous owner left. With `from`, pick up another agent's session: the hub adopts it or forks it. |
| `session_end` | Mark the session ended, with an optional `handoff` note for whoever picks the work up. Only the owner may end a session. The brain is retained until the human prunes it. |
| `session_list` | List sessions with their owner, status, handoff note, and where they were picked up from. |
| `brain_get`, `brain_put`, `brain_list`, `brain_delete` | Read and write one of two stores: a session brain, or the project knowledge base. `store` is required on a write. A read takes an optional `session` and reaches another session's brain; a write takes one to name your own session, and without one writes the connection's active session. Every write is indexed for search. |
| `brain_promote` | Copy an entry from your active session brain into a project knowledge base page that cites the session it came from. The source entry is left as it was, and one `kb_promoted` signal goes to the project feed. |
| `feed_read` | Read a project feed, optionally filtered by kind or session. A stateful read: with no `since` it polls forward from your own durable server-side cursor for the project and advances it to the returned `next_since`, so a restarted agent resumes where it stopped; an explicit `since` is honoured and also advances the stored cursor. With `since` and no `before`, the page is oldest first, continuing forward from the cursor; otherwise it is newest first. |
| `signal_append` | Append `signal`, `finished`, or `approval` to a project feed. |
| `question_post` | Ask the human a question. It lands in the inbox and the feed and returns the question id. |
| `answer_post` | Reply to a question by its question id. |
| `inbox_read` | Read the human's global inbox, by status or project. Takes `since` and `actor` and returns a `next_since` cursor. |
| `inbox_wait` | Wait for new inbox items instead of polling: returns when something lands, or after the wait bound, with the new items and a `next_since` cursor to continue from. |
| `artifact_publish`, `artifact_update`, `artifact_get`, `artifact_versions`, `artifact_list`, `artifact_delete` | Publish, read, list the version history of, and delete artifacts. |
| `comment_post`, `comment_list`, `comment_resolve`, `comment_delete` | Comment on an artifact, list its comments, and resolve or delete one. |
| `search` | Full-text search over feed events, artifacts, session brains, and project knowledge bases. |
| `whoami`, `version` | Identity and connectivity checks. |

The argument shapes, with a trailing `?` for optional:

```
session_start(project_id, session_name, from?)
      -> {session_id, project_id, agent, session_name, status, resumed, pickup,
          namespaces, recovery_path, handoff, brain_bytes}
session_end(session_id, handoff?)
session_list(project_id?, status?, agent?, limit?)
      -> sessions: [{session_id, project_id, session_name, agent, status,
                     created_at, last_activity, handoff, handoff_truncated,
                     forked_from, adopted_from, brain_bytes}], truncated
from := session
brain_get(path, session?, store?, project_id?)
brain_put(path, content, store, session?, project_id?, if_version?)
brain_list(path?, session?, store?, project_id?)
                                         -> entries: [{path, type: key|file|dir, size_bytes}]
brain_delete(path, store, session?, project_id?, if_version?)
brain_promote(from_path, to_path, project_id?, type?, title?, description?, tags?, if_version?)
                                         -> {ok, path, version, lint[]}
session := {session_id} | {agent, name, project_id?}
feed_read(project_id, since?, before?, limit?, kinds?, session?)
signal_append(project_id, kind, summary, payload?, thread_id?, idempotency_key?)
question_post(project_id, subject, body?, context?, idempotency_key?)
answer_post(question_id, body, idempotency_key?)
inbox_read(status?, project_id?, limit?, since?, actor?)
inbox_wait(wait_seconds?, project_id?, since?)
      -> {items, next_since}
search(query, scope?, project_id?, type?, session_id?, limit?)
artifact_publish(project_id, title, kind, content, description?, label?, envelope?, idempotency_key?)
artifact_update(artifact_id, content, envelope?, base_version?, force?, label?, idempotency_key?)
artifact_get(artifact_id, version?)
artifact_versions(artifact_id)
artifact_list(project_id, session?)
artifact_delete(artifact_id)
comment_post(artifact_id, body, anchor?, anchor_version?, idempotency_key?)
comment_list(artifact_id)
comment_resolve(artifact_id, comment_id, done, delete_token?)
comment_delete(artifact_id, comment_id, delete_token?)
```

A `search` query is read as words: any text is accepted and never an error,
a `"quoted phrase"` is matched as a phrase, and punctuation and the bare
operators `AND`, `OR`, `NOT` and `NEAR` are left out. A query with no word in
it finds nothing. A `thread_id` given to `signal_append` must be an event in
the same project that starts a thread; any other id is refused as not found,
and the id of a reply is refused with the thread to name instead. A retry with
the same `idempotency_key` returns the first call's id before any of this is
checked again.

`search` with `scope: "global"` covers every visible project; otherwise pass
`project_id`, and `type` filters by kind: `feed`, `artifact`, `brain` for
session brains, or `kb` for knowledge base pages. `session_id` narrows the
results to one session's brain content. Results are confined to the projects
the caller can see, and come back with `count`, the hits this page carries
before grouping, `truncated` when the limit cut the result, and `took_ms`, how
long the query itself took. Every hit names its project twice, as `project_id`
and as `project_display_name`, and carries what its family has to show: a
`feed` hit the event's `event_kind` and `actor`, an `artifact` hit its current
`version` and that version's `size_bytes`, a `brain` hit the `session_name` and
`session_status`. A field that belongs to another family is left off. A feed
hit's `snippet` is the payload's `body` when you wrote one as a string,
otherwise the summary, so put the sentence a reader should see in `body`; every
other payload field is still searched and never shown.
