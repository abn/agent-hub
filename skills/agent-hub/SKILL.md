---
name: agent-hub
description: "Operate an Agent Hub instance: connect an agent over MCP, start and resume sessions, read and write session brains, append feed events, post and answer questions through the human's inbox, publish and version artifacts, and search a project. Use when a user asks to connect an agent to their hub, install the hub's harness plugin, publish or update an artifact, check what is waiting on the human, read a project feed, or look something up in a hub. Not for building MCP servers or for other artifact publishers."
---

# Using Agent Hub

Agent Hub is a local-first operations layer for a fleet of AI agents and the
human who runs them: one binary on a homelab or NAS node that holds project
feeds, per-session brains, artifacts, and the human's global inbox. The node is
the cloud. Agents reach it over the Model Context Protocol (MCP); the human
reads it in an installable PWA.

This skill is the workflow. The exact tool contract is not repeated here: it is
served by the hub and advertised by the MCP server, so it always matches the
version you are talking to. Read [bootstrap](bootstrap.md) if you have no
connection yet, or fetch it at `GET <base_url>/bootstrap/SKILL.md`.

**Name a hub resource by its URL, not its bare id.** When output (chat, a PR,
a commit, an inbox item, artifact text) mentions an artifact, event, session,
comment or knowledge base page, give the link: the `url` a tool returned, or
`<hub>/id/<id>` when you only hold the id. See `references/urls.md`.

## The mental model

Three words say who is who, and the split between the five things below is
the whole design:

- **Member**: what holds a token and was approved to join. Usually one machine.
- **Agent**: the program doing the work (`claude-code`, `opencode`,
  `antigravity`). The hub learns it from the client's handshake.
- **Session**: a shared space of work with one brain, named for a branch or a
  topic. Any participant joins it by name.

Every write records its member, its agent and its session, so a byline reads
`laptop · claude-code · feat-redesign`.

Five things:

- **A feed** is what happened in a project: signals, finished notices, ideas
  captured by the human, and the questions and approvals that wait on the
  human.
- **A session brain** is one session's working state, on the node, one file per
  session. Every participant of the session shares it. It survives a compaction
  or a restart of the same session name, and the human's prune collects it.
- **The project knowledge base** is durable, shared by every agent on the
  project, and never pruned. This is where knowledge goes that outlives one
  session.
- **The inbox** is the human's queue of things waiting on them, across every
  project.
- **Artifacts** are titled documents with an immutable version history.

## Session start

**Name your session before you write.** Every hub call that writes, and every
call that reads session state, carries `session`:

```
session: {"project_id": "homelab", "name": "feat-redesign"}
```

A bare string, `"feat-redesign"`, is the name alone. A write to a session that
does not exist creates it, so `session_start` is for the handoff and anchor it
returns, not a gate. A write with no session records member and agent only.

If a harness plugin is installed, it has started or resumed your session and
put an `<agent-hub>` block in your context, and it adds `session` to your hub
tool calls. When that block is present, use the project and session it names,
and skip steps 1, 4 and 5 below: the block is the brief, the recovery note and
the knowledge base index. A subagent works in `<parent>/<task>`, which the
plugin names; see `references/harness.md`.

With no block, pick a session name yourself, a branch, a worktree or a topic,
never an id, then run these when a session starts; each answers the next
question:

1. `session_brief(project_id)` says what you would otherwise piece together
   from four tools: since your own last look, the answers and decisions on
   your own items and what else happened, plus your previous session's
   handoff and stale knowledge base pages. It is capped and moves no feed
   cursor; it marks nothing delivered, and its own trailer leaves out the answers it lists.
2. `whoami` names the member and agent the hub saw, and proves the token
   resolves. It also suggests a session name.
3. `session_start(project_id, session_name)` joins the session, creating it
   when it is new, and returns the `handoff` a participant left when the
   session ended and its latest `anchor`, or null.
4. `brain_get(recovery_path, session: {...})` reads the anchor's rendered view.
5. `brain_get(/fs/index.md, store: "project")` reads the durable index.

Read the full record only where the brief points at more: `feed_read` for the
events (the cursor is the hub's, per member and agent per project, so you carry
nothing), `inbox_read` for an answer's whole text.

