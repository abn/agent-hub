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

There are two ways to take a backup, and both write the same set: every engine
file copied through the engine itself (`VACUUM INTO`), every artifact blob
verbatim, and a `manifest.json` holding the schema version, a timestamp, and
each file's size and SHA-256. A backup is the whole set of files the manifest
names. When the embedded tailnet is in use its key state, `tailnet/keys.json`,
is part of the set, so a restore keeps the node's tailnet identity instead of
re-registering. A project's files that a delete moved aside and could not
remove are left out: they are no longer part of the store, and the next start
removes them. `check` and `restore` read either kind the same way.

### Offline

```sh
agent-hub backup --out /path/to/backup
```

A running hub holds the engine's exclusive file lock, so the offline backup
refuses while one serves the store, and says to ask the hub with `--url`, stop
it, or snapshot the volume. With the store free it copies everything while it
holds the lock itself, so nothing changes under the copy.

### Online, taken by the serving hub

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

The request is the admin's, so the token is `HUB_ADMIN_TOKEN`, or
`admin_token` under `[hub]` in `config.toml`, the one the hub serves with. The
hub writes a new directory named for the UTC time to the millisecond, such as
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

What an online backup guarantees:

- **Every engine file is a consistent snapshot of itself.** `hub.db` is copied
  through the hub's own handle, inside one read transaction, so writers carry
  on and the copy holds exactly what was committed when it began. Each session
  brain and knowledge file is copied the same way while its per-file write lock
  is held, so no write to that file lands mid-copy.
- **The store never names a blob the backup lacks.** `hub.db` is copied first,
  and a blob is always written before the row that names it commits, so every
  blob the snapshot names is on disk by then. Deleting or overwriting those
  files is held off for the whole backup (an artifact or project delete, a
  prune's commit, a live artifact write and the update that seals it, which wait
  and then finish), so none of those blobs can go or change before it is
  copied, and each one holds the bytes its row describes. The hub reads the
  copied store back and refuses to finish a backup that names a blob it lacks.
- **The set is not one instant across files.** The brain, knowledge and blob
  files are copied after `hub.db`, so they can hold writes the store snapshot
  does not: a page written to a brain moments after the snapshot, a blob whose
  row committed later, a session brain with no session row yet. The first start
  on a restored directory removes blobs and brain files no row names, as it
  does after a crash.
- **One backup at a time,** and nothing is deleted or written live while one
  runs.

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
service's account only. The container image is built without the client, so on
a container host run the published binary or `curl` from the host against the
published port.

Old backups are never deleted by the hub. You prune them, as you do sessions:
remove the oldest directories under `backup_dir` when the volume needs the
room.

### Snapshots

On a NAS or a virtual machine the other alternative is a filesystem snapshot of
the data directory, which is consistent without stopping the hub. Either way
the `hub.db-wal` is part of the data: a copy of `hub.db` alone silently omits
recent writes.

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

An offline and an online backup restore the same way.

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
by kind, MCP tool calls by tool, and sends to the notify target by result
(`agenthub_notify_sends_total`, `delivered` or `failed`). It also carries storage gauges read fresh
for each scrape: free bytes on the data volume, the `hub.db` size, and the
write-ahead log size, so a time-series system can alert before the readiness
margin trips rather than only when the node is nearly out of room. The route is
admin-gated like the rest of the control surface, so the scraper carries the
admin token in `Authorization: Bearer`. There is no separate scrape credential.

Set `integrity_sample_secs` (`HUB_INTEGRITY_SAMPLE_SECS`, default `0`, disabled)
to a number of seconds to have the hub sample the store's own integrity in the
background, on the sweeper's cadence. Each sample runs `PRAGMA quick_check` on
its own connection, never inside a request, bounded by a wall-clock cap and
skipped while the previous one has not finished, so a large or damaged store
cannot pin the loop. It raises two series: `agenthub_integrity_ok`, a gauge that
is `1` for a passing sample and `0` for a failing one (absent until the first
sample runs), and `agenthub_integrity_failures_total`, a counter of samples that
timed out, errored, or reported a problem. A failed sample is what tells you a
zeroed page has appeared between offline `check` runs; the offline commands still
walk the store with the full `PRAGMA integrity_check`.

## Notify a closed app

An installed app that is fully closed raises nothing on its own. If you run a
notification service on your LAN or tailnet, the hub can nudge it instead: set
`notify_url` to a URL that accepts a plain-text POST, and when a question, an
approval or an enrolment request starts waiting, the hub POSTs this body to it:

```
Something is waiting for you in Agent Hub.
```

That sentence is the whole payload. It names no project, agent, title, id or
count, so the target learns only that it is worth opening the app. The setting
is off by default and nothing is sent without it
([ADR 0026](../adr/0026-contentless-notify-target.md)). A retried
write that the hub answers from its idempotency key posts nothing new, so it
sends nothing.

A self-hosted ntfy on the LAN, with an access token for a topic of its own:

```toml
[hub]
notify_url = "https://ntfy.lan/agent-hub?title=Agent+Hub&click=https://hub.lan"
notify_token = "tk_..."
notify_interval_secs = 60
```

ntfy reads its own message options from the query, so the title and the tap
target are set there, by you; the hub sends neither. Subscribe to the topic in
the ntfy app on the phone. A target that wants another shape, such as Gotify's
JSON message, needs a small bridge in front of it.

Sends are coalesced: at most one per `notify_interval_secs`, and anything that
arrives inside the quiet period is one trailing send when it ends. A send runs
in the background with a ten-second timeout and is not retried, so a slow or
unreachable target never holds up the agent's write. A failure is logged at
warn with the target's origin and the cause, and counted in
`agenthub_notify_sends_total{result="failed"}`. The token is sent only in the
`Authorization` header and is never logged or printed; `agent-hub config` shows
the URL by its origin alone.

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
