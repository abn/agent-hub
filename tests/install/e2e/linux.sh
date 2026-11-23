#!/bin/sh
# The Linux install and service e2e. A rootless podman container runs systemd as
# PID 1; inside it, root (the container's, never the host's) makes a user and
# turns linger on, and that user then runs the real install.sh against a local
# release made from the binary under test. No host service manager is touched
# and nothing needs sudo, which a CI runner and a developer machine both allow.
#
#   linux.sh [PATH-TO-agent-hub]     (default: target/release/agent-hub, built if absent)
#
# Files live under target/tmp. The container is removed on exit.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
bin=${1:-$repo/target/release/agent-hub}
image=localhost/agent-hub-install-e2e:systemd
name=agent-hub-install-e2e-$$

command -v podman >/dev/null || { echo "install/e2e needs podman" >&2; exit 1; }
if [ ! -x "$bin" ]; then
  [ "$bin" = "$repo/target/release/agent-hub" ] || { echo "no binary at $bin" >&2; exit 1; }
  (cd "$repo" && cargo build --release --locked)
fi

mkdir -p "$repo/target/tmp"
work=$(mktemp -d "$repo/target/tmp/install-e2e.XXXXXX")
cleanup() {
  podman rm --force --volumes "$name" >/dev/null 2>&1 || true
  rm -rf "${work:?}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

version=$(sh "$here/release-dir.sh" "$bin" "$work/release")
podman build --quiet -t "$image" -f "$here/Containerfile.systemd" "$here" >/dev/null

# --systemd=always makes podman mount what systemd needs; the container is
# unprivileged and its root maps to the invoking user.
podman run --detach --name "$name" --systemd=always \
  --security-opt label=disable \
  --volume "$work/release:/release:ro" \
  --volume "$here/linux-inner.sh:/e2e/linux-inner.sh:ro" \
  --volume "$repo/deploy/install.sh:/e2e/deploy/install.sh:ro" \
  "$image" >/dev/null

in_box() { podman exec "$name" "$@"; }

i=0
until in_box systemctl is-system-running 2>/dev/null | grep -Eq 'running|degraded'; do
  i=$((i + 1))
  [ $i -le 60 ] || { echo "systemd did not come up in the container" >&2; podman logs "$name" | tail -20 >&2; exit 1; }
  sleep 1
done

in_box useradd --create-home hubuser
# Root turns linger on, which also starts the user's systemd manager.
in_box loginctl enable-linger hubuser
uid=$(in_box id -u hubuser)
i=0
until in_box test -S "/run/user/$uid/systemd/private"; do
  i=$((i + 1))
  [ $i -le 30 ] || { echo "the user manager did not start" >&2; exit 1; }
  sleep 1
done

podman exec --detach "$name" python3 -m http.server 8099 --bind 127.0.0.1 --directory /release
i=0
until podman exec "$name" curl -fsS "http://127.0.0.1:8099/agent-hub-v$version/" >/dev/null 2>&1; do
  i=$((i + 1))
  [ $i -le 60 ] || { echo "the release server did not answer in the container" >&2; exit 1; }
  sleep 0.5
done

# The user arrives as an ssh login would: no session bus variables set.
podman exec --user hubuser "$name" sh /e2e/linux-inner.sh "$version"
