#!/bin/sh
# shellcheck disable=SC2015,SC2016  # A && B || C and the inner sh -c strings are deliberate
# The macOS install and service e2e, for a CI runner: the real install.sh
# against a local release made from the binary under test, then the lifecycle
# through a real LaunchAgent. It installs into the runner's own home under the
# production names, so it refuses to start where an agent-hub is already
# installed, loaded or answering, and outside CI unless
# AGENT_HUB_E2E_DISPOSABLE=1 says the profile is disposable. On any exit it
# boots the job out and removes what it installed.
#
#   macos.sh [PATH-TO-agent-hub]     (default: target/release/agent-hub)
set -u

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
bin_under_test=${1:-$repo/target/release/agent-hub}
[ "$(uname -s)" = Darwin ] || { echo "macos.sh runs on macOS" >&2; exit 1; }

script=$repo/deploy/install.sh
bin=$HOME/.local/bin/agent-hub
label=dev.agync.agent-hub
plist=$HOME/Library/LaunchAgents/$label.plist
datadir="$HOME/Library/Application Support/agent-hub"
uid=$(id -u)
# The installer uses gui/<uid> when a GUI session exists and user/<uid> when
# not. Resolve it once and use it for every probe.
if launchctl print "gui/$uid" >/dev/null 2>&1; then domain=gui/$uid; else domain=user/$uid; fi

if [ "${CI:-}" != true ] && [ "${AGENT_HUB_E2E_DISPOSABLE:-}" != 1 ]; then
  echo "macos.sh installs and purges agent-hub in this profile: run it in CI, or set AGENT_HUB_E2E_DISPOSABLE=1 on a disposable machine" >&2
  exit 1
fi
if [ -e "$bin" ] || [ -e "$plist" ] || [ -e "$datadir" ] ||
  launchctl print "$domain/$label" >/dev/null 2>&1 ||
  curl -fsS --max-time 2 http://127.0.0.1:8080/readyz >/dev/null 2>&1; then
  echo "an agent-hub install, job, data directory or running hub already exists here; refusing to touch it" >&2
  exit 1
fi

