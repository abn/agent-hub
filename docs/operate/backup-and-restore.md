---
type: Runbook
title: Back up and restore
description: Back up, verify, and restore the hub store as one unit
tags: [operator, operations]
status: stable
---

# Back up and restore

By the end you will have a verified backup of the whole data directory and a
tested way to put it back. The store's shape is in
[Data model](/architecture/data-model.md); this page is the procedure.

## Before you start

| Thing | Why |
|---|---|
| The data directory | `hub.db` and its write-ahead log, one brain file per session, one knowledge file per project, and the artifact blobs all live in it |
| A destination outside that directory | a backup written into the tree it copies is refused |
| The `agent-hub` binary on the host | the container image is built without the client, so run the binary or `curl` from the host against the published port |

Back up the whole directory as one unit. A copy of `hub.db` alone silently
omits recent writes, because `hub.db-wal` is part of the data.

## What a backup holds

Both backup kinds write the same set:

- every engine file, copied through the engine itself (`VACUUM INTO`);
- every artifact blob, verbatim;
- a `manifest.json` holding the schema version, a timestamp, and each file's
  size and SHA-256.

A backup is the whole set of files the manifest names. When the embedded
tailnet is in use, its key state, `tailnet/keys.json`, is part of the set, so a
restore keeps the node's tailnet identity instead of re-registering. A project's
files that a delete moved aside and could not remove are left out: they are no
longer part of the store, and the next start removes them. `check` and
`restore` read either kind the same way.

## Offline backup

```sh
agent-hub backup --out /path/to/backup
```

A running hub holds the engine's exclusive file lock, so the offline backup
refuses while one serves the store, and says to ask the hub with `--url`, stop
it, or snapshot the volume. With the store free it copies everything while it
holds the lock itself, so nothing changes under the copy.

## Online backup

The hub is the single writer for every file it serves, so it can take the
backup itself without stopping. Online backup is off until `backup_dir`
(`HUB_BACKUP_DIR`) names a directory for it, outside the data directory:

```sh
HUB_BACKUP_DIR=/srv/agent-hub-backups
```

The hub refuses to start with a `backup_dir` inside the data directory, and
checks again with symlinks resolved before each backup. The directory must
already exist: an unmounted backup volume is reported, not filled in on the
root filesystem. Then ask the running hub for a backup:

```sh
HUB_ADMIN_TOKEN=<the admin token> agent-hub backup --url http://127.0.0.1:8080
```

The request is the admin's, so the token is `HUB_ADMIN_TOKEN`, or `admin_token`
under `[hub]` in `config.toml`, the one the hub serves with. The hub writes a
new directory named for the UTC time to the millisecond, such as
`20261007T031500.123Z`, under `backup_dir`, and nowhere else: the request
carries no path. The command prints where the backup landed, and exits `0` on
success, `1` when the hub refused (online backup off, one already running),
`69` when the hub is unreachable, and `77` when the token was refused.
`HUB_TIMEOUT` bounds the wait; a backup the client stops waiting for still runs
to the end on the hub.

The same request without the binary, for a host that only has `curl`:

```sh
curl -fsS -X POST -H "Authorization: Bearer $HUB_ADMIN_TOKEN" \
  http://127.0.0.1:8080/api/v1/backups
```

It answers `201` with the directory's name, its path on the node, the
manifest's schema version, timestamp, file count and bytes, and how long the
backup took. Online backup off is a `404` saying how to turn it on, a backup
already running is a `409`, and a missing backup directory is a `503`.

### What an online backup guarantees

- **Every engine file is a consistent snapshot of itself.** `hub.db` is copied
  through the hub's own handle, inside one read transaction, so writers carry on
  and the copy holds exactly what was committed when it began. Each session
  brain and knowledge file is copied the same way while its per-file write lock
  is held, so no write to that file lands mid-copy.
