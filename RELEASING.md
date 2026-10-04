# Releasing

Agent Hub ships as one binary built from source and one container image on
GitHub Container Registry. A release is: the version bumped in one commit, an
annotated tag, the workflow that builds the image and writes the GitHub
release, then verification against the published image.

The runbook is ordered the way a release actually runs. Every command is a real
one from this repository; nothing here is aspirational.

## Preconditions

`main` is clean and current, and the gate that CI runs is green on it:

```sh
git switch main
git pull --ff-only
git status --porcelain          # must print nothing
HUB_REQUIRE_BROWSER=1 make check
```

`make check` is the whole gate: the declarative hooks, the one-engine check,
clippy with warnings as errors, rustfmt, the OKF bundle validation, the PWA
static and browser checks, the headless accessibility audit, the optional
tailnet build, the serve-only build the image uses, and the test suite.
`HUB_REQUIRE_BROWSER=1` makes a missing Playwright a failure rather than a
skip, so the browser checks cannot pass by not running.

The change itself must already be on `main`:

- `CHANGELOG.md` carries an accurate `## [<version>]` section, because the
  release notes are that section and the workflow copies it verbatim.
- `docs/log.md` carries an entry for every wiki change in the release. The
  release workflow does not touch the wiki bundle, so a missing entry is only
  caught in review.

Record the version and the image name once, and use them in every command below.
The repository lives under `abn`, so `ghcr.io/$OWNER/agent-hub` is the image the
tag publishes; change `OWNER` if the repository moves:

```sh
VERSION=1.0.0
OWNER=abn
IMAGE=ghcr.io/$OWNER/agent-hub
```

## The version bump

`CARGO_PKG_VERSION` is the single source of the version. The build script reads
the commit from the checkout, or from `GIT_COMMIT` when a build passes it, so
nothing about the commit is typed into a file.

```sh
git switch -c chore/release-$VERSION main
```

Edit `Cargo.toml`:

```toml
[package]
name = "agent-hub"
version = "1.0.0"
```

Let cargo record it in the lock file rather than editing it by hand:

```sh
cargo check --locked   # fails if Cargo.lock disagrees with Cargo.toml
```

Then verify what the interface will show. The Version row reads the version from
`CARGO_PKG_VERSION` and the short commit from the build script, both resolved
at compile time, so a real run is the only honest check:

```sh
make build
HUB_DATA_DIR=target/tmp/release-data HUB_BIND=127.0.0.1:18081 \
  HUB_ADMIN_TOKEN=release-check \
  ./target/debug/agent-hub &
curl -sS -H 'Authorization: Bearer release-check' \
  http://127.0.0.1:18081/api/v1/storage
```

The payload carries `"version":"1.0.0"` and the commit the binary was built
from. Stop the hub, then commit:

```sh
kill %1
git add Cargo.toml Cargo.lock
git commit -m "chore(release): $VERSION"
```

A release commit is conventional, has no trailers, and carries only the version
change. The changelog section and any install-path change ride their own
commits ahead of it.

## The tag

The tag is annotated, so `git describe` and a failed push have a message to
show. Push the branch first: a tag whose commit is not on the remote is worse
than no tag.

```sh
git push origin chore/release-$VERSION
git push origin main
```

Tag the commit on `main` that carries the release, then drop the branch:

```sh
git switch main
git pull --ff-only
git tag -a "v$VERSION" -m "Agent Hub $VERSION"
git tag -n99 -l "v$VERSION"    # read the message back before pushing
git push origin "v$VERSION"
git branch --delete chore/release-$VERSION
```

Pushing the tag starts `.github/workflows/release.yml`. It builds the
`Containerfile` with `GIT_COMMIT` set to the tagged commit, pushes the image to
`ghcr.io/abn/agent-hub` as both `1.0.0` and `latest`, runs the image and probes
its readiness, then creates the GitHub release with the matching
`CHANGELOG.md` section as its body.

The workflow is the whole release. The manual path below exists for the times
it cannot run, and it is also what you run to verify a published image from a
second machine.

## The container image

### Through the tag

The workflow does the build and the push. Watch it:

```sh
gh run watch
gh run view --log-failed    # when it fails
```

### By hand

The fallback when the workflow cannot run, and the path the image is verified
on. Log in to the registry first; the token needs `write:packages` and
`read:packages`.

