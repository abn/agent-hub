---
type: Guide
title: Install and run the hub
description: Install the container, a release binary, or from source, and run it
tags: [user, install, config]
status: stable
---

# Install and run the hub

By the end you will have the hub running and answering its health probes. This
page is the one canonical install; [Choose your setup](/start/choose-your-setup.md)
tells you which lane is yours, and [Deploy as a service](/operate/deploy-as-a-service.md)
turns a running hub into one that starts on boot.

## Before you start

| Thing | Why |
|---|---|
| An admin token | the control surface refuses every request without one |
| A place for the data | a named volume, or a directory you control |
| Podman or Docker, or a Rust toolchain | one of the install lanes below |

Set the token once and keep it out of your shell history where you can:

```sh
HUB_ADMIN_TOKEN="$(openssl rand -hex 32)"
```

## The container

Pull and run the published image. This is the default lane.

```sh
podman pull ghcr.io/abn/agent-hub:1.0.0
podman run --detach --name agent-hub \
  --publish 127.0.0.1:8080:8080 \
  --env HUB_ADMIN_TOKEN="$HUB_ADMIN_TOKEN" \
  --volume agent-hub-data:/data \
  ghcr.io/abn/agent-hub:1.0.0
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
  --tag ghcr.io/abn/agent-hub:1.0.0 .
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

Each GitHub release carries one archive per platform: Linux on x86_64 and arm64,
statically linked, and macOS on Apple silicon and Intel. Windows is not built.
The archive holds the full binary, server and client in one, with a `.sha256`
checksum beside it. Download the one for the host, check it, and unpack it:

```sh
VERSION=v1.2.0
TARGET=aarch64-apple-darwin
NAME="agent-hub-$VERSION-$TARGET"
BASE="https://github.com/abn/agent-hub/releases/download/agent-hub-$VERSION"
curl -fsSLO "$BASE/$NAME.tar.gz"
curl -fsSLO "$BASE/$NAME.sha256"
shasum -a 256 -c "$NAME.sha256"
tar -xzf "$NAME.tar.gz"
"./agent-hub-$VERSION-$TARGET/agent-hub" --help
```

`TARGET` is one of `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`aarch64-apple-darwin`, or `x86_64-apple-darwin`.

## From source

A stable Rust toolchain, 1.97 or newer. The pinned engine compiles C, so you
also need a C toolchain with `clang` and `cmake`. `make build` produces a debug
binary; for a release binary use the locked build:

```sh
cargo build --release --locked
```

## Run the binary

Point the process at a data directory and a bind address:

```sh
HUB_DATA_DIR=./data HUB_BIND=127.0.0.1:8080 HUB_ADMIN_TOKEN=change-me \
  ./target/debug/agent-hub
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
`http://127.0.0.1:8080/` and paste the admin token:
[Open the app and log in](/start/first-login.md) walks through it.

> [!NOTE]
> A missing configuration file is not an error, and a file that others can read
> warns on stderr when it holds a token, and still works. Every setting, its
> default, and its environment variable is in [Configuration](/reference/configuration.md).

## See also

- [Choose your setup](/start/choose-your-setup.md) - which lane is yours
- [Deploy as a service](/operate/deploy-as-a-service.md) - start on boot
- [Network and access](/operate/network-and-access.md) - TLS, a reverse proxy, a tailnet
- [Configuration](/reference/configuration.md) - every setting
