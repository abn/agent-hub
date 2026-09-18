---
type: Decision Record
title: Sessions belong to their agent and are picked up self-serve
description: A session brain has one owner, another agent takes it over or branches from it without the human, and the hub chooses which of the two it is.
tags: [adr, sessions, brain, ownership, handoff]
status: stable
---

# 0020. Sessions belong to their agent

## Context

A session name was unique per project and a resume never compared the caller.
Two agents that both chose `nightly` in one project therefore landed in one
session and wrote one working-state file, each overwriting what the other had
just stored, with nothing in any surface to say it had happened.

The opposite gap sat beside it. When an agent stopped, its state was reachable
to read but there was no way to take it over, so work stalled until the human
noticed. Nothing listed sessions to an agent either, so an agent could not even
see that a sibling had left something behind.

Ownership here is about coherence, not defence. Agents on this hub are the
operator's own; the problem is two writers on one file, not a hostile one.

## Decision

A session belongs to the agent that started it. A live session name is unique
per `(project, agent)`, so the same name under another agent is a different
session with its own brain, and each agent resumes its own. The uniqueness is
a partial index over live rows: a session inside its prune undo window no
longer holds its name against the lookups, all of which skip soft-deleted rows.
Re-taking such a name is refused with a `conflict` naming the pruned session,
because the human's undo still points at it and a second session behind that
token would be a surprise.

Pickup is self-serve, and the hub chooses the mechanism from the source's
state rather than letting the caller name it: from outside, an agent cannot
tell a running session from a stopped one, and a wrong guess either forks a
dead session or adopts a live one. `session_start` takes a `from` reference,
and an **ended** source is **adopted**, keeping its id, its brain file, its
search rows and its handoff note while ownership moves; an **active** source is
**forked** into a new session whose brain is copied and whose lineage is
recorded, with the source neither ended nor notified. The resolution, the
checks and the write share one immediate transaction with rows-affected
guards, so two agents reaching for one ended session cannot both be told they
got it.

The copy runs through the engine, `VACUUM INTO` on the source's own
connection, under the same per-file write lock a prune takes. It carries every
table, the append-only tool call log included, which is the provenance a
logical walk of the entries would drop. A copy is taken under a re-check that
the source is still there, and a failed copy leaves neither a partial file nor
a session row.

Reads stay open: any agent that may read a project reads the brains of the
sessions in it, and any agent that may write a project may adopt an ended
session there. Ending a session is the owner's alone, and carries an optional
handoff note that lives on the session row and in the feed event, never in the
brain, so ending a session that never wrote still leaves no brain file.

The human is informed and never asked. Adopt, fork, reassign and
end-with-handoff appear in the feed as ordinary session events that need no
action. The human's only move here is reassigning a running session when the
agent holding it is not coming back.

## Consequences

- Two agents that pick one name get two brains instead of one corrupted one,
  and the silent merge that used to hide behind a shared name is gone.
- Work outlives the agent doing it without a human in the loop: an agent
  reads the listing, picks the session up, and continues from the note and the
  brain it inherits.
- A forked brain is a real second file, so a fork costs the source's size on
  disk, and the human prunes it like any other session. Adopting costs
  nothing, since only the owner column moves.
- Lineage ids outlive what they name. A source that is later pruned leaves a
  dangling reference, which the surfaces render as a session that is gone
  rather than repairing or hiding it.
- Ownership is a coherence rule, not a security boundary: an agent with
  project write can take an ended session there without asking. That is the
  point, and it is why the rule is stated in the surfaces rather than enforced
  as a privilege.
