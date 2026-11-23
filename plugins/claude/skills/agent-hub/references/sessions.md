# Sessions, the brain, and the stores

## Before you start

`session_brief(project_id)` is the first read in a project. `previous_session`
is your most recent other session there, with the handoff note it left and its
`recovery_path`, so `brain_get(recovery_path, session: {session_id})` reads the
detail it points at. Call it before `session_start`, so a resumed session is
still the previous one.

## Sessions are shared

A session is a shared space of work. It belongs to no one: any participant
joins it by name, and nothing is handed over. Two agents on one branch both
join `feat-redesign` and write as two participants. Two instances of the same
agent on one member in one session are the same participant.

A participant is one agent of one member in one session. Every write records
all three, and the hub shows them joined: `laptop · claude-code · feat-redesign`.

Name a session for people: a branch, a worktree or a topic, reusable, with
letters, digits, `_` and `-`, at most 64 characters per segment, and a slash-separated child name at most 128 in all. A name that is a UUID or a
ULID is refused with `session_name_opaque`.

## Naming the session on a call

Every tool except `session_start` and `enrol` takes `session`. It is one of:

| Shape | Means |
|---|---|
| `"feat-redesign"` | the session of that name, in the call's project |
| `{"name": "feat-redesign", "project_id": "homelab"}` | the same, in a named project |
| `{"session_id": "..."}` | one session by id |

`{}` or a project alone is refused with `session_invalid`. When you give an id
and also a name or project, they must match the session, or the call is refused
with `session_mismatch`. An id the hub does not know is `session_unknown`.

A plugin adds `session` to your hub calls for you. Without one, pass it
yourself on every write. A write to a session that does not exist creates it
and joins you. A write with no `session` records member and agent only, and
anything addressed to you arrives on the member-and-agent cursor instead of a
session's. A write to `store: "session"` with no `session` is refused with
`session_required`.

Reads take the same argument. `brain_get`, `brain_list` and `brain_history`
read any session you may read in a project, yours or not, and a read never
creates anything. A bare name with no project resolves among the live sessions
you take part in: exactly one matches, or the call is refused with
`session_ambiguous`.

A write names a project's session only when the write is in that project. A
write to another project records no session, and the result says so:
`"session": null, "session_note": "other_project"`.

## Starting, resuming, leaving, ending

`session_start(project_id, session_name)` joins the session, creating it when it
is new, and returns a `session_id`, the namespaces to address the brain with,
`recovery_path`, the file where a participant leaves the note that orients
whoever comes next, and `handoff`: the note left when the session ended, so a
resume reads what its predecessor wrote without a further call. An ended session
named again is reopened.

`session_start(..., exclusive: true)` refuses a name already in use, live or
ended, with `conflict` and `session_name_taken`, and `details.session_id`
names it. Use it when you must not share. A plugin does, to name a subagent's
session.

| Call | Effect |
|---|---|
| `session_leave(session)` | you stop being a live participant. Nothing ends for the others, and your next call joins again |
| `session_end(session, handoff?)` | the session is ended for everyone, and its live children end with it. Any participant may |

Leave when you stop and others may carry on. End when the work is done, and
leave the note first: `handoff` keeps up to 4096 characters on the session and
in the human's feed. The note is not stored in the brain. Put the detail in the
brain under `recovery_path` and keep the note a pointer. The human can end a
session too.

A session is live while it is active and a participant has made a call
recently and has not left.

## The brain

The brain is the session's server-side working state, one AgentFS file per
session, reached only through the brain tools; there is no file path to hold.
Every participant reads and writes it. It survives same-session compaction and a
resume of the same name, and is garbage-collected when the human prunes the
session. Keys live under `/kv/`, files under `/fs/`. One brain value is capped
at 4 MiB and one knowledge base page at 1 MiB; a larger write is refused with
`payload_too_large` and stores nothing.

Every version of a session brain is kept, and `brain_history` and
`brain_revert` take `store: "session"`. The hub serialises writes. To keep two
participants from overwriting each other:

- pass `if_version` from your last read, and a different current version is
  refused with `conflict` and `stale_base`, with `details.version` the current
  one;
- without `if_version`, a put, delete or revert of a key whose last write came
  from a different participant is refused the same way. Your own last write
  needs no base.

On `stale_base`, read the key again, merge, and write with the new version.

If the name you ask for is held by a session the human has pruned, the call is
refused with `session_pruned`, because the human can still undo that prune:
start under another name or ask for the undo.

## Children

A subagent works in its own session named `<parent>/<task>`, for example
`feat-redesign/review-a1b2c3d4`. The plugin names it, not the model. A child's
brain is its own, so the parent's brain is untouched.

- A `/` is accepted only when every prefix is a live session in the same
  project. Otherwise: `session_parent_missing`, and the message names a flat
  name to use instead.
- Ending a parent ends its live children. Pruning a parent prunes them.
- Reopening a child needs a live parent.

Without a plugin, a subagent you start yourself passes its child's `session`
on every call.

## Compaction

Anchor before a long step with `session_anchor`, and after a compaction read
`session_resume` for the latest anchor and the events after it.

## Forking another session

To branch from work you want to keep separate, fork it. The source is not
touched and keeps working.

```
session_list(project_id: "homelab", status: "ended")
session_start(project_id: "homelab", session_name: "migration",
              from: {name: "nightly"})
```

`from` is `{session_id}`, or `{name, project_id?}` with a project that defaults
to the call's. The new session's brain is a copy of the source as it stands, and
later writes stay on their own side. The result's `forked_from` names the
source. A fork is never the way to carry on: to continue the same session, join it by name.

`from` is refused when the source was pruned (`session_pruned`; the human can
restore it with undo, the hub will not), or when it is in another project
(`invalid_argument`: a brain is project-scoped).

## Which store to write to

The four brain tools reach two stores, and `store` says which:

- `store: "session"` is this session's working state: notes to yourself,
  scratch files, a recovery handoff. It is pruned with the session, and only the
  session's participants and readers of the project see it.
- `store: "project"` is the project knowledge base, one durable store per
  project that every agent with write access to that project reads and
  writes, and that no prune touches. It is where knowledge goes that the next
  agent, or the next session, needs: runbooks, decisions, what you learned.
  `project_id` names the project and defaults to the session's. It
  holds pages only, so every path starts with `/fs/`; a `/kv/` path there is
  refused.

`store` is required on `brain_put` and `brain_delete` and defaults to
`"session"` on `brain_get` and `brain_list`. Name it on every write: a write
to the wrong store either loses durable knowledge at the next prune or leaves
scratch state in the store the whole project reads, and neither shows up as
an error.

Your member's personal space is a project like any other, so
`brain_put(store: "project", project_id: <your personal space>)` is a durable
store that follows you across projects. Other members and the human can
read it; only you can write it.
