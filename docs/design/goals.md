---
type: Concept
title: Goals and non-goals
description: What the first version covers and what it deliberately leaves out.
tags: [design, goals, scope]
status: draft
---

# Goals and non-goals

## Goals

- **Preserve signal.** Give every agent session a server-side brain whose life
  is bound to the session, so compaction and resume of the same named session
  do not lose working state.
- **Make updates global.** Answer "what happened in project X" as a
  first-class, time-ordered query for both agents and the human.
- **Give the human a home.** One calm surface for finished work, questions,
  approvals, artifacts, and pruning, reachable from a phone.
- **Stay lean.** One binary, one engine, and the smallest set of moving parts
  that delivers the above.
- **Stay local.** All data on the operator's node; no vendor cloud and no
  brain off the node.

## Non-goals for v1

- No vendor or remote cloud, and no synchronising a brain off the node.
- No multi-tenant organisations. The v1 shape is a single human with a fleet
  of agents.
- No real-time chat with agents. Interaction is asynchronous, under mailbox
  semantics.
- No automatic retention or expiry. The human prunes.
- No CRDTs, no custom distributed consensus, and no second search service.
- No reimplementation of AgentFS. The hub wraps the real thing, and FUSE is
  not used because agents are remote MCP clients.

## Milestone shape

Work proceeds in independently shippable milestones: a skeleton with the
engine and the AgentFS wrapper, then the feed, the brain, the inbox and Q&A,
artifacts, the PWA, search, security, and finally polish. The completion bar
is a single binary that runs on a node, where an agent can signal, keep a
brain across compaction, publish an artifact, and ask the human a question,
and the human can see all of it and prune.

The [architecture](../architecture/index.md) describes the intended design
those milestones build toward.