- **The store never names a blob the backup lacks.** `hub.db` is copied first,
  and a blob is always written before the row that names it commits, so every
  blob the snapshot names is on disk by then. Deleting or overwriting those
  files is held off for the whole backup (an artifact or project delete, a
  prune's commit, a live artifact write and the update that seals it, which wait
  and then finish), so none of those blobs can go or change before it is copied,
  and each one holds the bytes its row describes. The hub reads the copied store
  back and refuses to finish a backup that names a blob it lacks.
- **The set is not one instant across files.** The brain, knowledge, and blob
  files are copied after `hub.db`, so they can hold writes the store snapshot
  does not: a page written to a brain moments after the snapshot, a blob whose
  row committed later, a session brain with no session row yet. The first start
  on a restored directory removes blobs and brain files no row names, as it does
  after a crash.
- **One backup at a time,** and nothing is deleted or written live while one
  runs.

### Schedule it

There is no scheduler inside the hub. Run the request from cron or a systemd
timer on the node, for example nightly at 03:15:

```sh
# crontab -e
15 3 * * * HUB_ADMIN_TOKEN=<the admin token> /usr/local/bin/agent-hub backup --url http://127.0.0.1:8080
```

or as a timer and a oneshot service:

```ini
# /etc/systemd/system/agent-hub-backup.service
[Service]
Type=oneshot
EnvironmentFile=/etc/agent-hub/backup.env
ExecStart=/usr/local/bin/agent-hub backup --url http://127.0.0.1:8080

# /etc/systemd/system/agent-hub-backup.timer
[Timer]
OnCalendar=*-*-* 03:15:00
Persistent=true

[Install]
WantedBy=timers.target
```

with `HUB_ADMIN_TOKEN=...` in `/etc/agent-hub/backup.env`, readable by the
service's account only. Old backups are never deleted by the hub. You prune
them, as you do sessions: remove the oldest directories under `backup_dir` when
the volume needs the room.

## Snapshots

On a NAS or a virtual machine the other alternative is a filesystem snapshot of
the data directory, which is consistent without stopping the hub. Either way,
remember that `hub.db-wal` is part of the data.

## Verify

```sh
agent-hub check --data-dir /path/to/data
```

It verifies every file the manifest names (present, right size, right digest),
runs `PRAGMA integrity_check` on the store and every brain and knowledge file,
and cross-checks artifact rows against the blobs on disk. It exits non-zero on
any problem. A store older than this binary's artifact history is checked
without the cross-check, because there are no version rows to read.

Run this against a backup before you rely on it, and run it on the live data
directory on a schedule. A partially corrupt store is found here, not by
`/readyz`: the readiness probe runs the cheap legs only, and a zeroed page in a
table the probe does not touch leaves it answering `200` while the routes that
read that page fail with `503`.

## Restore

```sh
agent-hub restore --from /path/to/backup --data-dir /path/to/data
```

An offline and an online backup restore the same way. The command verifies
every checksum before touching anything, refuses while a hub holds the store,
and refuses a non-empty data directory unless you pass `--force`. It stages
beside the destination and swaps by rename.

> [!WARNING]
> The swap is two renames: the live directory is moved to
> `.agent-hub-replaced-<token>` beside it, then the staged tree is moved into
> place. A crash between them leaves no data directory at all while the real
> data sits in the sibling, so the next start would create a fresh empty one. If
> a restore is interrupted that way, move the sibling back before starting the
> hub: `mv /path/to/.agent-hub-replaced-<token> /path/to/data`.

## You should now see

`agent-hub check` prints `checked N engine files, no problems` and exits 0. On a
restore, the command prints `restored N files, M bytes, to <data directory>`.

## See also

- [Upgrade and roll back](/operate/upgrade-and-rollback.md) - the pre-migration store a rollback uses
- [Monitoring](/operate/monitoring.md) - offline `check`, `doctor`, and the integrity sample
- [Configuration](/reference/configuration.md) - `backup_dir`
