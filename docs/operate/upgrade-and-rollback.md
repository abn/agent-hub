---
type: Runbook
title: Upgrade and roll back
description: Migrate forward on upgrade, and roll back to an earlier store
tags: [operator, operations]
status: stable
---

# Upgrade and roll back

By the end you will have moved the hub to a new version with a recent store to
fall back on, and you will know the manual steps if the upgrade goes wrong. The
store's shape is in [Data model](/architecture/data-model.md).

## Before you start

| Thing | Why |
|---|---|
| A verified backup | the offline `check` proves it before you rely on it |
| The data directory | migrations rewrite `hub.db` in place |
| The `backups/` directory inside it | the hub keeps its own pre-migration copies there |

## Upgrade

Migrations run on startup, forward only, each in one transaction that records
its version. Before applying any migration the hub copies the store to
`backups/pre-migration-v<from>-<timestamp>.db` and keeps the three newest, so an
upgrade that goes wrong has a recent store to restore.

A binary refuses to open a store whose schema is newer than the most it
supports, and says which versions those are, so an older binary will not quietly
run against a newer store.

To upgrade:

1. Take a backup first,
   ([Back up and restore](/operate/backup-and-restore.md) has the command).
2. Pull the new image tag, or unpack the new release binary.
3. Restart the service. Under compose or Docker, replace the image tag and
   re-run; under Quadlet or a binary, stop the unit, swap the image or binary,
   and start it.

The store migrates forward on startup. There is no separate migrate command and
nothing to run by hand.

## You should now see

The hub starts, prints its bound address, and answers its readiness probe:

```sh
agent-hub health --url http://127.0.0.1:8080
```

It prints `ready` and exits 0, and the Settings Version row names the new
version.

## Roll back

Rolling back is a manual file copy of the store, not `agent-hub restore`: a
pre-migration backup is a bare `.db` with no `manifest.json`, and `restore`
refuses a set it cannot verify.

1. Stop the hub.
2. Copy the pre-migration store over the live one. Copy its sidecars with it,
   because `hub.db` alone omits whatever is still in the write-ahead log.
3. Remove a `hub.db-wal` or `hub.db-shm` the backup did not carry, so the store
   is not opened over a stale log.
4. Start the older binary against the data directory.

```sh
docker compose -f deploy/compose.yaml down          # or: systemctl stop the unit
cp backups/pre-migration-v<from>-<stamp>.db data/hub.db
cp backups/pre-migration-v<from>-<stamp>.db-wal data/hub.db-wal   # only if present
cp backups/pre-migration-v<from>-<stamp>.db-shm data/hub.db-shm   # only if present
# start the older binary against data/
```

> [!IMPORTANT]
> Read the [data model](/architecture/data-model.md) before rolling back across
> a release that changed the schema. A store written by a newer schema is not
> readable by an older binary, which is exactly what the schema guard catches.

## See also

- [Back up and restore](/operate/backup-and-restore.md) - take the store the rollback returns to
- [Deploy as a service](/operate/deploy-as-a-service.md) - the service that restarts
- [Monitoring](/operate/monitoring.md) - readiness and the offline `doctor`
