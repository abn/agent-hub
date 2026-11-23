---
type: Guide
title: Run the hub as a native service
description: Install, start, upgrade and remove the hub as a per-user service, with no container runtime
tags: [operator, install, operations]
status: stable
---

# Run the hub as a native service

By the end you will have the hub running as a service of your own user account,
with no container runtime and no `sudo`. It starts on its own, you manage it
with `agent-hub start`, `stop`, `status` and `logs`, and you can upgrade and
remove it with one command each. For the container instead, read
[Deploy as a service](/operate/deploy-as-a-service.md).

## Before you start

| Thing | Why |
|---|---|
| Linux with systemd, macOS 13 or later, or Windows 10 1809 or later | the three service managers the binary drives |
| A normal user account | the service belongs to it; the installer refuses to run as root |
| A free port, 8080 by default | the hub binds `127.0.0.1:8080` unless you say otherwise |

Windows binaries ship only in releases whose gated Windows build passed: the
build runs the real installer, the task, a stop and an uninstall on the exact
archive before it is attested and uploaded. A release without a Windows archive
makes the PowerShell installer exit with 3 and point at WSL. Linux and macOS
never wait on it.

## Install

On Linux and macOS:

```sh
curl -fsSL https://hub.agync.dev/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://hub.agync.dev/install.ps1 | iex
```

The installer downloads the release archive for the machine, checks its
`.sha256`, places the binary, and runs `agent-hub service install`. That
command writes the service definition and a config, starts the hub, and prints
a summary with the address and the admin token:

```text
Admin token (shown once): <the 64 hex characters>

Installed the agent-hub service.
  unit         ~/.config/systemd/user/agent-hub.service
  config       ~/.config/agent-hub/config.toml (created)
  data         ~/.local/share/agent-hub
  logs         agent-hub logs  (journalctl --user -u agent-hub)
  starts at    boot (linger on)
  address      http://127.0.0.1:8080/
  admin token  9c1f...e04a (64 characters, saved in config.toml)

started (pid 4121) at http://127.0.0.1:8080/

Open http://127.0.0.1:8080/ and paste the admin token to log in.
Manage it with: agent-hub status | stop | start | logs | uninstall
```

The full token is printed once, on the run that created it. A later run says
`admin token  kept (in config.toml)` and the heading reads `Updated the
agent-hub service.` or, when nothing changed, `The agent-hub service is already
installed and up to date.` A token that the shell already held in
`HUB_ADMIN_TOKEN` is kept and copied into `config.toml`, because the service
reads only that file. Open the address and paste the token:
[Open the app and log in](/start/first-login.md) has the rest.

`agent-hub service install` takes the flags below. It never changes a key
`config.toml` already holds, so a `--bind` or `--data-dir` that disagrees with
the file is refused with exit 2 and nothing changes: edit the file, then run it
again.

| Flag | Effect |
|---|---|
| `--bind ADDR` | the address to bind, written to `[hub] bind` on the first install |
| `--data-dir DIR` | the data directory, written to `[hub] data_dir` as an absolute path |
| `--boot`, `--login` | `--boot` (the Linux default) tries `loginctl enable-linger` and falls back to login when the system refuses; `--login` starts at login and neither reads nor changes linger, so linger that is already on stays on |
| `--no-start` | write and register the service without starting it |
| `--yes` | accepted so a script can pass it; install never prompts |
| `--force` | install over a container install's Quadlet file |

It exits 0 when done, 69 when no service manager is reachable (when the first
manager check fails nothing is written; a manager that stops answering during
registration, or when the service is started after config and registration
succeeded, can leave the config merge already done), 2 for a usage error or a refusal that needs another flag, 78 for an
invalid configuration, and 1 for anything else, including a hub that did not
come up after the install.

### Installer flags

