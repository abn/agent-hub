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
2. Pull the new image tag, or unpack the new release binary. A native install
   upgrades by running the installer again (below).
3. Restart the service. Under compose or Docker, replace the image tag and
   re-run; under Quadlet or a binary, stop the unit, swap the image or binary,
   and start it.

The store migrates forward on startup. There is no separate migrate command and
nothing to run by hand.

### A native install

Re-run the install command:

```sh
curl -fsSL https://hub.agync.dev/install.sh | sh
```

On Windows run `irm https://hub.agync.dev/install.ps1 | iex`. The script keeps
the current binary as `agent-hub.prev` (`agent-hub.prev.exe` on Windows) in the
state directory (for example `~/.local/state/agent-hub/` on Linux, which
`XDG_STATE_HOME` moves, though not the binary), installs the new one, re-renders the
service definition, and restarts the hub if it was running. It leaves the
config and the data alone, other than adding keys that are new. `--version
vX.Y.Z` pins a release, and an install of the version already present says so
and changes nothing. [Run the hub as a native service](/operate/native-service.md)
has the paths for each platform.

### The upgrade to members, agents and sessions

One release changes the identity model: what held a token is now a member, the
program is an agent, and a session is shared by every participant that joins it
([ADR 0039](/adr/0039-members-agents-and-sessions.md)). The upgrade moves the
data for you, but it reads the old owners first, and it can stop to ask you
before it changes anything.

**Preflight.** Before the first write, the hub reads every owner label in the
store (session owners, feed cursors, notification cursors, subscriptions) and
places each as a member, or as a member and an agent when the label was
`<member>/<name>`. Run it first, on a copy or a stopped hub, to see the plan:

```sh
agent-hub migrate --check --data-dir /path/to/data
```

It prints where each owner would go, which sessions would merge, and anything that
would stop the upgrade. It writes nothing, exits 0 when the plan is clean and 78
when it would stop, and 75 when a serving hub holds the store.

**A stop.** An owner the hub cannot place, such as `claude` from a stdio setup, a
name that is not a registered member, stops the start with exit 78 and the message
`the store holds owners this release cannot place`, naming each owner. Nothing was
changed. Write the mapping in `identity-map.toml` in the data directory, then
start again:

```toml
[owners]
"claude"  = { member = "laptop", agent = "claude-code" }
"old-bot" = "human"        # or "local", or "drop"
```

`drop` clears a cursor or a subscription, but never a session owner: map those to
a member.

**What changes.** Two sessions in one project with the same name, which used to
belong to `laptop/claude` and `laptop/opencode`, become one shared session with two
participants. The newest active one survives and its brain takes the others'
entries; an entry that would collide moves under `merged/<id>/` in the survivor's
brain, and the session detail says it was merged on upgrade. The other brains are
archived under `merged/<project>/` in the data directory, and follow the survivor
through prune, undo and backup. Old session ids still resolve. The hub does not
serve until every merge is finished, and a crash resumes from where it stopped.
Rows written before the upgrade keep their old author label and render it as is.
The old `/api/v1/agents` routes are gone, replaced by `/api/v1/members`, so update
any script that calls them.

**The backup.** Before it changes anything the hub takes a full backup set into
`backups/pre-migration-v<from>-<stamp>/` (the store, every brain, the knowledge
bases and the artifact blobs), and keeps the three newest sets.

**Roll back.** Stop the hub, restore that whole set over the data directory
(the store with its write-ahead log removed, every brain and the blobs), delete
`merged/`, and start the previous release. **Every write made after the upgrade is
lost.** The set is a full backup so that nothing deleted after the upgrade is
revived without its blobs.

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
4. Start the older binary against the data directory. On a native install the
   previous binary is `agent-hub.prev` (`agent-hub.prev.exe` on Windows) in the
   state directory: copy it over `agent-hub` (`agent-hub.exe`), then
   `agent-hub start`.

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
