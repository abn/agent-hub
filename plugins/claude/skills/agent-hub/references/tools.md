# Tools

| Tool | What it does |
|---|---|
| `session_start` | Join a session by project and session name, creating it when it is new. Joining the same name again reuses the shared brain, and the result carries the handoff note the last participant left. With `from`, fork another session into a new name. With `exclusive`, refuse a name already in use. |
| `session_leave` | Stop being a live participant. Ends nothing for the others; your next call joins again. |
| `session_end` | Mark the session ended for everyone, with an optional `handoff` note for whoever carries the work on. Any participant may end it, and its live children end with it. The brain is retained until the human prunes it. |
| `session_list` | List sessions with their participants, status, handoff note, who started it, its child count, and where it was forked from. |
| `session_brief` | Brief yourself on a project in one call: since your feed cursor, answers and decisions on your own items and the other events, ranked and capped with a count of the rest; your previous session with its handoff and anchor; and knowledge base pages past `stale_after`. Call it before `session_start`. Moves no feed cursor and marks nothing delivered; its own trailer leaves out the answers it lists. |
| `session_anchor` | Write an `anchor` event at a phase boundary: `phase`, `summary`, `next_steps`, and optional `source_ids`, the project event ids the summary rests on. Session-scoped. Best-effort mirrors a rendered document to `/fs/RECOVERY.md` and returns `{anchor, recovery_path, recovery_written}`. |
| `session_resume` | Read a session's latest anchor, the scores that cite it, and the events after the anchor oldest first, with `next_since`, `recovery_path` and `truncated`. The full log stays one cursor page away. |
| `brain_get`, `brain_put`, `brain_list`, `brain_delete` | Read and write one of two stores: a session brain, or the project knowledge base. `store` is required on a write. `session` names the session to read or write. A session-store write needs one, and a write to a session that does not exist creates it. Every write is indexed for search. |
| `brain_promote` | Copy an entry from a session brain into a project knowledge base page that cites the session it came from. The source entry is left as it was, and one `kb_promoted` signal goes to the project feed. |
| `brain_history`, `brain_revert` | List a project page's versions, newest first, and write one back as a new write in your name, which also restores a deleted page. Reverting to the version the page holds writes nothing and returns `changed: false`. Read a version with `brain_get` and `version`. One `kb_reverted` signal goes to the project feed. |
| `feed_read` | Read a project feed, optionally filtered by kind or `session_id`. A stateful read: with no `since` it polls forward from your own durable server-side cursor for the project and advances it to the returned `next_since`, so a restarted agent resumes where it stopped; an explicit `since` is honoured and also advances the stored cursor. With `since` and no `before`, the page is oldest first, continuing forward from the cursor; otherwise it is newest first. |
| `signal_append` | Append `signal`, `finished`, `approval`, or `idea` to a project feed. Refuses `anchor` and `score`, which have their own tools. |
| `question_post` | Ask the human a question. It lands in the inbox and the feed and returns the question id. |
| `answer_post` | Reply to a question by its question id. |
| `inbox_read` | Read the human's global inbox, by status or project. Takes `since` and `actor` and returns a `next_since` cursor. |
| `inbox_wait` | Wait for new inbox items instead of polling: returns when something lands, or after the wait bound, with the new items and a `next_since` cursor to continue from. |
| `notify_subscribe`, `notify_unsubscribe`, `notify_list`, `notify_wait` | Register, remove, list, or wait on a standing interest in feed events by kind (including `comment` and `page`), optionally scoped to a project or narrowed to an author (`author: {member?, agent?}`) or to events that name you (`about_self`), bound to a session, so they arrive through the `notifications` member of a later tool result instead of being polled. |
| `artifact_publish`, `artifact_update`, `artifact_draft`, `artifact_get`, `artifact_versions`, `artifact_list`, `artifact_delete` | Publish an artifact, publish a new version, hold a version live while you write it, read a version, list the version history, and delete an artifact. |
| `comment_post`, `comment_list`, `comment_resolve`, `comment_delete` | Comment on an artifact, list its comments, and resolve or delete one. |
| `search` | Full-text search over feed events, artifacts, session brains, and project knowledge bases. |
| `enrol` | Offered only to a connection with no token or a pending one, with `whoami`. Asks the human to let this machine in as a member and returns a pending token to store. Approval may take hours. |
| `whoami`, `version` | Identity and connectivity checks. `whoami` returns the hub URL, your member, your agent, your session, the default project, a suggested session name and your personal space. |

Every successful tool result may carry a `notifications` member with a
`pending` list of what needs your attention: an answer to your question or a
decision on your approval (`source: "attention"`), or an event matching a
standing subscription (`source: "subscription"`). Each item is delivered once,
so read it on the call that carries it. It is a nudge, not the record: the
detail is read with `inbox_read`, `inbox_wait`, or `feed_read`.

Rows carry `author`, a typed record of who wrote them (a participant with its
member, agent and session, the human, local, or legacy), and `origin`. They also
carry `actor`, an opaque key you never read or parse.

Every tool except `session_start` (which takes `project_id` and
`session_name`) and `enrol` (which takes enrolment fields) also takes
`session?`, the shape below, and a plugin fills it for you. `whoami` and
`version` take only that. The argument shapes, with a trailing `?`
for optional:

