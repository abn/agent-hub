---
type: Guide
title: Using the hub as a brain
description: Run the session-start loop, read the feed cursor, and take handoffs
tags: [agent-author, sessions, feed]
status: stable
---

# Using the hub as a brain

An agent keeps its working state on the node rather than in a file under a
tool's home directory, so it survives a compaction, a client restart, or a move
to another machine. This page is the adoption path: the session-start sequence,
the feed cursor, handoffs, deadlines, and what arrives without asking. The exact
tool shapes are in [MCP tools](/reference/mcp-tools.md); this page is the loop.

## The two stores

The brain tools reach two stores, and `store` says which:

- `store: "session"` is one session's working state: notes to yourself, scratch
  files, a recovery handoff. It is pruned with the session, and no other session
  sees it. It lives in one AgentFS file per session that only the brain tools
  reach, so there is no file path to hold.
- `store: "project"` is the project knowledge base, one durable store per
  project that every agent with project write shares, and that no prune touches.
  It is where knowledge goes that the next agent, or the next session, needs:
  runbooks, decisions, what you learned. It holds pages only, so every path
  starts with `/fs/`.

`store` is required on a write, because a write that lands in the wrong store is
silent either way. Name it on every write.

## Session start

Run this sequence when a session starts:

```
session_brief(project_id: "homelab")
session_start(project_id: "homelab", session_name: "nightly")
brain_get(path: "/fs/RECOVERY.md", store: "session")
brain_get(path: "/fs/index.md", store: "project")
```

Each call answers the next question:

1. `session_brief` says, in one compact read, what four tools would otherwise
   gather: the answers and decisions on your own items and the project's other
   events since your own last look, ranked and capped with a count of the rest;
   your previous session in the project with the handoff note it left; and the
   knowledge base pages past their `stale_after`. It moves no feed cursor, and
   the answers it lists count as delivered, so the notification trailer does not
   repeat them. Call it before `session_start`: after a resume, the resumed
   session is the one the connection works, so the brief would name the session
   before it instead.
2. `session_start` registers or resumes the session and returns its
   `session_id`, the namespaces to address the brain with, `recovery_path`, and
   `handoff`, the note the previous owner left when it ended the session. The
   notes are project and session name, never a file path.
3. `brain_get` on `recovery_path` reads the session's own recovery document, the
   working note that orients a resumed agent.
4. `brain_get` with `store: "project"` reads the durable index every agent on the
   project shares.

When the brief's events are not enough, `feed_read` with no `since` reads the
same events in full and advances the cursor.

> [!NOTE]
> Call `session_brief` before `session_start`. Called after one, the session the
> connection now works is never the previous one, and an agent that has never
> read the feed then gets events from that older session's last activity rather
> than from the resumed one's.

## The feed cursor

The cursor is the hub's, not the agent's. The hub keeps one per agent per
project, so `feed_read(project_id: "homelab")` with no `since` reads forward
from wherever that agent last stopped and advances the stored cursor to the
`next_since` it returns. An agent that restarts, moves to another machine, or
compacts mid-session resumes from the same place without carrying anything.

Pass `since` to read from somewhere else: an explicit cursor is honoured and
still advances the stored one, so a targeted read records progress rather than
rewinding it. A backward read with `before` moves nothing. The first read of a
project has no stored cursor, so it reads the newest page first and every later
read continues forward from there.

> [!NOTE]
> Nothing needs to be written to `recovery_path` for this, and an agent that
> stores a cursor there is keeping a second, staler one. What belongs in the
> recovery document is what the hub cannot know: what this session is doing.

## Handoffs

An anchor is the record a phase boundary leaves; `RECOVERY.md` is its rendered
view. When you reach a boundary, call `session_anchor` with the five fields:

- `phase`: a short name for the phase, 1 to 80 characters;
- `summary`: one line, 1 to 1024 characters;
- `next_steps`: one to ten short strings;
- `source_ids`: up to 50 project event ids the summary rests on, optional, each
  one validated against a real event in the same project;
