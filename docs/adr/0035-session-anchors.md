---
type: Decision Record
title: Session handoffs are anchors and scores are derived facts
description: A phase boundary writes a compact anchor event, resume reads the latest anchor plus what came after it, and a human score is a derived event that cites the slice it grades.
tags: [adr, session, brain, context, scoring]
status: stable
---

# 0035. Session handoffs are anchors and scores are derived facts

## Status
Accepted (companion to ADR 0020 and ADR 0032).

## Context
The event store is already an append-only log. A session brain survives a
restart, and `session_end` can leave a handoff note, but there is no compact
checkpoint between phases. A resumed agent either reads the final note or pages
through the whole log. `RECOVERY.md` is a well-known path the hub hands to
every session start, but nothing writes it, so the path is often empty.

An assessment of the tape.systems fact model against this hub found that the
log is not the gap. The gap is the discipline between the log and the brain: a
small state written at each phase boundary, summaries that name their sources,
and a review step whose result is appended rather than folded into the run.

## Decision

**An anchor is an event.** Add the event kind `anchor`, written by the session's
owner at a phase boundary through MCP `session_anchor`. Its contract is five
fields:

- `phase`: a short name for the phase, 1 to 80 characters.
- `summary`: the event summary, 1 to 1024 characters.
- `next_steps`: one to ten short strings.
- `source_ids`: up to 50 project event ids the summary rests on. Every id must
  resolve to a non-audit event in the same project when the anchor is written.
- `owner`: the server-set event actor, never a client field.

The anchor is session-scoped and does not wait on the human. The tool also
writes a rendered `/fs/RECOVERY.md` into the session brain, best effort. The
event is the anchor of record; the file is a view of it, and a failed mirror
does not fail the anchor.

**Resume is anchor-scoped.** `GET /api/v1/sessions/:id/resume` and MCP
`session_resume` return the latest anchor, the session events after it oldest
first, the scores that cite it, `recovery_path`, and a `next_since` cursor.
`session_start` returns the latest anchor so a resumed agent can orient before
it reads the log. A session with no anchor resumes exactly as today: its
handoff note and its full event log. The full log stays one cursor page away.

**A score is a derived fact.** Add the event kind `score`, written only by the
human through `POST /api/v1/sessions/:id/scores`. Its payload carries a `slice`
of one to twenty anchor ids in the same session, a `score` from 1 to 5, and a
`rationale` of 1 to 2000 characters. The event actor is the human. A score
cites the slice it grades; it never edits the run or the anchor. Agents read
scores and cannot write them, so the human review step is the only way one
lands.

**The surfaces.** The session detail gains an anchor card and a resume view. The
card names the phase, summary, next steps, sources and the latest score; the
resume view lists the events after the anchor. A human action, "Score this
run", opens one dialog with a 1 to 5 control and a rationale. The project feed
draws `anchor` and `score` rows with their own glyphs and labels.

**What stays out.** No new store and no schema migration: both kinds ride the
events table. No bub runtime, no task graph, no shared-tape arbitration, no
fork-merge overlay, no hash chain, no sidecar store. An anchor is a validated
convention, not a workflow engine. A score is a human judgement, not an
automated metric.

## Consequences
- A resumed agent reads one compact anchor and only the work after it, then
  pages back to a cited event when a claim matters. Source ids are validated on
  write, so an anchor cannot cite an event that never existed.
- The anchor is the record and `RECOVERY.md` is its view. A session that never
  anchors behaves exactly as before.
- A source that is later pruned leaves a dangling citation. The resume view
  reports it as unresolved; it never repairs or hides it.
- Anchors and scores are ordinary events: they page, search, subscribe and
  prune with the session. They add two labels to the metrics and two rows to
  the feed.
- The load-bearing claim, that anchors measurably improve resume quality, is
  not proved by this record. The assessment leaves it unverified, and the hub
  should compare resumes with and without anchors before treating the
  convention as settled.
