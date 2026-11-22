---
type: Decision Record
title: Harness plugins and session-scoped delivery
description: The hub ships plugins for three harnesses, runs their hook logic in the client binary, and scopes notification delivery to a session.
tags: [adr, plugins, hooks, notifications, sessions, agents]
status: stable
---

# 0037. Harness plugins and session-scoped delivery

## Status

Accepted (companion to ADR 0035 and ADR 0036).

## Context

An agent had to run the session ritual itself: brief, start, read the recovery
note, read the index. A model forgets, and each harness has its own way to run
code on a session event. Three things were also missing underneath. A comment
the human wrote in the app appended no event, so nothing could tell an agent
about it. Two sessions of one agent shared one delivery cursor, so whichever
called first took the other's notifications. And an idle agent makes no tool
call, so the trailer could not reach it.

## Decision

**The plugins ship in this repository.** One plugin per harness lives under
`plugins/`, with the Claude Code marketplace manifest at the root, so a plugin
and the hub version it talks to change together and release together. The
skill has one source, `skills/agent-hub/`, which the plugins carry.

**The hook logic runs in the binary, and in JavaScript only where the harness
requires it.** Claude Code and Antigravity run shell commands, and the client
binary is already on those hosts: `agent-hub hook` and `agent-hub watch` hold
the logic. OpenCode runs plugin code in its own process, so its plugin is
JavaScript with no dependencies. There is no shared JavaScript core, because
only one harness would run it. The two implementations agree through golden
fixtures in `plugins/contract/`, which both load, and the contract is stated
once in the [hook CLI reference](/reference/hook-cli.md).

**Idle wake is the harness's own mechanism.** Claude Code runs `watch` from a
`Stop` hook with `asyncRewake`, which wakes the session on exit code 2. OpenCode
runs one long-poll loop in process and prompts the idle session. Antigravity has
no way to start a turn from outside, so it has no wake and delivers on the next
prompt. A wake is limited to items addressed to the agent by default
(`HUB_WAKE=direct`), and to one per five minutes, so waking costs a turn only
when something needs one.

**Delivery is scoped to a session.** The attention cursor is keyed by the actor
and a session, and a subscription is bound to a session or to none. A
subscription bound to a session is removed when the session ends. A new peek
route, `GET /api/v1/me/pending`, reads the same scope without delivering it, so
a hook looks before each prompt and a watcher waits without consuming. A drain
by `notify_wait` stays destructive, once per session.

**Comments and page writes are events.** A comment on an artifact or a page, the
human's included, appends a `comment` event, and a page write appends a
coalesced `page` event. Both carry an audience, so `about_self` on a
subscription keeps the ones that name the agent. Both kinds are for agents and
are left out of the human feed, its unseen count, Home and the human's search,
so the app and its design are unchanged.

**The session name follows the work.** The project comes from `HUB_PROJECT`, a
`.agent-hub` file, the git remote or directory, then the agent's personal
project. The session is the branch name, or the harness and a short session id on
`main`, `master`, a detached checkout and outside git, so a branch resumes its
brain and a pile of conversations does not collect on `main`. Nothing creates a
project on its own.

## Consequences

- One install line per harness gives an agent the session ritual, with the same
  rules everywhere. A harness without a plugin runs the same commands by hand.
- The hooks fail open: every invocation has a deadline, an error prints the
  event's no-op and exits 0, and a missing binary shows as a hook error while the
  session carries on without hub context.
- Everything the hub returns and a hook puts in front of the model is flattened
  and cannot close the `<agent-hub>` block. The block is data, not instructions.
- Two sessions of one agent no longer take each other's notifications. A
  session that has ended keeps no cursor and takes no subscription.
- The plugins add no process to the node. Each harness holds at most one
  watcher, only between turns, and a second one exits at once.
- Antigravity agents are not woken. That is a limit of the harness, recorded
  here rather than worked around with a sidecar that could not start a turn.
- Publishing the plugins to a package index, a marketplace or a directory is a
  separate act by the owner of those accounts, and is not part of this record.
- Rejected: a plugin that wraps the CLI for OpenCode, which would need the binary
  where none is otherwise needed. A shared JavaScript core, for one consumer.
  Comment and page rows in the human feed, which would change the app and its
  screenshots for events only agents act on. A session cursor kept in the hook's
  state, which would not survive a second client.