```sh
printf '%s' "$(gh auth token)" | podman login ghcr.io \
  --username "$(gh api user --jq .login)" --password-stdin
podman build -f Containerfile \
  --build-arg GIT_COMMIT="$(git rev-parse --short HEAD)" \
  --tag "$IMAGE:$VERSION" \
  --tag "$IMAGE:latest" \
  .
podman push "$IMAGE:$VERSION"
podman push "$IMAGE:latest"
```

The image serves only. It is built with `--no-default-features`, so it has no
stdio proxy and no one-shot calls; those run on the agents' machines.

## The GitHub release

The workflow creates it from the changelog section. To do it by hand, extract
the section so the body and the file cannot drift:

```sh
mkdir -p target/tmp
awk -v heading="## [$VERSION]" '
  substr($0, 1, length(heading)) == heading { inside = 1; next }
  inside && /^## \[/ { exit }
  inside { print }
' CHANGELOG.md > "target/tmp/release-notes-$VERSION.md"
gh release create "v$VERSION" \
  --title "Agent Hub $VERSION" \
  --notes-file "target/tmp/release-notes-$VERSION.md"
```

`gh release create` also moves the tag if the tag does not exist yet. Push the
annotated tag first when you want the message to survive.

## Verify the release

Nothing is done until the published artifact has answered.

### The image serves

```sh
podman run --detach --name agent-hub-verify \
  --publish 18080:8080 \
  --env HUB_ADMIN_TOKEN=release-check \
  --volume agent-hub-verify-data:/data \
  "$IMAGE:$VERSION"
```

The container may need a moment to migrate an empty data directory before it
answers. Poll the probe rather than sleeping a fixed time:

```sh
for attempt in $(seq 30); do
  podman exec agent-hub-verify \
    /usr/local/bin/agent-hub health --url http://127.0.0.1:8080 && break
  sleep 1
done
```

The image runs as a non-root user and writes only to `/data`, so the rest of
the filesystem can be read-only, and it needs no shell to be probed:

```sh
podman exec agent-hub-verify \
  /usr/local/bin/agent-hub health --url http://127.0.0.1:8080
```

`health` GETs `/readyz` and exits 0 only when the store is usable: the schema
version is read and supported, the data directory and the store are the ones
the process opened, a content read confirms it is this hub's store, and the
volume has room. A non-zero exit means the image is not ready to serve, and the
release is not verified.

From the host, the same answers are visible over the published port:

```sh
curl -sS http://127.0.0.1:18080/healthz     # ok
curl -sS http://127.0.0.1:18080/readyz      # the schema version
curl -sS http://127.0.0.1:18080/SKILL.md    # the agent guide, address filled in
```

### The version is the released one

The published image carries the tagged commit, so the row agrees with the tag:

```sh
curl -sS -H 'Authorization: Bearer release-check' \
  http://127.0.0.1:18080/api/v1/storage | grep -o '"version":"[^"]*","commit":"[^"]*"'
```

Clean up the verification container and its volume when the checks pass:

```sh
podman rm --force agent-hub-verify
podman volume rm agent-hub-verify-data
```

### The binary path

The binary is built from source by the reader, not shipped as an artefact:

```sh
cargo build --release --locked
./target/release/agent-hub health --url http://127.0.0.1:8080
```

## Roll back

### The image

An image is immutable, so a rollback is a redeploy of the previous tag:

```sh
podman pull "$IMAGE:1.0.0"
podman run --detach --name agent-hub \
  --publish 8080:8080 \
  --env HUB_ADMIN_TOKEN="$HUB_ADMIN_TOKEN" \
  --volume agent-hub-data:/data \
  "$IMAGE:1.0.0"
```

Roll the `latest` tag back too, or the next pull re-breaks it:

```sh
podman pull "$IMAGE:<previous>"
podman tag "$IMAGE:<previous>" "$IMAGE:latest"
podman push "$IMAGE:latest"
```

### The GitHub release

```sh
gh release delete "v$VERSION"
git push origin --delete "v$VERSION"
```

A consumed tag is not reusable. Release the fix as a new patch version instead.

### The store

The store schema migrates forward on startup, one version per transaction, and a
binary refuses to open a store whose schema is newer than the most it supports.
So rolling the binary back is a data operation, not a file copy, and the
[operations runbook](docs/usage/operations.md) owns it: stop the hub, copy the
matching `backups/pre-migration-v<from>-<stamp>.db` from the data directory's
`backups/` over `hub.db`, copy the `-wal` and `-shm` sidecars with it, remove any
the backup did not carry, and start the older binary. Take an offline backup
first, while no hub holds the store:

```sh
agent-hub backup --out /path/to/pre-rollback-backup
agent-hub check --data-dir /path/to/data
```
