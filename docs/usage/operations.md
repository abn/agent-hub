---
type: Runbook
title: Operations
description: Back up, restore, verify, upgrade, and roll back a hub safely.
tags: [usage, operations, backup, restore, upgrade, readiness]
status: draft
---

# Operations

A hub keeps everything in one data directory: the hub store (`hub.db` and its
write-ahead log), a session brain file per session, a knowledge base file per
project, and the artifact blobs. Back up, verify, and upgrade it as one unit.

## Back up

The backup is offline on purpose: a running hub holds the engine's exclusive
file lock, so a copy taken underneath it is not consistent.

```sh
agent-hub backup --out /path/to/backup
```

It refuses while a hub holds the store, telling you to stop the hub or snapshot
the volume. With the store free it copies every engine file through the engine
itself (`VACUUM INTO`) and every blob verbatim, and writes a `manifest.json`
holding the schema version, a timestamp, and each file's size and SHA-256. A
backup is the whole set of files the manifest names. When the embedded tailnet
is in use its key state, `tailnet/keys.json`, is part of the set, so a restore
keeps the node's tailnet identity instead of re-registering.

On a NAS or a virtual machine the alternative is a filesystem snapshot of the
data directory, which is consistent without stopping the hub. Either way the
`hub.db-wal` is part of the data: a copy of `hub.db` alone silently omits recent
writes.

## Verify

```sh
agent-hub check --data-dir /path/to/data
```

It verifies every file the manifest names (present, right size, right digest),
runs `PRAGMA integrity_check` on the store and every brain and knowledge file,
and cross-checks artifact rows against the blobs on disk. It exits non-zero on
any problem. A store older than this binary's artifact history is checked
without the cross-check, because there are no version rows to read.

A partially corrupt store is found here, not by `/readyz`. The readiness probe
runs the cheap legs only (the schema version, a content read, the store
identity, free space); a zeroed page in a table the probe does not touch leaves
it answering `200` while the routes that read that page fail with `503`. The
offline `check`, and `doctor`, walk the store with `PRAGMA integrity_check` and
name the damage, so run them on a schedule or after an unclean shutdown.

## Restore

```sh
agent-hub restore --from /path/to/backup --data-dir /path/to/data
```

It verifies every checksum before touching anything, refuses while a hub holds
the store, and refuses a non-empty data directory unless you pass `--force`. It
stages beside the destination and swaps by rename.

The swap is two renames: the live directory is moved to
`.agent-hub-replaced-<token>` beside it, then the staged tree is moved into
place. A crash between them leaves no data directory at all while the real data
sits in the sibling, so the next start would create a fresh empty one. If a
restore is interrupted that way, move the sibling back before starting the hub:
`mv /path/to/.agent-hub-replaced-<token> /path/to/data`.

## Upgrade and roll back

Migrations run on startup, forward only, each in one transaction that records
its version. Before applying any migration the hub copies the store to
`backups/pre-migration-v<from>-<timestamp>.db` and keeps the three newest, so an
upgrade that goes wrong has a recent store to restore.

A binary refuses to open a store whose schema is newer than the most it
supports, and says which versions those are, so an older binary will not quietly
run against a newer store. To roll back: stop the hub, replace `hub.db` with the
pre-migration `.db` from `backups/`, and start the older binary. This is a manual
file copy, not `agent-hub restore`: a pre-migration backup is a bare `.db` with
no `manifest.json`, and `restore` refuses a set it cannot verify. Copy the
sidecars with it, because `hub.db` alone omits whatever is still in the
write-ahead log:

```sh
docker compose -f deploy/compose.yaml down          # or: systemctl stop the unit
cp backups/pre-migration-v<from>-<stamp>.db data/hub.db
cp backups/pre-migration-v<from>-<stamp>.db-wal data/hub.db-wal   # only if present
cp backups/pre-migration-v<from>-<stamp>.db-shm data/hub.db-shm   # only if present
# start the older binary against data/
```

Remove a `hub.db-wal` or `hub.db-shm` the backup did not carry, so the store is
not opened over a stale log. Read the
[data model](../architecture/data-model.md) before rolling back across a release
that changed the schema.

## The feed ceiling and shutdown

