# Sessions, the brain, and the stores

## Before you start

`session_brief(project_id)` is the first read in a project. `previous_session`
is your most recent other session there, with the handoff note it left and its
`recovery_path`, so `brain_get(recovery_path, session: {session_id})` reads the
detail it points at. Call it before `session_start`: it never counts the
session your connection is working as the previous one, so after a resume it
would name the session before the one you resumed.

## Sessions and the brain

`session_start` takes a `project_id` and a `session_name` and returns a
`session_id`, the namespaces to address the brain with, and `recovery_path`,
the file where a session leaves the note that orients whoever comes next. It
also returns `handoff`: the note the previous owner left when it ended the
session, so a resume reads what its predecessor wrote without a further call.
`session_start` returns no feed cursor of its own: `feed_read` keeps your cursor
per project on the server and advances it on every read, so a resumed session
continues where it stopped without carrying the `next_since` itself. The
brain is the session's server-side working state, one AgentFS file per
session, reached only through the brain tools; there is no file path to hold.
It survives same-session compaction and a resume of the same name, and is
garbage-collected when the human prunes the session. Keys live under `/kv/`,
files under `/fs/`. One brain value is capped at 4 MiB and one knowledge base
page at 1 MiB; a larger write is refused with `payload_too_large` and stores
nothing.

A session belongs to the agent that started it. A session name is yours: the
same name under another agent is a different session with its own brain, so
two agents that happen to pick `nightly` never share working state. Only the
owner ends a session. If the name you ask for is held by a session the human
has pruned, the call is refused with `conflict` and a `pruned_session_id=`
tail, because the human can still undo that prune; start under another name or
ask for the undo.

`brain_get` and `brain_list` take an optional `session` and read another
session's brain: either `{session_id}`, or `{agent, name}` with a `project_id`
that defaults to your active session's project. Anyone who may read a project
may read the brains of the sessions in it, so an agent can see what a sibling
is working from, and reading needs no session of your own. A read never creates
anything. A session the human has pruned is `not_found` while it can still be
restored, and after that it reads like any session that never existed.

A write names your own session, or takes the one your connection is working:
`brain_put` and `brain_delete` take an optional `session`, by `{session_id}` or
by `{agent, name}`, and write the session it names. That is how a one-shot call
writes, since it holds no connection to keep a session active on. Another
agent's session is refused with `forbidden` and an `owner=` tail: two agents
writing one working-state file clobber each other, which is the whole reason a
session has one owner. Knowledge meant for another agent belongs in the project
knowledge base, which is built to be written by everyone. With no `session`
named, the write is your connection's active session, and a connection with
none is a `conflict`.

## Picking up another agent's work

When an agent stops, crashes, or is running something you want to branch from,
you take the work yourself. The human is not involved.

```
session_list(project_id: "homelab", status: "ended")
session_start(project_id: "homelab", session_name: "migration",
              from: {agent: "deploy-bot", name: "nightly"})
```

`from` names one session, the same shape as a cross-session read:
`{session_id}`, or `{agent, name}` with a `project_id` that defaults to the
project of the call. **The hub decides what picking up means**, because you
cannot tell from outside whether that session is still running:

- the source has **ended**: the hub **adopts** it. You get the same
  `session_id`, the same brain, and its handoff note. Nothing is copied, and
  the previous owner no longer holds it.
- the source is still **active**: the hub **forks** it. You get a new
  `session_id` whose brain is a copy of the source as it stands, and the
  source's owner keeps working undisturbed. Later writes on either side stay
  on their own side.

The result says which happened:

```
pickup: {mode: "adopt" | "fork", from_session_id, from_agent, handoff,
         source_active, note?}
```

A fork carries `source_active: true` and a `note` naming who holds the
original. Read it: it means that agent is still working from the same state
and the same handoff note you now have, so coordinate through the feed or pick
other work rather than doing the same thing twice.

`pickup` is `null` on an ordinary start or resume. Any agent that may write the
project may adopt an ended session there; picking up work is not a privilege
the human hands out. What is refused, and why:

- the name you asked for is already yours and live: `conflict` with
  `existing_session_id=`, so call again with another name.
- somebody else picked the ended session up a moment before you: no refusal.
  The session is active again under them, so you get a fork of it, and the
  result says so as above.
- the source was pruned: `conflict` with `session_id=`. The human can restore
  it with undo; the hub will not do that for you.
- the source is in another project: `invalid_argument`. A brain is
  project-scoped.

A session can also leave you. If the human ends or reassigns it and another
agent picks it up, your next write is a `conflict` that names the new owner.
Call `session_start` again: your own name gives you a fresh session, and
`from` gives you a copy of where the work now stands.

The owner of a session is the identity the hub saw when it was started: the
token's agent over HTTP or through the stdio proxy, and `HUB_AGENT_ID`
(`local` when unset) for a standalone `agent-hub mcp` that opens the data
directory itself. Moving from standalone stdio to the proxy therefore keeps
your sessions only when the two are the same string. When they are not, your
earlier work is still there under the old owner: list it with `session_list`
and pick it up with `from: {agent: "local", name: "..."}`.

Leave the note before you stop: `session_end(session_id, handoff: "...")` keeps
up to 4096 characters on the session and in the human's feed, and whoever picks
the session up gets it back in `pickup.handoff`. The note is not stored in the
brain, so ending a session that never wrote still leaves no brain behind. Put
the detail in the brain under `recovery_path` and keep the note a pointer.

## Which store to write to

The four brain tools reach two stores, and `store` says which:

- `store: "session"` is this session's working state: notes to yourself,
  scratch files, a recovery handoff. It is pruned with the session, and no
  other session sees it.
- `store: "project"` is the project knowledge base, one durable store per
  project that every agent with write access to that project reads and
  writes, and that no prune touches. It is where knowledge goes that the next
  agent, or the next session, needs: runbooks, decisions, what you learned.
  `project_id` names the project and defaults to the active session's. It
  holds pages only, so every path starts with `/fs/`; a `/kv/` path there is
  refused.

`store` is required on `brain_put` and `brain_delete` and defaults to
`"session"` on `brain_get` and `brain_list`. Name it on every write: a write
to the wrong store either loses durable knowledge at the next prune or leaves
scratch state in the store the whole project reads, and neither shows up as
an error.

Your own agent space is a project like any other, so
`brain_put(store: "project", project_id: <your personal space>)` is a durable
store that follows you across projects. Other agents and the human can
read it; only you can write it.
