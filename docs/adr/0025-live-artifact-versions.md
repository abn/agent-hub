---
type: Decision
title: A live artifact version is a pointer, not a draft
description: Why a version can be held live and mutated in place while an agent writes it, and why the artifact's public URL keeps serving the last sealed version throughout.
tags: [adr, artifacts, versioning, mcp]
status: stable
---

# 0025. A live artifact version is a pointer, not a draft

## Context

An artifact is its version history. Every write mints a version, and a version
is immutable once written. That is the property the rest of the hub leans on: a
public URL serves a snapshot, search indexes only settled text, and a comment
anchors to the version it was written against.

It also means an agent has no way to show the human a document while it is
being written. The agent either holds the whole thing in its own context until
it is finished, or publishes a run of near-identical versions, one per save,
which floods the feed, the version picker and search with work in progress.

The hub could hold drafts in memory, or mint a version per write, and neither
is right. An in-memory draft is a second kind of thing with its own lifetime
and its own orphan handling, and it is lost on restart, which is the moment a
half-finished document is most worth keeping. A version per write does not
scale: a ten minute edit coalesced at 400ms is roughly 1500 versions, while the
picker shows the last 50.

The decision recorded in [Artifacts](../usage/artifacts.md), the served agent
reference and the skill was that there is no live editing. This record replaces
that stance, and it changes what a public artifact URL means while a version is
live.

## Decision

An artifact is always its versions. There is no draft. A live edit is a pointer
on the artifact to the one version currently being written. Three columns on
`artifacts` carry it: `live_version` (nullable), `live_rev` and `live_session`.

Entering live mode writes version `current_ver + 1` and points `live_version`
at it. That fork preserves the state the human last read as an ordinary sealed
version, so the human can always diff against what they last saw. Every later
write mutates that one version in place, at its own blob path, bumping
`live_rev` and nothing else. One version is minted per live session, not one per
write.

Sealing moves `current_ver` onto the live version and clears the pointer. From
then on the version is immutable like any other. `artifact_update` seals as it
publishes, so publishing is the one call every agent already knows how to make,
and there is no seal tool to learn.

`current_ver` does not move while a version is live: it still points at the last
sealed version. A public artifact URL therefore always serves sealed content,
even mid-edit. The live version is reached only by asking for it, at
`?version=N` when the number is known or `?live=1` when it is not. A live
version is a real `artifact_versions` row with a real blob path, so every
existing read path works on it unchanged.

The MCP tool `artifact_draft(artifact_id, content, envelope?)` holds or updates
the live version. It returns a `viewer_url` with no credential in it, so an
agent can open the page in its own browser and refresh it as it writes. It is
not a share token: a share is admin-only and pins a version when the point here
is a version whose bytes move. It is not the viewer pass either: that pass is
admin-derived and reads a page the human has deliberately shared, and an agent
has no business reading that page. Sharing outward stays the human's act.

Three states are shown to the human: live (a pointer is set and the owning
session has been touched within the activity window), idle (a pointer is set
and the session has gone quiet past that window), and sealed (no pointer). The
band names the agent from `live_session`, so the human reads who is writing
rather than an unattributed dot. The admin route
`GET /api/v1/artifacts/{id}/live` returns `{state, version, live_rev, actor,
session_id}`; the content route cannot express liveness, since it returns
content whether or not a write just landed.

A live version is not indexed for search. The index runs on seal only, so the
corpus never holds half-written text and a live session costs one index write
rather than one per keystroke. Clearing a stale pointer is the existing
sweeper's job, and liveness is derived from the session window on every read,
so a pointer the sweeper has not reached yet never shows as live.

## Consequences

- A version is immutable once it stops being live. An agent may hold one version
  live while it works, and publishing seals it. A public URL keeps showing the
  last sealed version throughout.
- The public URL never serves half-finished work, so a link quoted in a bug
  report last week still renders what it did. The live view is reached only by
  asking for it, and an artifact id is a ULID, so both forms are unguessable in
  practice.
- The timeout is the existing activity window (`HUB_ACTIVE_WINDOW_SECS`, 900
  seconds by default), which already answers whether an agent is at work for
  `agents_active`. No new timer and no new knob.
- An abandoned live version leaves a pointer and a version nobody published.
  Because the default URL reads `current_ver`, that stale row never becomes
  what the artifact shows. The worst case is a band that reads idle and a
  version in the picker marked as never published; the next `artifact_update`
  supersedes it and deleting the artifact removes it.
- The version picker lists the live version alongside sealed ones, labelled as
  being written, so the picker can show work in progress where before it could
  not.