```
session := "name" | {name, project_id?} | {session_id}
session_start(project_id, session_name, from?, exclusive?)
      -> {session_id, project_id, session_name, status, resumed,
          namespaces, recovery_path, handoff, anchor, brain_bytes, pickup}
      pickup := fork details {mode, from_session_id, from_session_name, handoff,
                source_active} on a new fork, else null
      from := {session_id} | {name, project_id?}   (fork only)
session_leave(session)
session_end(session, handoff?)
session_list(project_id?, status?, member?, agent?, kind?, live?, limit?)
      -> sessions: [{session_id, project_id, session_name, status, participants,
                     started_by, live, children_count, created_at, last_activity,
                     handoff, handoff_truncated, forked_from, brain_bytes}], truncated
session_brief(project_id)
      -> {project_id, agent, since: {event_id, basis},
          answers: {items: [{id, answer_id, kind, outcome, summary, text, by, at,
                             expired?}], more, more_capped?},
          previous_session: {session_id, session_name, status, last_activity,
                             handoff, anchor, recovery_path} | null,
          events: {items: [{id, kind, author, summary, at, thread_id?, open?}], more,
                   more_capped?},
          stale_pages: {items: [{path, title, stale_after}], more}}
session_anchor(phase, summary, next_steps, source_ids?, session, idempotency_key?)
      -> {anchor, recovery_path, recovery_written}
session_resume(session)
      -> {session_id, anchor|null, scores, events, next_since, recovery_path,
          truncated}
brain_get(path, session?, store?, project_id?, version?)
      with version -> {path, store, content, version, size_bytes, current, at, actor}
brain_put(path, content, store, session?, project_id?, if_version?)
brain_list(path?, session?, store?, project_id?)
                                         -> entries: [{path, type: key|file|dir, size_bytes}]
brain_delete(path, store, session?, project_id?, if_version?)
brain_promote(from_path, to_path, session?, project_id?, type?, title?, description?, tags?, if_version?)
                                         -> {ok, path, version, lint[]}
brain_history(path, store?, session?, project_id?, before?, limit?)
      -> {path, current_version, versions: [{id, op, actor, at, version,
          size_bytes, summary, kept, current}], total, next_before, truncated}
brain_revert(path, version, store?, session?, project_id?, if_version?)
      -> {ok, path, store, version, size_bytes, lint[], warnings[], changed}
feed_read(project_id, since?, before?, limit?, kinds?, session_id?, member?, agent?, kind?)
signal_append(project_id, kind, summary, payload?, thread_id?, idempotency_key?,
              expires_in_seconds?, on_expiry?)
      kind is one of "signal", "finished", "approval", or "idea"; "anchor" and
      "score" are refused and have their own tools
question_post(project_id, subject, body?, context?, options?, idempotency_key?,
              expires_in_seconds?)
answer_post(question_id, body, idempotency_key?)
inbox_read(status?, project_id?, limit?, since?, actor?)
inbox_wait(wait_seconds?, project_id?, since?)
      -> {items, next_since}
notify_subscribe(kinds, project_id?, author?, exclude_self?, about_self?, session?)
      author := {member?, agent?}
      -> {subscription_id, cursor, created, session_id}
notify_unsubscribe(subscription_id)
      -> {removed}
notify_list()
      -> {subscriptions: [{subscription_id, kinds, project_id, cursor, created_at}]}
notify_wait(wait_seconds?, session?)
      -> {notifications}?
enrol(member?, display_name?, reason, os?, host?)
      -> {status: "pending", member, token, status_url, store}
whoami()
      -> {state, hub_url, member, agent, kind, session, default_project,
          suggested_session, personal_project, admin, skill_url}
search(query, scope?, project_id?, type?, session_id?, member?, agent?, kind?, limit?, mode?)
artifact_publish(project_id, title, kind, content, description?, label?, envelope?, idempotency_key?)
artifact_update(artifact_id, content, envelope?, base_version?, force?, label?, idempotency_key?)
artifact_draft(artifact_id, content, envelope?)
artifact_get(artifact_id, version?)
artifact_versions(artifact_id)
artifact_list(project_id, session_id?, member?, agent?, kind?)
artifact_delete(artifact_id)
comment_post(artifact_id, body, anchor?, anchor_version?, idempotency_key?)
comment_list(artifact_id)
  Comments are flat (no parent or reply id). The PWA groups comments into one
  displayed thread when their anchors share a quote on the same version (text
  anchors) or when both are point anchors on the same version, so reply by
  posting with that same anchor and version. Unanchored comments never group.
comment_resolve(artifact_id, comment_id, done, delete_token?)
comment_delete(artifact_id, comment_id, delete_token?)
```

`mode` selects the recall backend: `lexical` (the default) or `semantic`.
Semantic mode falls back to the lexical index when no embedding model is
configured, and reports `fts-fallback` in the result's `backend` field.

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
`feed` hit the event's `event_kind` and `author`, an `artifact` hit its current
`version` and that version's `size_bytes`, a `brain` hit the `session_name` and
`session_status`. A field that belongs to another family is left off. A feed
hit's `snippet` is the payload's `body` when you wrote one as a string,
otherwise the summary, so put the report a reader should see in `body`; the
feed stage and an opened inbox item render that body as markdown, and its
opening is the search snippet. Every other payload field is still searched and
never shown.
