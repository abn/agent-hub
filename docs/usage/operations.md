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
backup is the whole set of files the manifest names.

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
any problem.

## Restore

```sh
agent-hub restore --from /path/to/backup --data-dir /path/to/data
```

It verifies every checksum before touching anything, refuses while a hub holds
the store, and refuses a non-empty data directory unless you pass `--force`. It
stages beside the destination and swaps by rename.

## Upgrade and roll back

Migrations run on startup, forward only, each in one transaction that records
its version. Before applying any migration the hub copies the store to
`backups/pre-migration-v<from>-<timestamp>.db` and keeps the three newest, so an
upgrade that goes wrong has a recent store to restore.

A binary refuses to open a store whose schema is newer than the most it
supports, and says which versions those are, so an older binary will not quietly
run against a newer store. To roll back: stop the hub, restore the pre-migration
backup from `backups/`, and start the older binary. Read the
[data model](../architecture/data-model.md) before rolling back across a release
that changed the schema.

## Readiness and disk space

`GET /readyz` answers `200` only when the store is usable: the schema version is
read, is supported, and matches what the process opened; the data directory and
`hub.db` still have the identity they had at open, so a removed or replaced store
is caught; a content read confirms it is still a hub store; and the volume has
free space above a safety margin. Any leg failing is a `503`, so a supervisor can
take the node out of rotation.

`GET /healthz` is liveness only: the process is up.

## Configuration

`agent-hub config` reports the active settings and where each came from, and
`agent-hub config --check` validates them. An unknown key under `[hub]` or
`[client]` is an error: the check exits non-zero and names the key, rather than
ignoring a setting you thought took effect.

Enrolment is on by default. Set `enrol = off` under `[hub]` (or `HUB_ENROL=off`)
to close the unauthenticated enrolment endpoint, and set `trust_proxy` to the
proxy addresses whose forwarded client header the hub may trust.