Pass flags after `sh -s --`, for example
`curl -fsSL https://hub.agync.dev/install.sh | sh -s -- --bind 0.0.0.0:8080`.
The PowerShell script takes the same options as parameters: `-Version`,
`-BinDir`, `-NoService`, `-Bind`, `-Boot`, `-Login`, `-RequireProvenance`,
`-Force`, `-Uninstall`, `-Purge`, `-Yes` and `-AllowAdmin`. To pass them, run
`& ([scriptblock]::Create((irm https://hub.agync.dev/install.ps1))) -Bind 0.0.0.0:8080`.
The script refuses an elevated shell (exit 2) because the install is per-user;
`-AllowAdmin` overrides that.

| Flag | Variable | Effect |
|---|---|---|
| `--version vX.Y.Z` | `AGENT_HUB_VERSION` | install that release instead of the latest |
| `--bin-dir DIR` | `AGENT_HUB_BIN_DIR` | where the binary goes |
| `--no-service` | `AGENT_HUB_NO_SERVICE=1` | place the binary and stop; for a machine that only runs agents |
| `--bind ADDR` | | the address the hub binds, passed to `service install` |
| `--boot`, `--login` | | start at boot (on Linux only where linger is granted, else at login), or only at login; `--login` never turns off linger that is already on |
| `--require-provenance` | | fail unless the build attestation verifies |
| `--force` | | reinstall the version already there (without it, a re-run at the same version still runs `service install`) |
| `--uninstall [--purge]` | | run `agent-hub uninstall` |
| `--yes` | | answer every prompt |

The shell script exits 0 when it is done, 1 when the download, a check or the
service install failed, 2 for a usage error, and 3 on a platform it has no
build for, or for a release that predates the service commands (v1.12.0 and
earlier): install a newer release, or pass `--no-service` to place the binary
only. The installer also refuses a binary whose `--version` is not the release
it asked for. It also exits 0 with the binary alone when the host has no service
manager, and says so. With `--uninstall` it passes through the result of
`agent-hub uninstall`.

The PowerShell exit codes are the same only when it runs as a file
(`powershell -File .\install.ps1`). Through `irm | iex` or a script block a
failure is a thrown error that leaves your shell open, and the code is in
`$LASTEXITCODE`; `powershell -Command` then exits 1.

### Without a service manager

`--no-service`, a host with no manager (exit 69) and a manual download all leave
the binary alone. Nothing writes a config or an admin token in that case, so
create one before you log in. With no token a loopback hub starts but its app
refuses every request. If `~/.config/agent-hub/config.toml` does not exist yet:

```sh
mkdir -p ~/.config/agent-hub
(umask 077; printf '[hub]\nadmin_token = "%s"\n' "$(openssl rand -hex 32)" > ~/.config/agent-hub/config.toml)
agent-hub start
```

If the file exists, do not run that: the `>` would replace it. Add the line
`admin_token = "<64 hex characters>"` under its `[hub]` table by hand (make one
with `openssl rand -hex 32`), or skip the file and run
`HUB_ADMIN_TOKEN=<token> agent-hub start`. Paste that token to log in. On Windows the file is `%APPDATA%\agent-hub\config.toml`.
`HUB_ADMIN_TOKEN` in the shell that runs `agent-hub start` works as well.

### What the installer checks

- **Checksum.** The `.sha256` beside each archive is compared before anything
  is placed, and a host with no tool to compute one fails closed.
- **Provenance.** The release workflows attest every archive with GitHub build
  provenance. With the `gh` CLI present the installer runs `gh attestation
  verify` against the downloaded archive and warns when it fails;
  `--require-provenance` (`-RequireProvenance` on Windows) makes a failure, or a
  missing `gh`, fatal. Without `gh` it says `provenance: not checked`. A checksum served beside the archive proves
  integrity, not who built it, so use `--require-provenance` where that matters.