mkdir -p "$repo/target/tmp"
work=$(mktemp -d "$repo/target/tmp/install-e2e.XXXXXX") || exit 1
server_pid=
cleanup() {
  launchctl bootout "$domain/$label" >/dev/null 2>&1
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null
  rm -f "$plist" "$bin"
  rm -rf "$datadir" "${work:?}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

version=$(sh "$here/release-dir.sh" "$bin_under_test" "$work/release") || exit 1
python3 -I -m http.server 8099 --bind 127.0.0.1 --directory "$work/release" >/dev/null 2>&1 &
server_pid=$!
i=0
until curl -fsS "http://127.0.0.1:8099/agent-hub-v$version/" >/dev/null 2>&1; do
  i=$((i + 1))
  [ $i -le 60 ] || { echo "the release server did not answer on 127.0.0.1:8099" >&2; exit 1; }
  sleep 0.5
done
# Downloads, latest-version discovery and provenance stay on this machine: the
# gh that accepts only the exact attestation call stands in for the real one.
export AGENT_HUB_DOWNLOAD_BASE=http://127.0.0.1:8099
export AGENT_HUB_LATEST_URL=http://127.0.0.1:8099/releases/latest
mkdir -p "$work/stubs"
cat >"$work/stubs/gh" <<'STUB'
#!/bin/sh
[ "$1 $2" = "attestation verify" ] || exit 64
[ -f "$3" ] || exit 65
[ "$4 $5" = "--repo agync/agent-hub" ] || exit 67
exit 0
STUB
chmod 755 "$work/stubs/gh"
PATH=$work/stubs:$PATH
export PATH
fails=0

pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1"
  fails=$((fails + 1))
}
check() {
  name=$1
  shift
  if "$@" >/dev/null 2>&1; then pass "$name"; else fail "$name"; fi
}
expect_out() {
  name=$1
  pat=$2
  shift 2
  out=$("$@" 2>&1)
  rc=$?
  if [ $rc -eq 0 ] && printf '%s\n' "$out" | grep -q -- "$pat"; then
    pass "$name"
  else
    fail "$name (rc $rc): $out"
  fi
}
ready() { curl -fsS --max-time 3 http://127.0.0.1:8080/readyz; }
wait_ready() {
  i=0
  while [ $i -lt 60 ]; do
    ready >/dev/null 2>&1 && return 0
    sleep 0.5
    i=$((i + 1))
  done
  return 1
}
wait_down() {
  i=0
  while [ $i -lt 60 ]; do
    ready >/dev/null 2>&1 || return 0
    sleep 0.5
    i=$((i + 1))
  done
  return 1
}
job_pid() { launchctl print "$domain/$label" 2>/dev/null | sed -n 's/^[[:space:]]*pid = \([0-9]*\)$/\1/p'; }

echo "domain: $domain"

# 1. A first install writes a valid plist, loads it and answers.
sh "$script" --version "v$version" --yes || fail "install.sh exit $?"
check "the binary is placed" test -x "$bin"
check "the plist is written" test -f "$plist"
check "plutil accepts the plist" plutil -lint "$plist"
check "the plist says launchd manages it" grep -q AGENT_HUB_MANAGED_BY "$plist"
check "the job is loaded" launchctl print "$domain/$label"
check "the hub answers /readyz" wait_ready
check "status says running" "$bin" status
pid1=$(job_pid)

# 2. The same version is a no-op; --force reinstalls over a running hub.
expect_out "the same version is a no-op" "already installed" \
  sh "$script" --version "v$version" --yes
sh "$script" --version "v$version" --yes --force || fail "forced reinstall exit $?"
check "the hub answers after a forced reinstall" wait_ready

# 3. stop checkpoints; start and restart bring it back.
expect_out "stop checkpoints the store" "store checkpointed" "$bin" stop
check "the hub is down after stop" wait_down
"$bin" status >/dev/null 2>&1
[ $? -eq 3 ] && pass "status exits 3 when stopped" || fail "status exit after stop"
check "start brings the job back" "$bin" start
check "the hub answers after start" wait_ready
pid3=$(job_pid)
check "restart succeeds" "$bin" restart
check "the hub answers after restart" wait_ready
pid4=$(job_pid)
[ -n "$pid4" ] && [ "$pid4" != "$pid3" ] && pass "restart is a new process" ||
  fail "restart kept pid '$pid3' (first install was '$pid1')"

# 4. A crash is restarted by launchd.
kill -9 "$pid4" 2>/dev/null
i=0
back=0
while [ $i -lt 60 ]; do
  p=$(job_pid)
  if [ -n "$p" ] && [ "$p" != "$pid4" ] && ready >/dev/null 2>&1; then
    back=1
    break
  fi
  sleep 0.5
  i=$((i + 1))
done
[ $back -eq 1 ] && pass "the hub is back after kill -9" || fail "the hub did not come back after kill -9"

# 5. Boot-time start is refused on macOS, and says why.
"$bin" service install --boot --yes >/dev/null 2>&1
[ $? -eq 2 ] && pass "service install --boot is refused with exit 2" || fail "service install --boot did not exit 2"

# 6. Uninstall with purge leaves nothing behind.
sh "$script" --uninstall --purge --yes || fail "uninstall exit $?"
check "the plist is gone" test ! -e "$plist"
check "the binary is gone" test ! -e "$bin"
not_loaded() { ! launchctl print "$domain/$label" >/dev/null 2>&1; }
check "the job is not loaded" not_loaded
check "the hub is down" wait_down
check "the data directory is gone" test ! -e "$datadir/hub.db"

printf '\n%s failed\n' "$fails"
[ "$fails" -eq 0 ]