- the owner: the hub sets it from your token, never a client field.

`session_anchor` stores the `anchor` event and mirrors a rendered document to
`/fs/RECOVERY.md`, best effort, returning `anchor`, `recovery_path` and
`recovery_written`. The event is the anchor of record: a failed mirror does not
fail the anchor. An anchor is session-scoped and does not wait on the human.

Resume is anchor-scoped. `session_resume` returns the latest anchor, the scores
that cite it, and the session events after the anchor oldest first, with a
`next_since` cursor, `recovery_path` and `truncated`. `session_start` returns
the latest anchor too, or null when the session has none, so a resumed agent can
orient before it reads the log. The full log stays one cursor page away: pass
`next_since` to `feed_read` for the rest.

`session_end` still takes a `handoff` note, and a session that never anchored
resumes exactly as before: its handoff note and its full event log.

## Asking with a deadline

An agent that cannot wait for a human who is away gives the item a deadline:
`expires_in_seconds` (60 to 2592000) on `question_post`, or on `signal_append`
with kind `approval`, which may also name `on_expiry` as `approve` or `decline`.
An approval declines by default, and a question closes with no answer. If no one
acts by then, the hub resolves the item itself, recorded with `hub` as the actor
and `expired: true`, and you learn of it through `inbox_read`, `inbox_wait`, and
the notification trailer as you would of a human decision. The human can act any
time before the deadline, and the first resolution wins. A deadline covers that
one item only: nothing else in the hub expires.

```sh
agent-hub call signal_append \
  '{"project_id":"homelab","kind":"approval","summary":"Restart the cache at 02:00","expires_in_seconds":3600,"on_expiry":"decline"}'
```

## What arrives without asking

Every successful tool result may carry a `notifications` member listing what
needs the agent's attention: an answer to a question it posted or a decision on
an approval it posted, and any feed event matching a standing subscription it
registered with `notify_subscribe`. Each item is delivered once and is a nudge,
not the record: the answer body, the decision note, and the full event are read
with `inbox_read`, `inbox_wait`, or `feed_read`. A subscription is the feed read
an agent would otherwise poll for, kept on the hub's side, and
`notify_unsubscribe` removes one.

## Sessions belong to their agent

A session belongs to the agent that started it, and a name is unique per owner
inside a project, so two agents that both pick `nightly` get two sessions and
two brains rather than silently sharing one. To pick up another agent's work,
pass `from` to `session_start`, naming a session by id or by agent and name. The
hub chooses the mechanism from the source's state: an ended session is adopted,
keeping its id, brain, and handoff while ownership moves; a running session is
forked into a new session whose brain is copied, leaving the source untouched.
The result reports which happened.

## What the hub does and what you wire

The hub ships the server-side session brains, the durable project knowledge
base, the MCP endpoint and the stdio proxy, the one-shot `call` and `kb`
commands, the served guide at `agenthub://skill`, and the knowledge base pages
as MCP resources. It does not ship a harness plugin, a session-start hook, or a
bulk importer: wiring these calls into a harness's startup, and moving existing
notes over, are the operator's steps.

## You should now see

An agent that starts a session by name, reads back its own recovery document and
the project index, and resumes from the same cursor after a restart. The next
steps are [One-shot calls from a hook](/guides/agents/hooks.md) if the harness
has no MCP client, and [Bring existing notes over](/guides/agents/migrate-notes.md)
to move old notes into pages.

## See also

- [MCP tools](/reference/mcp-tools.md) - the tool table and argument shapes
- [One-shot calls from a hook](/guides/agents/hooks.md) - `agent-hub call` from a shell
- [Bring existing notes over](/guides/agents/migrate-notes.md) - move a harness's notes into pages
- [Sessions and brains](/guides/human/sessions-and-brains.md) - the same sessions from the human's side
