# Releasing

A release is automated from the commit history. `release-please` reads the
conventional commits on `main`, keeps a release pull request open with the next
version and the changelog, and, when that pull request is merged, tags the
release and publishes it. The same workflow then builds the container image with
buildah and podman, pushes it to `ghcr.io/agync/agent-hub`, and verifies that the
published image serves.

The version lives in `Cargo.toml`, and `CARGO_PKG_VERSION` is the single source
of it. `release-please` keeps `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md` and
`.release-please-manifest.json` in step; nothing is edited by hand.

## Preconditions

`main` is where releases are cut from, and the gate that CI runs is green on it:

```sh
git switch main
git pull --ff-only
git status --porcelain          # must print nothing
HUB_REQUIRE_BROWSER=1 make check
```

## How a release happens

1. Merge conventional commits to `main`. `fix:` and `feat:` decide the bump.
2. `release-please` opens or updates a pull request titled
   `chore(main): release <version>`. It bumps `Cargo.toml`, refreshes
   `Cargo.lock`, and writes the `CHANGELOG.md` section.
3. Review the pull request. Its changelog section is the release notes.
4. Merge it. `release-please` creates the annotated tag `v<version>` and the
   GitHub release with the changelog as its body.
5. The same workflow run sees `release_created`, builds the image, pushes
   `ghcr.io/agync/agent-hub:<version>` and `:latest`, runs it, and probes it. The
   release is not done until that step is green.
6. In parallel it calls the binaries workflow, which builds the standalone
   binary for each target and attaches the archives to the release.