- **An existing container install.** If a Quadlet file or another program or
  hub on the port is found (a container is seen only through its port), the installer stops and
  points at [the migration below](#move-from-a-container).
- **PATH.** It prints the line to add when `~/.local/bin` is not on `PATH`, and
  warns when another `agent-hub` shadows the new one. It edits no shell file.
  On Windows it adds the folder to the user `Path`.

## Where things live

| | Linux | macOS | Windows |
|---|---|---|---|
| Binary | `~/.local/bin/agent-hub` | `~/.local/bin/agent-hub` | `%LOCALAPPDATA%\Programs\agent-hub\agent-hub.exe` |
| Config | `~/.config/agent-hub/config.toml` | `~/.config/agent-hub/config.toml` | `%APPDATA%\agent-hub\config.toml` |
| Data | `~/.local/share/agent-hub` | `~/Library/Application Support/agent-hub` | `%LOCALAPPDATA%\agent-hub\data` |
| Logs | the journal, or `~/.local/state/agent-hub/hub.log` | `~/Library/Logs/agent-hub/hub.log` | `%LOCALAPPDATA%\agent-hub\logs\hub.log` |
| Previous binary | `~/.local/state/agent-hub/agent-hub.prev` | `~/Library/Application Support/agent-hub-state/agent-hub.prev` | `%LOCALAPPDATA%\agent-hub\state\agent-hub.prev.exe` |
| Service | `~/.config/systemd/user/agent-hub.service` | `~/Library/LaunchAgents/dev.agync.agent-hub.plist` | the task `\Agent Hub` |

On Linux the XDG variables move these rows, and none moves the binary:
`XDG_CONFIG_HOME` the config and the unit, `XDG_DATA_HOME` the data, and
`XDG_STATE_HOME` the log file and the previous binary. The data directory holds `hub.db` at its top, and also `hub.lock`
and `hub.pid.json`, which belong to the running hub and are not part of a
backup. `service install` writes the data directory into `config.toml` as an
absolute path, so the service and every command agree on it.

The service reads that one `config.toml`, named by `HUB_CONFIG` in the unit.
It does not read `/etc/agent-hub/config.toml` or the `HUB_*` variables of your
shell: the unit clears them, and `status`, `start`, `stop` and `uninstall` read
the same pinned file, so they agree with the service from any shell. A key that
is already set is kept when you run `service install` again, and the row says
`kept` or `added <keys>`. An empty `admin_token` counts as unset. On macOS the
install also writes `log_file` into the config.

## Run it

```sh
agent-hub status          # running or not, how it is managed, where it listens
agent-hub stop            # drain, checkpoint the store, stop
agent-hub start
agent-hub restart
agent-hub logs -f         # follow the log; -n 200 shows the last 200 lines
```

With a service installed these go through the service manager, and without one
they act on a plain background process the same way, so the commands do not
change. `start` on a running hub says `already running` and exits 0.

```text
agent-hub 1.13.0
  state        running, ready (pid 4121, up 3h12m)
  managed by   systemd user service (enabled, starts at boot)
  address      http://127.0.0.1:8080/
  data         ~/.local/share/agent-hub (412 MiB)
  config       ~/.config/agent-hub/config.toml
  logs         agent-hub logs
```

On macOS the `managed by` row reads `launchd agent (enabled, starts at login)`,
or `launchd agent (disabled in Login Items)`. Without a service it reads
`nothing (no service installed)` for a stopped hub, and names the plain process
for a running one. When the service manager does not answer, the row says so
and carries its message. A hub that a terminal started while the service is
installed is labelled `the running hub was started by ...`, and `stop` ends it
by process id, never through the manager.

`agent-hub status --json` prints the same facts for a script. `status` exits 0
when the hub is running and ready, 1 when it is running and not ready, and 3
when it is not running. `stop` waits for the hub to drain, 10 seconds, then to
checkpoint the store, and reports `store checkpointed` only when that happened.
It gives up after `--timeout` seconds (20 by default) and exits 1.
`stop --force` kills the process at once, with no drain and no checkpoint.

A bare `agent-hub` prints the status block and the usage and exits 0. It does
not serve. The foreground command is `agent-hub serve`; use it in a container,
a hand-written unit or a terminal you mean to keep open.

### Logs

| Where it runs | Where the log goes |
|---|---|
| Linux service | the journal: `agent-hub logs` runs `journalctl --user -u agent-hub` |
| macOS service, Windows task, plain process | the log file in the table above, rotated at 10 MiB with three archives kept |

`log_file` under `[hub]` (or `HUB_LOG_FILE`) sets the file. A hub that cannot
open its log file does not start. A `log_file` named `hub.lock`, `hub.pid.json`
or ending in `.start` is refused, as is a symlink. A plain `start` also keeps
the child's own output, what it prints before the logger is up, in
`<log_file>.start`, rewritten at each start.

### One store, one process

A second hub on the same data directory is refused before the engine opens
anything, with `a hub is already serving <data> (pid 4121, systemd user
service); use agent-hub stop, or set HUB_URL to reach it`. The same holds for
the embedded `agent-hub mcp` mode and for `restore`. A developer running
`agent-hub serve` in a checkout therefore cannot touch the installed hub's
store by accident. When a service manager owns the hub, `stop` goes through the
manager. Otherwise it signals only the process the lock record names; the next
section says how strictly that is checked on each platform.

### Limits worth knowing

- On Linux and Windows a process is identified by its pid and start time, so a
  `stop` of a hand-started hub signals only the process the lock names. On macOS
  it checks only that the pid is alive, which is weaker. A hub that a service
  manager owns is stopped through the manager on every platform. `stop --force`
  without a process file descriptor falls back to the
  raw pid, with a microsecond window for pid reuse after the final check.
- After a drain timeout `stop` still tries to checkpoint, and never reports
  `store checkpointed` unless that happened.
- A signal that arrives before the hub installs its handlers ends a process
  that has no open store yet, so nothing is left half written.
- Two stores that you point at one `log_file` share its `.start` file. Give each
  its own.
- The tailnet's per-connection tasks are not waited for at shutdown. Stopping
  the hub closes them with the process.

## Start at boot or at login

| Platform | Default | What decides it |
|---|---|---|
| Linux | boot | the user manager runs without a login only while linger is on; the installer tries `loginctl enable-linger` for your own user, which needs no `sudo` where polkit allows it |
| macOS | login | a LaunchAgent exists only for a logged-in user; `--boot` is refused (exit 2) and the message names automatic login and a LaunchDaemon you install yourself |
| Windows | logon | a task with a logon trigger; `--boot` works only from an elevated PowerShell, never elevates itself, and registers a boot trigger (a normal shell exits 1) |

Boot is a request, not a guarantee. On Linux, if the system refuses linger, the
install carries on, the hub starts at login,
and the output ends with the one command to run yourself:

```text
  starts at    login (linger is off: the hub stops when you log out; could not turn it on: <reason>)
To start it at boot with no login, run once:
  sudo loginctl enable-linger <uid>
```

The line names your numeric user id. The install never runs `sudo` itself.

`agent-hub service install --login` skips the attempt and never reads or
changes linger. When no user manager is running and linger cannot be turned on
(a box reached with `su` or `sudo -u`, say), the install exits 69 with the same
`sudo loginctl enable-linger` line and `agent-hub start` as the fallback that
needs no manager. With `--login` and no manager the message says to log in to a
session that starts one, or to drop `--login`.

On macOS, version 13 and later shows a "Background item added" notice and lists
the hub under Login Items. If you switch it off there, `status` reports
`launchd (disabled in Login Items)`. Over SSH with nobody logged in, the agent
loads into the `user/<uid>` domain, and the `starts at` row says it runs until
the next reboot; log in once to make it last.

### The Windows task

The task is named `Agent Hub`. Its definition is kept at
`%LOCALAPPDATA%\agent-hub\state\agent-hub-task.xml`, and its presence is what
`status` and `uninstall` read as installed. The task runs `agent-hub service
run`, a small supervisor, so the hub restarts without the Task Scheduler.

- **Restarts.** It restarts the hub when it exits with an error, waiting 1
  second and doubling to 30, and gives up with exit 1 after five exits in a
  minute. A run of a minute resets the wait. A usage error (2) or a
  configuration error (78) is never restarted. A second supervisor exits 0 and
  leaves the first one running.
- **Log.** The hub writes its own log file. What it prints before the logger is
  up, and the supervisor's own notes, go to `<log_file>.start`.
- **Stop.** `stop` asks the hub to stop through a named event, so the store
  drains and is checkpointed. The supervisor never kills the hub for a slow
  stop: a stop that times out exits 1 and names `--force`. `stop --force` ends
  the task and the hub at once, with no checkpoint. `start`, `stop` and
  `restart` also work on a plain process in the same way.
- **Unknown state.** When the Task Scheduler cannot be queried, `start`, `stop`
  and `uninstall` exit 69 and do nothing: an unreadable answer is never taken
  as stopped.
- **Boot.** `service install --boot` needs an elevated PowerShell and registers
  a boot trigger. The hub then runs without a login.
- **Identity.** `service uninstall` and `uninstall` only remove a task that runs
  this install's binary with its `--config`. A task of that name that runs
  something else is refused (exit 1) and the command prints the `schtasks
  /Delete` line to run yourself.
- **Paths.** A binary path that contains `%`, `!`, `"`, `^`, `&`, `|`, `<` or `>`
  cannot be used with the task, because `cmd` and the scheduler would expand it.

The installer adds its folder to your user `Path` and records the entry in
`%LOCALAPPDATA%\agent-hub\state\path-added`. `uninstall` removes only an entry
recorded there, so a folder that was already on your `Path` stays.

### The unit the installer writes

On Linux the unit runs `agent-hub serve` with `HUB_CONFIG` pinned, restarts on
failure but not on exit 78 or 2, allows 20 seconds to stop, and applies
seccomp-only hardening (`NoNewPrivileges`, a system-call filter, a private
umask). It does not use `ProtectSystem` or `PrivateTmp`, which fail where a
user manager cannot create user namespaces. The file says it is rewritten by
`service install`. The unit also clears every `HUB_*` setting the hub reads, so
the pinned config file is the only source, and sets `AGENT_HUB_MANAGED_BY` to
`systemd` (the plist sets `launchd`), which is how the hub tells its own
service process from one a terminal started.

Put your changes in a drop-in with `systemctl --user edit agent-hub`. A drop-in
that overrides `HUB_CONFIG` is your choice, and the service then runs with that
file, but `status`, `stop` and `uninstall` follow the config the unit file
names, so they can disagree with it. Leave `HUB_CONFIG` alone.

## Change the bind

Loopback is the default, so only this machine reaches the hub. To reach it from
others, bind a LAN or tailnet address. On an existing install, edit `[hub] bind`
in `config.toml` and run `agent-hub restart`: `service install --bind` is
refused (exit 2) when the file already holds a different bind. Update `[client]
url` as well if it still names the old address, or the CLI and the plugins keep
calling it. On a first install you can pass the address up front:

```sh
agent-hub service install --bind 192.168.1.20:8080
```

The token the install created is already in the config, and a non-loopback bind
needs one.
A LAN bind carries plain HTTP; [Network and access](/operate/network-and-access.md)
covers TLS and the tailnet.

## Upgrade

Run the installer again:

```sh
curl -fsSL https://hub.agync.dev/install.sh | sh
```

There is no `agent-hub upgrade` subcommand: the installer is the upgrade. It
resolves the latest release. When that is what you have, it says `already
installed` and does not replace the binary (`--force` reinstalls), but it still
runs `agent-hub service install --yes` with your flags, which is idempotent: a
first run that failed on a busy port, or `--bind` on a second run, takes effect.
`--no-service` skips that. Otherwise it keeps the old binary, swaps in
the new one, runs `agent-hub service install --yes` so the new binary renders
its own service definition and merges any new config keys, and restarts the hub
if it was running. That `service install` does not pass `--no-start`, so an
upgrade can start a hub you had stopped. Config and data are never overwritten.
The store migrates on start, and the hub copies it to
`backups/pre-migration-v<from>-<timestamp>.db` first.

To roll back, stop the hub, copy `agent-hub.prev` (`agent-hub.prev.exe` on
Windows) from the state directory over the binary, restore the store from the pre-migration copy if the schema
changed, and start it.
[Upgrade and roll back](/operate/upgrade-and-rollback.md) has the steps.

On Windows a running `.exe` cannot be overwritten but can be renamed: the
installer stops the hub, renames the old binary, moves the new one in, starts
it, and removes the renamed copy on a later run. The previous binary is
`agent-hub.prev.exe` in the state directory.

## Uninstall

```sh
agent-hub service uninstall   # stop and unregister; keep binary, config and data
agent-hub uninstall           # also remove the binary
agent-hub uninstall --purge   # also remove the hub's config table, data and logs
```

| Removed | `service uninstall` | `uninstall` | `uninstall --purge` |
|---|---|---|---|
| The running hub, stopped with a drain | yes | yes | yes |
| The unit, plist or task | yes | yes | yes |
| The binary and the previous binary | no | yes | yes |
| The `[hub]` table of `config.toml` | no | no | yes |
| The `[client]` table | no | no | no |
| The data directory | no | no | yes, after the checks |
| Logs and state | no | no | yes |

`[client]` stays because agents and plugins on the machine read it. A
container's volume or Quadlet file is never touched. The paths come from the
config file the installed service names, never from your shell's `HUB_*`
variables, and a service definition that cannot be read is exit 78 with nothing
removed.

`uninstall` lists what it will remove and asks `Continue? [y/N]`. `--purge`
then shows the size of the data and asks you to type `delete`. Take a backup
first. `backup` is offline and refuses a running store, so stop the hub, pass
`--data-dir` with the installed data path if it differs from what your shell
configures (`agent-hub backup --out DIR --data-dir <data path>`), and verify
the result with `agent-hub check --data-dir DIR` before you purge. With no terminal and no `--yes`,
`uninstall` and `service uninstall` exit 2 and remove nothing. Declining exits
1.

`uninstall --purge` runs its checks first, before it stops or removes anything,
and refuses (exit 1, `Nothing was removed`) when the data directory holds files but
no `hub.db`, is `/` or your
home directory, is a mount point, is a symlink or is reached through one, holds
the config file, the binary or the service definition, or is held by a running
hub. It deletes only what the hub itself writes: the store and its sidecars,
the lock and its record, the hub's own subdirectories, the log files and the
previous binary. A data directory that is missing, empty, or holds only the lock
and its record is accepted; the prompt still asks you to type `delete`, but
shows no data size. After the checks and the prompts it stops the managed hub
and removes nothing when that fails. A directory is removed only once that leaves it empty.
Anything else in it, such as a note you dropped there, stays and is listed
under `Left in place (not written by the hub)`; that run still exits 0. The config writer's lock file, `.<config filename>.agent-hub.lock` (for the
default name, `.config.toml.agent-hub.lock`), also stays beside the config.
The systemd journal is not erased either; clear it with `journalctl` if you
need to. `uninstall` refuses (exit 1) when the manager still has the job loaded
but its definition is missing or names another config; unload it first with the
command it prints. Linger is left as the installer found it.

