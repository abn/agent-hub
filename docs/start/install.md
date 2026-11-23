---
type: Guide
title: Install and run the hub
description: Install the hub natively, in a container, from an archive or from source, and run it
tags: [user, install, config]
status: stable
---

# Install and run the hub

By the end you will have the hub running and answering its health probes. This
page is the one canonical install; [Choose your setup](/start/choose-your-setup.md)
tells you which lane is yours. The native install starts on its own (at boot on
Linux when the system allows it); the container lane turns into a boot-time service with
[Deploy as a service](/operate/deploy-as-a-service.md).

## Before you start

| Thing | Why |
|---|---|
| A normal user account | the native install is a per-user service and refuses to run as root |
| An admin token | the control surface refuses every request without one; the native install creates it for you |
| A place for the data | the native install picks one; the container needs a named volume or a directory |
| Podman or Docker, or a Rust toolchain | only for the container and source lanes |

## The native install

One command downloads the release for the machine, checks it, places the
binary, and registers a service that runs as your user. No container runtime
and no `sudo`. On Linux and macOS:

```sh
curl -fsSL https://hub.agync.dev/install.sh | sh
```

On Windows, in PowerShell (this needs a release that carries the Windows
build; an earlier one stops with a pointer to WSL):

```powershell
irm https://hub.agync.dev/install.ps1 | iex
```

The hub listens on `127.0.0.1:8080`, starts on its own (at boot on Linux when linger can be turned on, otherwise
at login; at login on macOS and Windows), and prints a generated admin token once. Manage it
with `agent-hub status`, `stop`, `start`, `restart` and `logs`. Running the
command again upgrades it. Flags, paths, upgrade, uninstall and a move from a
container are in
[Run the hub as a native service](/operate/native-service.md).

For a machine that only runs agents and needs the binary on `PATH`, add
`--no-service`: `curl -fsSL https://hub.agync.dev/install.sh | sh -s -- --no-service`.

## The container

Pull and run the published image. It is the isolation option, and the lane
for a host where you would rather not install anything.

Set the token once and keep it out of your shell history where you can:

```sh
HUB_ADMIN_TOKEN="$(openssl rand -hex 32)"
```

```sh
podman pull ghcr.io/agync/agent-hub:1.0.0
podman run --detach --name agent-hub \
  --publish 127.0.0.1:8080:8080 \
  --env HUB_ADMIN_TOKEN="$HUB_ADMIN_TOKEN" \
  --volume agent-hub-data:/data \
  ghcr.io/agync/agent-hub:1.0.0
```

`docker run` takes the same arguments. The image is distroless and runs as a
non-root user, so it needs no shell. It publishes to loopback here; widen the
bind in [Network and access](/operate/network-and-access.md) once you have a
token and a plan for TLS.

To build the image from a checkout instead of pulling it, pass the commit in.
The image context carries no `.git`, and the Settings Version row reads the
version and that commit:

```sh
podman build -f Containerfile \
  --build-arg GIT_COMMIT="$(git rev-parse --short HEAD)" \
  --tag ghcr.io/agync/agent-hub:1.0.0 .
```

Compose builds that image, mounts a named volume at `/data`, keeps the rest of
the filesystem read-only, sets a stop grace period above the hub's drain, and
refuses to render without `HUB_ADMIN_TOKEN`:

```sh
HUB_ADMIN_TOKEN="$HUB_ADMIN_TOKEN" \
  docker compose -f deploy/compose.yaml up --detach
```

Stop the stack with `docker compose -f deploy/compose.yaml down`; the named
volume keeps the data.

## A release binary

The installer fetches this for you; do it by hand to inspect the archive first
or to mirror it. Each GitHub release carries one archive per platform: Linux on
x86_64 and arm64, statically linked, and macOS on Apple silicon and Intel. A
release that includes the Windows build adds a `.zip` for `x86_64-pc-windows-msvc`.
The archive holds the full binary, server and client in one, with a `.sha256`
checksum beside it. Download the one for the host, check it, and unpack it:

```sh
VERSION=v1.2.0
TARGET=aarch64-apple-darwin
NAME="agent-hub-$VERSION-$TARGET"
BASE="https://github.com/agync/agent-hub/releases/download/agent-hub-$VERSION"
curl -fsSLO "$BASE/$NAME.tar.gz"
curl -fsSLO "$BASE/$NAME.sha256"
shasum -a 256 -c "$NAME.sha256"
tar -xzf "$NAME.tar.gz"
"./agent-hub-$VERSION-$TARGET/agent-hub" --help
```

`TARGET` is one of `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`aarch64-apple-darwin` or `x86_64-apple-darwin`. Run the unpacked binary with
`serve` to start the hub in the foreground.

On Windows the archive is a `.zip` for `x86_64-pc-windows-msvc`, present only on
a release whose Windows build passed. In PowerShell:

```powershell
$Version = "v1.2.0"
$Name = "agent-hub-$Version-x86_64-pc-windows-msvc"
$Base = "https://github.com/agync/agent-hub/releases/download/agent-hub-$Version"
Invoke-WebRequest "$Base/$Name.zip" -OutFile "$Name.zip"
Invoke-WebRequest "$Base/$Name.sha256" -OutFile "$Name.sha256"
(Get-FileHash "$Name.zip" -Algorithm SHA256).Hash.ToLower() -eq (Get-Content "$Name.sha256").Split(" ")[0]
Expand-Archive "$Name.zip" -DestinationPath .
& ".\$Name\agent-hub.exe" --help
```

The comparison prints `True` when the checksum matches. Run `agent-hub.exe serve`
to start the hub in the foreground.

## From source

A stable Rust toolchain, 1.97 or newer. The pinned engine compiles C, so you
also need a C toolchain with `clang` and `cmake`. `make build` produces a debug
binary; for a release binary use the locked build:

```sh
cargo build --release --locked
```

## Run the binary

`serve` is the foreground command. A bare `agent-hub` no longer serves: it
prints the hub's status and the usage. Point the process at a data directory
and a bind address:

```sh
HUB_DATA_DIR=./data HUB_BIND=127.0.0.1:8080 HUB_ADMIN_TOKEN=change-me \
  ./target/debug/agent-hub serve
```

On start it prints one line naming the bound address, the data directory, and
whether an admin token is configured, so a port `0` bind is readable without
turning on logs. `RUST_LOG=info` turns on the rest of the tracing output. The
release binary lives at `target/release/agent-hub`.

## Check it

From another shell:

```sh
curl http://127.0.0.1:8080/healthz
curl http://127.0.0.1:8080/readyz
```

`/healthz` answers `ok` while the process runs. `/readyz` asks the engine for
its schema version and answers with it, or a `503` problem when the store does
not reply. Point a container healthcheck at `/readyz`; the binary's own
`health --url http://127.0.0.1:8080` subcommand does exactly that and exits
non-zero the moment the store stops answering.

## Where the data lives

On first start the hub creates the data directory and its `sessions/`, `kb/`,
and `artifacts/` children, then opens `hub.db` at the top of the directory.
`hub.db` holds the event store; the child directories hold the per-session
brains, the project knowledge bases, and the artifact blobs. Back up the whole
directory as one unit; [Back up and restore](/operate/backup-and-restore.md)
has the procedure.

## You should now see

A hub logging its bound address and answering `/readyz` with a version. Open
`http://127.0.0.1:8080/` and paste the admin token (the native install printed
it, and saved it in `config.toml`):
[Open the app and log in](/start/first-login.md) walks through it.

> [!NOTE]
> A missing configuration file is not an error, and a file that others can read
> warns on stderr when it holds a token, and still works. Every setting, its
> default, and its environment variable is in [Configuration](/reference/configuration.md).

## See also

- [Choose your setup](/start/choose-your-setup.md) - which lane is yours
- [Run the hub as a native service](/operate/native-service.md) - the per-user service in full
- [Deploy as a service](/operate/deploy-as-a-service.md) - the container on boot
- [Network and access](/operate/network-and-access.md) - TLS, a reverse proxy, a tailnet
- [Configuration](/reference/configuration.md) - every setting
