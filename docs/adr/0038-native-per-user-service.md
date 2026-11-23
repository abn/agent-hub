---
type: Decision Record
title: A native per-user service with a binary-owned lifecycle
description: The hub installs and runs as a per-user service with no container runtime, and the binary owns every lifecycle verb.
tags: [adr, install, operations]
status: stable
---

# 0038. A native per-user service with a binary-owned lifecycle

## Status

Accepted. The container stays supported and is still the isolation option.

## Context

The only supported way to keep a hub running was a container under Quadlet,
Docker or compose. A homelab or NAS node without a container runtime had a
release binary and nothing to start it on boot, stop it cleanly or upgrade it.
Windows did not build. A bare `agent-hub` served `./data` in the current
directory, so typing it in the home directory of a machine with a native
install created a second, empty store beside the real one.

## Decision

**The binary owns the lifecycle; the script is thin.** `install.sh` and
`install.ps1` download a release archive, check its checksum, place the binary
and run `agent-hub service install`. Start, stop, restart, status, logs,
`service` and uninstall are subcommands of the binary, and an upgrade is the
installer run again; it is one Rust implementation
with a backend per platform. A script that did everything would be two copies
of every rule, could not uninstall without being fetched again, and would have
to know how every past version rendered its service definition.

**One per-user service, no root.**

| Platform | Service | Starts |
|---|---|---|
| Linux | systemd user unit | at boot through linger when the system allows it, otherwise at login, with the one command that fixes it |
| macOS | launchd LaunchAgent in the `gui/<uid>` domain | at login |
| Windows | Scheduled Task at logon, running a small supervisor | at logon, or at boot from an elevated shell |

The default bind is loopback, `127.0.0.1:8080`. Installing generates an admin
token and saves it in the user's `config.toml`. The installer never uses `sudo`
and never elevates itself. Where a platform needs a privileged step, such as
lingering on a host that refuses it, the installer prints the one command for
the human to run. macOS has no boot-time LaunchAgent, so `--boot` there is a
refusal that names the two ways to get it.

**Without a service, the same verbs act on a plain process.** `start` spawns a
detached `agent-hub serve` and `stop` signals it, so a developer box with no
service behaves the same way.

**One store, one process, enforced by a file lock.** `serve` takes an exclusive
OS lock on `hub.lock` in the data directory before it opens the engine, and
publishes who holds it in `hub.pid.json`. The kernel drops the lock when the
process dies, so a stale record never blocks a start. The record is read only
while the lock is held, `stop` signals a process only when the record, the
lock and the process agree (the pid and start time on Linux and Windows, the
pid alone on macOS), a hub that a manager owns is stopped through the manager,
and `restore` takes the same lock. This
is the rule from the [operating model](/architecture/operating-model.md), that
two processes never quietly share one store, made true before any engine I/O on
every platform.

**A bare `agent-hub` no longer serves.** It prints the hub's status and the
usage and exits 0. `serve` is the foreground mode and takes no flags. The
container image sets `CMD ["serve"]`, so images, Quadlet files and compose
files behave as before. This is a break in the command line: a hand-written
unit, a script or a supervisor that ran the bare binary must run
`agent-hub serve`. The commit that made the change carries a `BREAKING CHANGE:`
footer, so the release notes list it.

**Upgrading is running the installer again.** There is no `agent-hub upgrade`
subcommand. The new binary re-renders its own
service definition and merges missing keys into the config, never replacing one
that is set. The previous binary is kept for a rollback. The store migrates on
start, as before, after the hub copies it to a pre-migration backup. The
installer's `service install` does not pass `--no-start`, so an upgrade can
start a hub that was stopped.

**Releases are checksummed and attested.** Each archive carries a `.sha256`,
and the release workflows attest each archive's build provenance. The installer
verifies it with `gh attestation verify` when `gh` is present, or insists on it
with `--require-provenance` (`-RequireProvenance`). A checksum fetched from the same origin proves
integrity, not authorship, and the docs say so.

**Windows ships gated.** Linux and macOS do not wait on it. A release carries
the Windows archive only when its Windows build passed: the build runs the real
installer, the task, a stop and an uninstall on the exact archive before it is
attested and uploaded. For a release without one the PowerShell installer exits with the unsupported-platform
code and points at WSL.

## Consequences

- A node with no container runtime installs, runs, upgrades and removes the hub
  with one command each, and on Linux the hub survives a reboot without a
  login when linger is granted, and otherwise starts at the next login.
- The service reads the user's `config.toml` and no other: the unit and the
  plist pin the file. The system file under `/etc` is not read by a service
  the installer made.
- A per-user service stops when the user's last session ends, unless lingering
  is on. The installer tries to turn it on and says plainly when it could not.
- A hub in a container and a native hub on one host fight over the port, and a
  Quadlet unit would be shadowed by ours. The installer refuses when it finds
  either and points to the migration steps. Moving between them is documented,
  not automated.
- The one-line change a bare command needs is visible in the release notes, in
  [the CLI reference](/reference/cli.md) and in the container's own command, and
  is the only break.
- Known limits, recorded rather than worked around. On macOS a process is
  identified by its pid alone before a signal, which is weaker than the start
  time check Linux and Windows get. A checkpoint is attempted after a drain that timed out,
  and then the stop is not reported as clean. Connections the embedded tailnet
  endpoint spawns are not joined on shutdown.
- Rejected: a Windows service under the service control manager, which needs
  administrator rights to install and a control protocol in the binary; a
  system-wide unit as the default, which needs `sudo`; having the script own
  the lifecycle; a bare `agent-hub` that serves when it has no terminal, whose
  behaviour would depend on how it was launched; a minisign key in the scripts,
  which is a long-lived secret to guard and rotate.