Two limits are accepted. Purge does not look for a bind mount you placed inside
one of the hub's own subdirectories (`sessions`, `artifacts` and the like), and
it would empty it. And a stale, non-empty `XDG_RUNTIME_DIR` can make `start`
and `status` report that the manager does not answer; unset it or log in again.

On Windows the binary cannot delete itself while running. It starts a detached
command that retries for up to a minute, deletes the binary, its rollback files
and the install folder if that is empty, and says so. This is best effort: a
leftover is swept by the next install, and the check that precedes the delete is
not repeated, so a file swapped in the meantime by someone who already controls
your account is outside what it guards against.

`install.sh --uninstall [--purge]` runs the same command.

## Move from a container

The two do not run side by side on one port, and the installer will not
install over a Quadlet unit. To move a container's data to a native install:

1. Stop the container and the Quadlet unit, then move its definition aside so
   the installer does not refuse: `systemctl --user stop agent-hub`, `podman
   stop agent-hub` if it still runs, and `mv
   ~/.config/containers/systemd/agent-hub.container ~/agent-hub.container.old`
   (the file's place on your host may differ).
2. Export the volume: `podman volume export agent-hub-data -o hub-data.tar`.
   With Docker, run a throwaway `tar` container over the volume.
3. `systemctl --user daemon-reload`, then install without starting:
   `agent-hub service install --no-start`.
4. Create the data directory, then unpack into it:
   `mkdir -p <data directory> && tar -xf hub-data.tar -C <data directory>`.
5. Check it: `agent-hub check --data-dir <data directory>`.
6. To keep the admin token you used, copy `HUB_ADMIN_TOKEN` from `hub.env` into
   `[hub] admin_token`. Member tokens live in the store and keep working
   whatever the admin token is.
7. `agent-hub start`. Delete the moved-aside Quadlet file when you are sure.

### Upgrading from a container or a bare-binary unit

A bare `agent-hub` no longer serves. Anything that ran the bare binary, such as
a hand-written systemd unit, a supervisor entry or a script, must run `agent-hub
serve` instead; the release notes list this as a breaking change. The container
image already sets `CMD ["serve"]`, so images, Quadlet files and compose files
need no change.

## A system-wide service

A per-user service covers a homelab node. A hub that must run with nobody ever
logged in, on macOS, or under a dedicated account, is a system service, which
needs `sudo` and is yours to set up. The installer never writes one. These are
sketches to adapt, with placeholders for the account and paths.

A Linux system unit, saved as `/etc/systemd/system/agent-hub.service`:

```ini
[Unit]
Description=Agent Hub
After=network-online.target
Wants=network-online.target

[Service]
User=agent-hub
ExecStart=/usr/local/bin/agent-hub serve
Environment=HUB_CONFIG=/etc/agent-hub/config.toml
Environment=HUB_DATA_DIR=/var/lib/agent-hub
Restart=on-failure
RestartPreventExitStatus=78 2
TimeoutStopSec=20
StateDirectory=agent-hub

[Install]
WantedBy=multi-user.target
```

A macOS LaunchDaemon in `/Library/LaunchDaemons/dev.agync.agent-hub.plist`
takes the same shape as the LaunchAgent the installer writes, with a `UserName`
key for the account, absolute paths to the binary, config and data, and
`RunAtLoad` set. Load it with `sudo launchctl bootstrap system <plist>`.

Remove the per-user service first with `agent-hub service uninstall`, so two
units do not contend for the port.

## You should now see

`agent-hub status` reports the hub running and ready, `agent-hub health`
prints `ready`, and the app opens at the printed address. After a reboot
(Linux with linger, or the next login elsewhere) it is running again.

## If something goes wrong

| You see | Cause and fix |
|---|---|
| `port 8080 is in use by another program` | another program, or a container publishing it, holds it. Stop that, or set `[hub] bind` |
| exit 69 from `service install` | no service manager reachable, often `su` or a bare SSH session, or WSL without systemd. Nothing was written unless the manager went away during registration or the final start, which leaves the config and registration in place. `agent-hub start` runs the hub as a plain process; it needs a token, see [Without a service manager](#without-a-service-manager) |
| `refusing to remove without --yes on a non-interactive terminal` | exit 2; add `--yes` |
| `start` or `status` says the manager does not answer | a stale `XDG_RUNTIME_DIR`; unset it or log in again |
| the hub stops when you log out | linger is off; run the `sudo loginctl enable-linger` line the install printed |
| `a hub is already serving` | the lock is held. `agent-hub status` names it; `agent-hub stop` ends it |
| a restart loop on a bad config | `agent-hub config --check` names the key; the manager does not restart exit 78 on Linux or Windows |
| `agent-hub` is not found | the install printed the `PATH` line to add |

## See also

- [Install and run the hub](/start/install.md) - the lanes side by side
- [Upgrade and roll back](/operate/upgrade-and-rollback.md) - the store and the binary
- [CLI](/reference/cli.md) - every lifecycle command and exit code
- [Configuration](/reference/configuration.md) - `log_file`, `data_dir` and the config paths
- [Network and access](/operate/network-and-access.md) - the bind, TLS and a tailnet