A project's feed is bounded by `events_per_project` (`HUB_EVENTS_PER_PROJECT`,
default one million). A write past it is refused with the cap named, so a
runaway agent cannot fill the node; promote durable work to the knowledge base
or prune the feed to make room. The ceiling bounds every agent-surface writer:
signals, questions, answers, artifact publish and update, and the knowledge
base's lifecycle signal. Session lifecycle records and the hub's audit trail
are exempt, so a full feed can never refuse `session_start`; a knowledge base
write itself still succeeds, because its feed signal is best-effort and is
dropped when the feed is full.

On `SIGTERM` or `SIGINT` the hub stops accepting, lets in-flight requests
finish for a bounded window, then checkpoints the store and exits 0. `docker
stop` and a service restart therefore drain rather than cut.

`agent-hub health [--url URL]` asks `/readyz` and exits 0 when ready, non-zero
otherwise. It is what the container healthcheck runs, because the runtime image
is distroless and has no shell or curl.

## Readiness and disk space

`GET /readyz` answers `200` only when the store is usable: the schema version is
read, is supported, and matches what the process opened; the data directory and
`hub.db` still have the identity they had at open, so a removed or replaced store
is caught; a content read confirms it is still a hub store; and the volume has
free space above a safety margin. Any leg failing is a `503`, so a supervisor can
take the node out of rotation.

`GET /healthz` is liveness only: the process is up.

## Practical limits

The node is sized for one operator and their agents, not a fleet of tenants,
so a handful of things grow with use rather than being capped. Knowing where
they are is how you decide when to prune.

- **Sessions.** No count cap. A session lives until the human prunes it. One
  session brain file warns past 256 MiB and is refused at 1 GiB, and a listing
  returns at most 200 sessions.
- **Events.** A project's feed holds `events_per_project`
  (`HUB_EVENTS_PER_PROJECT`, default one million). A write past it is refused
  and names the cap; promote durable work to the knowledge base or prune the
  feed to make room. The ceiling bounds signals, questions, answers, artifact
  publish and update, and the knowledge base's lifecycle signal. Session
  lifecycle records and the hub's audit trail are exempt, so a full feed never
  refuses `session_start`; a knowledge base write still succeeds, its feed
  signal dropping when the feed is full.
- **Knowledge base.** One page is capped at 1 MiB and one path at 512 bytes. A
  project's knowledge base file warns past 256 MiB and is refused at 1 GiB, so
  its page count follows that file's size rather than a count of its own.
  History and last-write scan the write log newest first, which is the brain
  engine's own call table; the hub wraps it and does not index it, so those
  reads cost more as the log grows. A history request carries at most 200 rows
  and reports the real total.
- **Artifacts.** One blob is capped at 50 MiB. A blob now moves on the Tokio
  blocking pool, so a large transfer no longer holds an async worker and the
  hub keeps answering other requests while it lands. Many large transfers at
  once are felt in memory (roughly 50 MiB each in flight) and disk bandwidth
  rather than in the workers; past the blocking pool's ceiling they queue.
- **Search.** A filtered search reads at most 5000 rows before it stops, and
  returns at most 100 hits a page. A scope that matches more than the fetch cap
  is reported as truncated rather than scanned whole.

## Metrics

`GET /metrics` reports the hub's own counters in Prometheus text: HTTP requests
by method and status class, failed responses by hub error code, events appended
by kind, and MCP tool calls by tool. It also carries storage gauges read fresh
for each scrape: free bytes on the data volume, the `hub.db` size, and the
write-ahead log size, so a time-series system can alert before the readiness
margin trips rather than only when the node is nearly out of room. The route is
admin-gated like the rest of the control surface, so the scraper carries the
admin token in `Authorization: Bearer`. There is no separate scrape credential.

## Doctor

```sh
agent-hub doctor [--data-dir DIR]
```

One read-only report: the data directory and its identity, the schema version
and the most this binary supports, free space, the write-ahead log size, the id
high-water mark, and any artifact blob the store names but the tree is missing.
It refuses while a hub holds the store, like the other offline commands, and
exits non-zero on any problem.

## Configuration

`agent-hub config` reports the active settings and where each came from, and
`agent-hub config --check` validates them. An unknown key under `[hub]` or
`[client]` is an error: the check exits non-zero and names the key, rather than
ignoring a setting you thought took effect.

Enrolment is on by default. Set `enrol = off` under `[hub]` (or `HUB_ENROL=off`)
to close the unauthenticated enrolment endpoint, and set `trust_proxy` to the
proxy addresses whose forwarded client header the hub may trust.
