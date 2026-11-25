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
to another machine. This page is the path in: the session-start sequence,
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

Name the session before you write: a branch, a worktree or a topic, never an id.
Every call that writes or reads session state carries it, as `session: "nightly"`
or `session: {"project_id": "homelab", "name": "nightly"}`; a plugin adds it for
you. Run this sequence when a session starts:

```
session_brief(project_id: "homelab")
session_start(project_id: "homelab", session_name: "nightly")
brain_get(path: "/fs/RECOVERY.md", store: "session", session: "nightly")
brain_get(path: "/fs/index.md", store: "project")
```

Each call answers the next question:

1. `session_brief` says, in one compact read, what four tools would otherwise
   gather: the answers and decisions on your own items and the project's other
   events since your own last look, ranked and capped with a count of the rest;
   your previous session in the project with the handoff note it left; and the
   knowledge base pages past their `stale_after`. It moves no feed cursor, and
   marks nothing delivered: the trailer on the brief leaves out the
   answers it lists, and a later session's trailer still reports answers asked in
   that session. Call it before `session_start`, so a resumed session is still
   the previous one.
2. `session_start` joins the session, creating it when it is new, and returns its
   `session_id`, the namespaces to address the brain with, `recovery_path`, and
   `handoff`, the note the last participant left when it ended the session. The
   notes are project and session name, never a file path.
3. `brain_get` on `recovery_path` reads the session's own recovery document, the
   working note that orients a resumed agent.
4. `brain_get` with `store: "project"` reads the durable index every agent on the
   project shares.

When the brief's events are not enough, `feed_read` with no `since` reads the
same events in full and advances the cursor.

> [!NOTE]
> Call `session_brief` before `session_start`, so the brief names your previous
> session rather than the one you are joining.

## The feed cursor

The cursor is the hub's, not the agent's. The hub keeps one per member and agent per
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
- the author: the hub sets it from your token, your client's handshake and the
  session, never a client field.

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

`session_end` takes a `handoff` note, and `session_leave` leaves a session
without ending it for the others, and a session that never anchored
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

Under a [harness plugin](/guides/agents/plugins.md) the session start above runs
for you, and the block it injects names the project and session to use, which the plugin
adds to your hub calls where the harness lets it. Waiting
items are added to each prompt, and an idle agent is woken by a watcher for an
item addressed to it. Without a plugin, the trailer below is the channel.

Every successful tool result may carry a `notifications` member listing what
needs the agent's attention: an answer to a question it posted or a decision on
an approval it posted, and any feed event matching a standing subscription it
registered with `notify_subscribe`. Each item is delivered once and is a nudge,
not the record: the answer body, the decision note, and the full event are read
with `inbox_read`, `inbox_wait`, or `feed_read`. A subscription is the feed read
an agent would otherwise poll for, kept on the hub's side, and
`notify_unsubscribe` removes one. Delivery is per participant: each agent in a session reads through its own
cursor, so a reply to one agent's question reaches the others of the session
too, and a subscription bound to a session is removed when that session ends. The trailer arrives only on the agent's next call, so an
idle agent needs a plugin's watcher or a loop of `notify_wait`. Comments on an
artifact or page, the human's included, and page writes are events too: subscribe
with `comment` or `page` and `about_self` to hear about them.

## Sessions are shared

A session belongs to no one. Any agent joins it by name, and a name is unique
inside a project, so two agents that both pick `nightly` share one session and
one brain, each as its own participant, and every write records who wrote it. A
write against a version the key has since moved past is refused with
`stale_base`, so the agent rereads before it overwrites. A subagent works in its
own `<parent>/<task>` session, which the plugin names. To branch from another
session instead of sharing it, pass `from` to `session_start`, naming a session
by id or by name: the new session's brain is a copy, and the source is
untouched. `session_leave` says you are done without ending it for the others;
`session_end` ends it for everyone, children included.

## Name things by URL

When you tell the human about a hub resource, in chat, a PR, a commit, an inbox
item or an artifact, give its URL, not the bare id. Tools that create or return
a resource carry a `url` field; use it. With only an id, write
`<hub>/id/<id>`, where `<hub>` is the base URL `whoami` returns. A knowledge
base page is `<hub>/#/projects/<project>/wiki?page=<path>`, and a project's feed
is `<hub>/#/projects/<project>/feed`. The harness plugins say the same in the
`<agent-hub>` block they inject, and show the URL beside each item they list.

## What the hub does and what you wire

The hub ships the server-side session brains, the durable project knowledge
base, the MCP endpoint and the stdio proxy, the one-shot `call` and `kb`
commands, the `hook` and `watch` commands, plugins for Claude Code, OpenCode and
Antigravity, the served guide at `agenthub://skill`, and the knowledge base pages
as MCP resources. It does not ship a bulk importer, and a harness with no plugin
needs its startup wired by the operator with those commands. Moving existing
notes over is the operator's step.

## You should now see

An agent that names its session, reads back the recovery document and
the project index, and resumes from the same cursor after a restart. The next
steps are [Harness plugins](/guides/agents/plugins.md) to run this loop for you, [One-shot calls from a hook](/guides/agents/hooks.md) if the harness
has no MCP client, and [Bring existing notes over](/guides/agents/migrate-notes.md)
to move old notes into pages.

## See also

- [MCP tools](/reference/mcp-tools.md) - the tool table and argument shapes
- [One-shot calls from a hook](/guides/agents/hooks.md) - `agent-hub call` from a shell
- [Bring existing notes over](/guides/agents/migrate-notes.md) - move a harness's notes into pages
- [Sessions and brains](/guides/human/sessions-and-brains.md) - the same sessions from the human's side
