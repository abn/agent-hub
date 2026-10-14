---
type: Guide
title: Storage and pruning
description: See what is using space and prune sessions reversibly
tags: [user, operations]
status: stable
---

# Storage and pruning

Storage shows what the hub is using and where, and prune reclaims the one thing
that can be reclaimed reversibly: ended session brains. Nothing expires on its
own; you are the garbage collector.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../../assets/screens/storage-desktop-dark.png">
  <img src="../../assets/screens/storage-desktop-light.png" alt="Storage: the stacked bars and the prune review.">
</picture>

## What storage shows

Storage names the data path and node, then shows used against the volume's own
capacity and a stacked bar by kind: events, sessions, artifacts, and knowledge.
The legend carries each kind's byte figure, so the bar is never the only place a
number lives. A row per project shows its total, its own bar split four ways to
scale, and the same split in words.

The events figure is the hub store, one file every project shares, so the rows'
event bytes add up to less than that figure. Storage says how much of it is the
shared hub database and how much can honestly be charged to one project.

> [!NOTE]
> When under 1% of the volume is used, the whole bar would be a few pixels, so
> it is drawn against what is used instead and says so beside it.

## Pruning

Prune acts on ended sessions only. It never touches a feed event, an artifact, an
active session, or a project knowledge base.

- A single session is pruned from its session detail
  ([Sessions and brains](/guides/human/sessions-and-brains.md)).
- A project's **Prune** button removes every ended session in that project, with
  the bytes it would free.
- **Prune all** reviews the affected projects, session counts, and bytes in a
  dialog first.

Every prune is confirmed with the safe action focused first, then undoable from
the toast for 30 seconds, then committed. A project a prune has just emptied
keeps its row; projects whose four figures are all zero and have nothing to
prune are folded under a count, so a run of empty rows does not crowd the list.

## What grows, and its ceiling

The node is sized for one operator and their agents, not a fleet of tenants, so
a few things grow with use rather than being capped. Two that decide when you
prune:

- a project's feed is bounded by `events_per_project`, and a write past the cap
  is refused until events are removed, which pruning a session does. The
  `session_start` write and the hub's audit trail are exempt from the refusal,
  so a full feed never refuses a session.
- a session brain file and a project knowledge base file warn past 256 MiB and
  are refused at 1 GiB.

Every ceiling and limit is in [Configuration](/reference/configuration.md) and
[Knowledge base pages](/reference/knowledge-base-pages.md).

## On a desktop

From 720px the screen shifts to a full-width table over four summary tiles, ON
DISK, ARTIFACT BLOBS, SESSION BRAINS, and RECLAIMABLE, with columns for Project,
Share, Total, Blobs, Brains, Reclaimable, Last Write, and Prune. A project with
no ended sessions shows a dash in Reclaimable rather than 0 B, and only rows
with reclaimable bytes carry a Prune button. Free space on the host volume is
not drawn; the used-against-capacity line above the table carries it.

## You should now see

What each project and each kind is using, what a prune would free, and a Prune
control wherever there is anything to reclaim.

## See also

- [Sessions and brains](/guides/human/sessions-and-brains.md) - prune one session and read its brain
- [Configuration](/reference/configuration.md) - the feed ceiling and every limit
- [Back up and restore](/operate/backup-and-restore.md) - the whole data directory as one unit
- [REST API](/reference/rest-api.md) - the storage and prune routes