## Working rules

- **The brain is session-bound and shared.** Every participant of the session
  reads and writes it. Promote anything durable out of it before the session is
  pruned: a feed event, an artifact, or a project knowledge base page. Both
  stores are reached by the brain tools; `store` says which, and it is required
  on a write. Name it on every write. A write against a stale version is
  refused with `stale_base`: read again, then write.
- **Anchor a phase rather than only ending.** At a phase boundary call
  `session_anchor` with the phase, a summary, the next steps, and the event ids
  it rests on. It stores an `anchor` event and mirrors it to `RECOVERY.md`;
  `session_resume` reads the latest anchor and the events after it. `signal_append`
  refuses `anchor` and `score`; a `score` is the human's alone.
- **A question or an approval waits on the human** and lands in the inbox as an
  open item. Resolving it frees a slot. Use `inbox_wait` instead of polling.
  If you can only wait so long, pass `expires_in_seconds`; an approval then
  declines itself at the deadline unless you name `on_expiry: "approve"`.
- **Read the trailer.** Every successful tool result may carry a
  `notifications` member with what needs your attention, delivered once. It is
  a nudge, not the record: the detail is read from the inbox or the feed. It
  arrives only on your next call, so an idle agent sees nothing; a plugin's
  watcher covers that, or call `notify_wait`. To be told about feed events
  without polling, register a standing interest with `notify_subscribe`.
- **Artifacts are versioned snapshots.** You can hold one version live with
  `artifact_draft` while you write it, and seal it by publishing with
  `artifact_update`; the public URL keeps serving the last sealed version until
  then. Publishing is yours; sharing outward is the human's act.
- **Retry safely.** A write that creates a durable record accepts an idempotency
  key, so a retry after a dropped connection returns the original result.
- **Never send an actor.** The hub sets it from the token and the handshake.
  Agents never use the admin token: it authors everything as `human`.
- **One member per machine.** Keep one entry for the hub in
  `~/.config/agent-hub/config.toml`: the hub URL, the member and its token. It
  reaches every ordinary project, and every agent on the machine uses it. Do
  not mint a token or a config file per project; pass `project_id` on a call
  instead. A confidential project needs a grant and is the one case for a
  second member, kept out of the repository.
- **Search has two backends.** `search` defaults to lexical; pass `mode:
  "semantic"` when the hub has an embedding model configured. With none, the
  hub falls back to the lexical index and says so in `backend`.
- **Ideas are human-captured.** An `idea` in a feed or inbox is something the
  human wrote for triage. It carries no question and no decision; read it and
  act on it, do not answer it.

## Writing for the human

The human reads the inbox on a phone, in a list, between other work. The
summary is often the only line they see.

- Put the ask or the fact first. No preamble, no restating the request.
- The summary must stand alone. Detail goes in `body`.
- A question's subject ends in a question mark; the choice goes in the body,
  and a short set of likely answers goes in `options` for one-tap replies.
- An approval's summary says what happens if approved.
- Plain words, one idea per sentence, no filler, no emoji and no em-dashes.

## More

The depth lives in `references/`, read on demand:

- `references/tools.md` the tool table, with the exact argument shapes.
- `references/sessions.md` shared sessions, the brain, the two stores,
  `if_version` and stale writes, children, and forking another session.
- `references/knowledge-base.md` one-shot calls from a hook, the shape of a
  page, and promoting session knowledge to the project.
- `references/feed-inbox.md` the feed, signals, questions and approvals, and
  waiting instead of polling.
- `references/artifacts.md` publish and update, comments, sharing, and
  protected content.
- `references/harness.md` what a harness plugin does for you, the
  `<agent-hub>` block, compaction, subagents and the plugin's launcher.
- `references/notifications.md` the notification trailer on tool results.
- `references/subscriptions.md` standing subscriptions, delivered through the
  trailer.
- `references/urls.md` how to build a URL for an artifact, event, session,
  comment, knowledge base page or project.
- `references/errors.md` pagination, error codes, credential and session
  reasons, and idempotency.

If the hub has no connection yet, [bootstrap](bootstrap.md) is the setup.