7. Also in parallel, the plugins ship (see [The plugins](#the-plugins)): the
   Antigravity archive is attached to the release, and the OpenCode package is
   published to npm when publishing is configured.

The workflow is `.github/workflows/release.yml`. The configuration is
`release-please-config.json` and `.release-please-manifest.json`.

## The image

`redhat-actions/buildah-build` builds it and `redhat-actions/push-to-registry`
pushes it, both logged in with `redhat-actions/podman-login`. The workflow
reuses the tags buildah reports for the push, so the built and the pushed tags
cannot drift. The image serves only: it is built `--no-default-features`, so it
has no stdio proxy and no one-shot calls, which run on the agents' machines.

The build runs in the release workflow rather than on the tag push, because a
tag created with the default `GITHUB_TOKEN` does not trigger another workflow.

## The binaries

`.github/workflows/binaries.yml` is a reusable workflow. It takes the release
tag, such as `agent-hub-v1.2.0`, and runs one job per target. Each job uploads
`agent-hub-v1.2.0-<target>.tar.gz` and its `agent-hub-v1.2.0-<target>.sha256`
to that release: the archive carries the version without the tag's prefix.
Each archive holds one directory with the binary, `LICENSE` and `README.md`.
The binary is the default build, so it carries the stdio proxy and the one-shot
calls that the image leaves out.

| Target | Runner | Notes |
|---|---|---|
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | Static, built with `cargo-zigbuild` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04` | Static, built with `cargo-zigbuild` |
| `aarch64-apple-darwin` | `macos-15` | Apple silicon |
| `x86_64-apple-darwin` | `macos-15` | Intel, cross-compiled by the Apple toolchain |

Windows and 32-bit ARM are not built. The store and the session filesystem
assume a Unix host, and the engine's `io-uring` dependency has no 32-bit ARM
bindings.

Each job installs the pinned toolchain, and Zig with `cargo-zigbuild` for the
Linux targets, then hands the build, the archive, the checksum and the upload to
[`taiki-e/upload-rust-binary-action`](https://github.com/taiki-e/upload-rust-binary-action).
The workflow builds the source at the tag. To attach binaries to a release by
hand, dispatch it with that tag:

```sh
gh workflow run binaries.yml -f tag=agent-hub-v1.2.0
```

The `targets` input takes a JSON array of `{"target", "runner", "build-tool"}`
objects when one archive needs rebuilding on its own. A re-run replaces the
assets it uploaded.

## The plugins

`release-please` bumps the plugin versions with `Cargo.toml`, through
`extra-files` in `release-please-config.json`: the Claude manifest and
marketplace entry, `plugins/opencode/package.json` and
`plugins/antigravity/plugin.json`. `make plugins/check` fails when they drift
from `Cargo.toml`, and CI runs it.

| Plugin | What a release does |
|---|---|
| Claude Code | Nothing. The repository is the marketplace, and `/plugin marketplace add agync/agent-hub` reads `.claude-plugin/marketplace.json` from the default branch, which carries the new version once the release pull request merges. |
| OpenCode | The `opencode` job publishes `@agync/opencode-agent-hub` from `plugins/opencode` with `npm publish --access public --provenance`. It does nothing until publishing is configured. |
| Antigravity | The `antigravity` job runs `make plugins/antigravity-dist` and attaches `agent-hub-antigravity-<version>.tar.gz` and its `.sha256` to the release. |

To attach the Antigravity archive to a release by hand:

```sh
make plugins/antigravity-dist
gh release upload "agent-hub-v<VERSION>" \
  "target/dist/agent-hub-antigravity-<VERSION>.tar.gz"
```

### One-time setup

Nothing below is done by the workflow. Until it is, a release skips the npm
publish and the plugins are listed nowhere but this repository.

1. **npm.** Create the `@agync` organisation on npmjs.com. Then either add an
   automation token as the repository secret `NPM_TOKEN`, or configure npm
   trusted publishing for the package (repository `agync/agent-hub`, workflow
   `release.yml`) and set the repository variable `PUBLISH_NPM` to `true`. The
   first publish creates the package. Trusted publishing needs npm 11.5.1 or later,
   so the `opencode` job runs on Node 24.
2. **Antigravity marketplace.** Listing is a manual step: submit the release
   archive through the Antigravity marketplace intake form. Repeat it for a
   release that should replace the listed version.
3. **Claude directory.** The marketplace works without a listing. To submit the
   plugin to the Claude plugin directory, prepare the icon and a privacy policy
   URL the form asks for, then submit through its form.

## Verify the release

Nothing is done until the published artifact has answered. The workflow does
this; to repeat it by hand:

```sh
VERSION=1.0.0
podman run --detach --name agent-hub-verify \
  --publish 18080:8080 \
  --env HUB_ADMIN_TOKEN=release-check \
  --volume agent-hub-verify-data:/data \
  "ghcr.io/agync/agent-hub:$VERSION"
for attempt in $(seq 30); do
  podman exec agent-hub-verify \
    /usr/local/bin/agent-hub health --url http://127.0.0.1:8080 && break
  sleep 1
done
curl -sS http://127.0.0.1:18080/readyz
curl -sS -H 'Authorization: Bearer release-check' \
  http://127.0.0.1:18080/api/v1/storage | grep -o '"version":"[^"]*","commit":"[^"]*"'
podman rm --force agent-hub-verify
podman volume rm agent-hub-verify-data
```

The runtime is distroless: no shell and no curl. The probe is the binary's own
`health` subcommand against `/readyz`, polled rather than slept, because an
empty data directory takes a moment to migrate.

## The first release, v1.0.0

`release-please` is seeded at `1.0.0` in `.release-please-manifest.json`, the
version `Cargo.toml` and `CHANGELOG.md` already carry. The bootstrap is one tag:
create `v1.0.0` at the release commit once, so `release-please` has a
last-release point to measure the next version from:

```sh
git switch main
git tag -a v1.0.0 -m "Agent Hub 1.0.0"
git tag -n99 -l v1.0.0       # read the message back
git push origin v1.0.0
```

From then on `release-please` owns every version. To build the v1.0.0 image
without a release-please run, dispatch the release workflow by hand and it
builds and verifies the current version.

## By hand

The workflow is the whole release. When it cannot run, build and push the image
with the same tools, create the release, and dispatch the binaries workflow for
its tag:

```sh
VERSION=1.0.0
printf '%s' "$(gh auth token)" | podman login ghcr.io \
  --username "$(gh api user --jq .login)" --password-stdin
podman build -f Containerfile \
  --build-arg GIT_COMMIT="$(git rev-parse --short HEAD)" \
  --tag "ghcr.io/agync/agent-hub:$VERSION" \
  --tag ghcr.io/agync/agent-hub:latest .
podman push "ghcr.io/agync/agent-hub:$VERSION"
podman push ghcr.io/agync/agent-hub:latest
```

Then verify as above.

## Roll back

### The image

An image is immutable, so a rollback is a redeploy of the previous tag. Pull it
and replace the running container; [deploy the hub as a service](docs/operate/deploy-as-a-service.md)
covers the restart for both Docker and Podman. Roll the `latest` tag back too,
or the next pull re-breaks it.

### The release

```sh
gh release delete "v<VERSION>"
git push origin --delete "v<VERSION>"
```

A consumed tag is not reusable. Release the fix as a new patch version, which is
the normal path; `release-please` proposes it from the fix commits.

### The store

The store schema migrates forward on startup, one version per transaction, and a
binary refuses to open a store whose schema is newer than the most it supports.
So rolling the binary back is a data operation, not a file copy, and the
[operations runbook](docs/operate/upgrade-and-rollback.md) owns it: stop the hub, copy the
matching `backups/pre-migration-v<from>-<stamp>.db` from the data directory's
`backups/` over `hub.db`, copy the `-wal` and `-shm` sidecars with it, remove
any the backup did not carry, and start the older binary. Take an offline backup
first, while no hub holds the store:

```sh
agent-hub backup --out /path/to/pre-rollback-backup
agent-hub check --data-dir /path/to/data
```
