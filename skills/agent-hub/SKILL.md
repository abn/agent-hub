---
name: agent-hub
description: "Operate an Agent Hub instance: connect an agent over MCP, start and resume sessions, read and write session brains, append feed events, post and answer questions through the human's inbox, publish and version artifacts, and search a project. Use when a user asks to connect an agent to their hub, publish or update an artifact, check what is waiting on the human, read a project feed, or look something up in a hub. Not for building MCP servers or for other artifact publishers."
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

## The mental model

Five things, and the split between them is the whole design:

- **A feed** is what happened in a project: signals, finished notices, and the
  questions and approvals that wait on the human.
- **A session brain** is one session's working state, on the node, one file per
  session. It survives a compaction or a restart of the same session name, and
  the human's prune collects it.
- **The project knowledge base** is durable, shared by every agent on the
  project, and never pruned. This is where knowledge goes that outlives one
  session.
- **The inbox** is the human's queue of things waiting on them, across every
  project.
- **Artifacts** are titled documents with an immutable version history.

## Session start

Run these when a session starts; each answers the next question:

1. `whoami` names the identity and proves the token resolves.
2. `feed_read(project_id)` reads what happened since your own last look. The
   cursor is the hub's, per agent per project, so you carry nothing.
3. `brain_get(recovery_path)` reads the note the previous owner left.
4. `session_start(project_id, session_name)` makes a brain active and returns
   the `handoff` the predecessor left when the session ended.
5. `brain_get(/fs/index.md, store: "project")` reads the durable index.

## Working rules

- **The brain is session-bound.** Promote anything durable out of it before the
  session is pruned: a feed event, an artifact, or a project knowledge base page.
  Both stores are reached by the brain tools; `store` says which, and it is
  required on a write. Name it on every write.
- **A question or an approval waits on the human** and lands in the inbox as an
  open item. Resolving it frees a slot. Use `inbox_wait` instead of polling.
  If you can only wait so long, pass `expires_in_seconds`; an approval then
  declines itself at the deadline unless you name `on_expiry: "approve"`.
- **Read the trailer.** Every successful tool result may carry a
  `notifications` member with what needs your attention, delivered once. It is
  a nudge, not the record: the detail is read from the inbox or the feed. To be
  told about feed events without polling, register a standing interest with
  `notify_subscribe`.
- **Artifacts are versioned snapshots.** You can hold one version live with
  `artifact_draft` while you write it, and seal it by publishing with
  `artifact_update`; the public URL keeps serving the last sealed version until
  then. Publishing is yours; sharing outward is the human's act.
- **Retry safely.** A write that creates a durable record accepts an idempotency
  key, so a retry after a dropped connection returns the original result.
- **Never send an actor.** The hub sets it from the token.
- **One credential.** Keep one client config with the hub URL and your token: it
  reaches every ordinary project. Do not mint a token or a config file per
  project; pass `project_id` on a call instead. A confidential project needs a
  grant and is the one case for a separate credential, kept out of the
  repository.

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
- `references/sessions.md` sessions, the brain, the two stores, if_version,
  and picking up another agent's work.
- `references/knowledge-base.md` one-shot calls from a hook, the shape of a
  page, and promoting session knowledge to the project.
- `references/feed-inbox.md` the feed, signals, questions and approvals, and
  waiting instead of polling.
- `references/artifacts.md` publish and update, comments, sharing, and
  protected content.
- `references/notifications.md` the notification trailer on tool results.
- `references/subscriptions.md` standing subscriptions, delivered through the
  trailer.
- `references/errors.md` pagination, error codes, and idempotency.

If the hub has no connection yet, [bootstrap](bootstrap.md) is the setup.
