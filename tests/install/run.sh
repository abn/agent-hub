#!/bin/sh
# Behaviour tests for deploy/install.sh against a local release server and a
# stub agent-hub binary. Everything lives under target/tmp; HOME and the XDG
# directories point into it, and no real service manager or hub is touched.
#
#   sh tests/install/run.sh

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
script="$repo/deploy/install.sh"
mkdir -p "$repo/target/tmp"
root=$(mktemp -d "$repo/target/tmp/install-test.XXXXXX") || exit 1
server_pid=

cleanup() {
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null
  rm -rf "${root:?}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

fails=0
passes=0

# --- release fixtures --------------------------------------------------------

case "$(uname -s)" in
  Linux) os_part=unknown-linux-musl ;;
  Darwin) os_part=apple-darwin ;;
  *)
    echo "unsupported host for this harness"
    exit 1
    ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch_part=x86_64 ;;
  aarch64 | arm64) arch_part=aarch64 ;;
  *)
    echo "unsupported host arch for this harness"
    exit 1
    ;;
esac
# An x86_64 shell under Rosetta: the installer asks for the native build.
if [ "$os_part" = apple-darwin ] && [ "$arch_part" = x86_64 ] &&
  [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
  arch_part=aarch64
fi
target="$arch_part-$os_part"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# make_release VERSION [TARGET [KIND]]  (VERSION without the v)
# KIND old: a release before the native lifecycle, which knows no --version and
# no service command. KIND lying: reports 0.0.1 whatever it is called.
make_release() {
  v=$1
  tgt=${2:-$target}
  kind=${3:-normal}
  reported=$v
  [ "$kind" != lying ] || reported=0.0.1
  name="agent-hub-v$v-$tgt"
  rel="$root/download/agent-hub-v$v"
  stage="$root/stage/$name"
  mkdir -p "$rel" "$stage"
  if [ "$kind" = old ]; then
    cat >"$stage/agent-hub" <<STUB
#!/bin/sh
# stub agent-hub $v, before the native lifecycle
printf '%s\n' "\$*" >>"\${STUB_LOG:-/dev/null}"
echo "agent-hub: unknown subcommand '\$1'" >&2
exit 2
STUB
  else
    cat >"$stage/agent-hub" <<STUB
#!/bin/sh
# stub agent-hub $v
printf '%s\n' "\$*" >>"\${STUB_LOG:-/dev/null}"
case "\$1" in
  --version) echo "agent-hub $reported (stub)"; exit 0 ;;
  serve) exec sleep 60 ;;
  service) [ "\$2" = --help ] && exit 0; exit "\${STUB_SERVICE_RC:-0}" ;;
  status) exit "\${STUB_STATUS_RC:-3}" ;;
  *) exit 0 ;;
esac
STUB
  fi
  chmod 755 "$stage/agent-hub"
  tar -czf "$rel/$name.tar.gz" -C "$root/stage" "$name"
  printf '%s  %s.tar.gz\n' "$(sha256_of "$rel/$name.tar.gz")" "$name" >"$rel/$name.sha256"
}

make_release 1.0.0
make_release 1.0.0 aarch64-apple-darwin
make_release 1.1.0
make_release 1.1.0 aarch64-apple-darwin
make_release 1.1.0 x86_64-apple-darwin
make_release 0.9.0 "$target" old
make_release 1.2.0 "$target" lying
printf 'v1.1.0\n' >"$root/LATEST"

python3 "$here/server.py" "$root" "$root/port" &
server_pid=$!
i=0
while [ ! -s "$root/port" ] && [ $i -lt 50 ]; do
  sleep 0.1
  i=$((i + 1))
done
[ -s "$root/port" ] || {
  echo "release server did not start"
  exit 1
}
port=$(cat "$root/port")

# --- case plumbing ------------------------------------------------------------

# new_case NAME: a fresh HOME and a dir of stubs ahead of PATH.
new_case() {
  case_name=$1
  case_dir="$root/case-$1"
  mkdir -p "$case_dir/home" "$case_dir/stubs"
  : >"$case_dir/log"
  : >"$case_dir/out"
  : >"$case_dir/err"
  : >"$root/requests.log"
  case_path="$case_dir/stubs:$PATH"
  # A gh that never touches the network and accepts only the exact call the
  # installer should make: attestation verify <archive> --repo agync/agent-hub.
  cat >"$case_dir/stubs/gh" <<STUB
#!/bin/sh
printf '%s\\n' "\$*" >>"$case_dir/gh.log"
[ "\${GH_RC:-0}" = 0 ] || exit "\$GH_RC"
[ "\$1 \$2" = "attestation verify" ] || exit 64
[ -f "\$3" ] || exit 65
case "\$3" in */agent-hub-v*.tar.gz) ;; *) exit 66 ;; esac
[ "\$4 \$5 \$#" = "--repo agync/agent-hub 5" ] || exit 67
exit 0
STUB
  chmod 755 "$case_dir/stubs/gh"
}

# run_install ARGS...: runs the script in the sandbox environment, sets rc.
run_install() {
  env -i \
    PATH="$case_path" \
    HOME="$case_dir/home" \
    XDG_CACHE_HOME="$case_dir/home/.cache" \
    XDG_STATE_HOME="$case_dir/home/.local/state" \
    XDG_DATA_HOME="$case_dir/home/.local/share" \
    XDG_CONFIG_HOME="$case_dir/home/.config" \
    SHELL="${T_SHELL:-/bin/sh}" \
    AGENT_HUB_NO_SERVICE="${AGENT_HUB_NO_SERVICE:-}" \
    STUB_LOG="$case_dir/log" \
    STUB_SERVICE_RC="${STUB_SERVICE_RC:-0}" \
    STUB_STATUS_RC="${STUB_STATUS_RC:-3}" \
    GH_RC="${GH_RC:-0}" \
    AGENT_HUB_DOWNLOAD_BASE="${T_DL_BASE:-http://127.0.0.1:$port/download}" \
    AGENT_HUB_LATEST_URL="${T_LATEST:-http://127.0.0.1:$port/releases/latest}" \
    http_proxy=http://127.0.0.1:9 https_proxy=http://127.0.0.1:9 \
    HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 \
    NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 \
    "${INSTALL_SH:-sh}" "$script" "$@" >"$case_dir/out" 2>"$case_dir/err"
  rc=$?
}

# run_piped FILE ARGS...: the way `curl | sh -s -- ARGS` runs it, script on stdin.
run_piped() {
  piped=$1
  shift
  env -i \
    PATH="$case_path" \
    HOME="$case_dir/home" \
    XDG_CACHE_HOME="$case_dir/home/.cache" \
    XDG_STATE_HOME="$case_dir/home/.local/state" \
    SHELL=/bin/sh \
    STUB_LOG="$case_dir/log" \
    AGENT_HUB_DOWNLOAD_BASE="http://127.0.0.1:$port/download" \
    AGENT_HUB_LATEST_URL="http://127.0.0.1:$port/releases/latest" \
    http_proxy=http://127.0.0.1:9 https_proxy=http://127.0.0.1:9 \
    NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 \
    "${INSTALL_SH:-sh}" -s -- "$@" <"$piped" >"$case_dir/out" 2>"$case_dir/err"
  rc=$?
}

# check DESCRIPTION COMMAND...
check() {
  desc=$1
  shift
  if "$@"; then
    passes=$((passes + 1))
  else
    fails=$((fails + 1))
    printf 'FAIL [%s] %s (rc=%s)\n' "$case_name" "$desc" "$rc"
    sed 's/^/  out: /' "$case_dir/out"
    sed 's/^/  err: /' "$case_dir/err"
  fi
}

rc_is() { [ "$rc" = "$1" ]; }
file_has() { grep -q -- "$2" "$1"; }
file_lacks() { ! grep -q -- "$2" "$1"; }
is_exec() { [ -x "$1" ]; }
is_absent() { [ ! -e "$1" ]; }
is_empty() { [ ! -s "$1" ]; }
installed_is() { [ "$("$case_dir/home/.local/bin/agent-hub" --version)" = "agent-hub $1 (stub)" ]; }
prev_is() { [ "$("$case_dir/home/.local/state/agent-hub/agent-hub.prev" --version)" = "agent-hub $1 (stub)" ]; }
requests_are() {
  printf '%s\n' "$@" >"$case_dir/expected-requests"
  # Order of the two downloads is fixed; the log is the server's own record.
  diff "$case_dir/expected-requests" "$root/requests.log" >/dev/null
}
cache_clean() { [ -z "$(ls "$case_dir/home/.cache/agent-hub")" ]; }

is_alive() { kill -0 "$1" 2>/dev/null; }
rc_not0() { [ "$rc" != 0 ]; }
same_file() { cmp -s "$1" "$2"; }
is_dir() { [ -d "$1" ]; }
no_leftovers() {
  for f in "$1"/.agent-hub.*; do
    [ ! -e "$f" ] || return 1
  done
}

# make_tools DIR: a PATH holding only the tools the installer may use, so a
# case can leave out curl or gh without depending on what the host has.
make_tools() {
  mkdir -p "$1"
  for t in "${INSTALL_SH:-sh}" uname id mkdir mktemp cat cut tr sed tar gzip sha256sum shasum \
    cp mv rm chmod cmp sleep dirname diff ls head; do
    p=$(command -v "$t" 2>/dev/null) || continue
    case "$p" in /*) ln -sf "$p" "$1/$t" ;; esac
  done
}

# --- cases --------------------------------------------------------------------

new_case fresh
run_install --yes
check "exit 0" rc_is 0
check "binary placed in ~/.local/bin" is_exec "$case_dir/home/.local/bin/agent-hub"
check "latest release chosen" installed_is 1.1.0
check "service install handed over with --yes" file_has "$case_dir/log" '^service install --yes$'
check "no previous binary yet" is_absent "$case_dir/home/.local/state/agent-hub/agent-hub.prev"
check "PATH advice printed" file_has "$case_dir/out" 'export PATH='
check "checksum reported" file_has "$case_dir/out" 'checksum: ok'
check "staging left no residue in the cache" cache_clean
check "exactly the expected local requests were made" requests_are \
  "HEAD /releases/latest" \
  "GET /download/agent-hub-v1.1.0/agent-hub-v1.1.0-$target.tar.gz" \
  "GET /download/agent-hub-v1.1.0/agent-hub-v1.1.0-$target.sha256"
check "gh verified the archive of this release in this repo" file_has "$case_dir/gh.log" \
  "^attestation verify .*/agent-hub-v1.1.0-$target.tar.gz --repo agync/agent-hub\$"

new_case noop
run_install --yes
: >"$case_dir/log"
run_install --yes
check "exit 0" rc_is 0
check "says already installed" file_has "$case_dir/out" 'already installed'
check "service install runs again, it is idempotent" file_has "$case_dir/log" '^service install --yes$'
: >"$case_dir/log"
run_install --yes --bind 127.0.0.1:8081 --login
check "a same-version re-run carries the service flags" file_has "$case_dir/log" \
  '^service install --yes --bind 127.0.0.1:8081 --login$'
: >"$case_dir/log"
run_install --yes --no-service
check "--no-service on a same-version re-run touches no service" file_lacks "$case_dir/log" 'service'
check "and still exits 0" rc_is 0
STUB_SERVICE_RC=1 run_install --yes
check "a same-version re-run still reports a service failure" rc_is 1
check "and names the retry" file_has "$case_dir/err" 'run: agent-hub service install'
run_install --yes --force
check "force reinstalls" file_has "$case_dir/out" 'Upgraded agent-hub 1.1.0 to 1.1.0'

new_case upgrade
run_install --yes --version v1.0.0
check "pinned version installed" installed_is 1.0.0
STUB_STATUS_RC=0 run_install --yes
check "exit 0" rc_is 0
check "upgraded to latest" installed_is 1.1.0
check "previous binary kept" is_exec "$case_dir/home/.local/state/agent-hub/agent-hub.prev"
check "previous binary is the old version" prev_is 1.0.0
check "running hub restarted" file_has "$case_dir/log" '^restart$'

new_case old-release
run_install --yes --version v0.9.0
check "a release without service commands is refused" rc_is 3
check "names the way out" file_has "$case_dir/err" 'install a newer release, or pass --no-service'
check "nothing was placed" is_absent "$case_dir/home/.local/bin/agent-hub"

new_case old-release-nosvc
run_install --yes --no-service --version v0.9.0
check "an older release may be placed without a service" rc_is 0
check "binary placed" is_exec "$case_dir/home/.local/bin/agent-hub"
check "service never touched" file_lacks "$case_dir/log" 'service'

new_case wrong-version
run_install --yes --version v1.2.0
check "a binary that is not the asked release is refused" rc_is 1
check "says what it reports" file_has "$case_dir/err" 'reports 0.0.1, not agent-hub 1.2.0'
check "nothing was placed" is_absent "$case_dir/home/.local/bin/agent-hub"
run_install --yes --no-service --version v1.2.0
check "refused without a service too" rc_is 1

new_case badsum
sumfile="$root/download/agent-hub-v1.1.0/agent-hub-v1.1.0-$target.sha256"
cp "$sumfile" "$case_dir/good.sha256"
printf '%064d  x\n' 0 >"$sumfile"
run_install --yes
cp "$case_dir/good.sha256" "$sumfile"
check "exit 1" rc_is 1
check "says mismatch" file_has "$case_dir/err" 'checksum mismatch'
check "nothing installed" is_absent "$case_dir/home/.local/bin/agent-hub"
check "service install never ran" is_empty "$case_dir/log"

new_case arch
cat >"$case_dir/stubs/uname" <<'STUB'
#!/bin/sh
case "$1" in
  -s) echo Linux ;;
  -m) echo armv7l ;;
esac
STUB
chmod 755 "$case_dir/stubs/uname"
run_install --yes
check "exit 3" rc_is 3
check "names the architecture" file_has "$case_dir/err" 'armv7l'
check "nothing downloaded" is_empty "$root/requests.log"

new_case freebsd
cat >"$case_dir/stubs/uname" <<'STUB'
#!/bin/sh
case "$1" in
  -s) echo FreeBSD ;;
  -m) echo amd64 ;;
esac
STUB
chmod 755 "$case_dir/stubs/uname"
run_install --yes
check "exit 3" rc_is 3

new_case noservice
run_install --yes --no-service
check "exit 0" rc_is 0
check "binary placed" is_exec "$case_dir/home/.local/bin/agent-hub"
check "service install never invoked" file_lacks "$case_dir/log" 'service'

new_case noservice-env
AGENT_HUB_NO_SERVICE=1 run_install --yes
check "env var also skips the service" file_lacks "$case_dir/log" 'service'

new_case bindir
run_install --yes --bin-dir "$case_dir/custom"
check "exit 0" rc_is 0
check "binary in the override dir" is_exec "$case_dir/custom/agent-hub"
check "default dir untouched" is_absent "$case_dir/home/.local/bin"
check "service install ran from the override" file_has "$case_dir/log" '^service install'

new_case flags
run_install --yes --bind 0.0.0.0:9000 --login
check "bind and login passed through" file_has "$case_dir/log" '^service install --yes --bind 0.0.0.0:9000 --login$'

new_case nomanager
STUB_SERVICE_RC=69 run_install --yes
check "no service manager is not a failure" rc_is 0
check "says so" file_has "$case_dir/out" 'without a service'

new_case svcfail
STUB_SERVICE_RC=1 run_install --yes
check "service failure exits 1" rc_is 1

new_case provenance-ok
run_install --yes --require-provenance
check "verified with gh" file_has "$case_dir/out" 'provenance: verified'

new_case provenance-fail
GH_RC=1 run_install --yes --require-provenance
check "required and failing exits 1" rc_is 1
check "nothing installed" is_absent "$case_dir/home/.local/bin/agent-hub"
GH_RC=1 run_install --yes
check "optional and failing only warns" rc_is 0
check "warning shown" file_has "$case_dir/err" 'provenance: not verified'

new_case uninstall
run_install --yes --no-service
: >"$case_dir/log"
run_install --uninstall --purge --yes
check "exit 0" rc_is 0
check "delegated with the flags" file_has "$case_dir/log" '^uninstall --purge --yes$'

new_case usage
run_install --nope
check "unknown flag exits 2" rc_is 2
run_install --purge
check "--purge alone exits 2" rc_is 2

new_case missing-release
printf 'v9.9.9\n' >"$root/LATEST"
run_install --yes
printf 'v1.1.0\n' >"$root/LATEST"
check "missing release exits 1" rc_is 1


# --- piped, truncated -------------------------------------------------------

new_case piped
run_piped "$script" --no-service --yes
check "a whole script on stdin honours its flags" rc_is 0
check "binary placed" is_exec "$case_dir/home/.local/bin/agent-hub"
check "--no-service survived the pipe" file_lacks "$case_dir/log" 'service'

new_case truncated
last=$(tail -n 1 "$script")
check "the script ends in one compound command" [ "$last" = '{ main "$@"; exit $?; }' ]
sed '$d' "$script" >"$case_dir/body.sh"
len=$(printf '%s' "$last" | wc -c)
bad=0
cut_at=1
while [ "$cut_at" -lt "$len" ]; do
  { cat "$case_dir/body.sh"; printf '%s' "$last" | cut -c "1-$cut_at" | tr -d '\n'; } >"$case_dir/cut.sh"
  run_piped "$case_dir/cut.sh" --no-service --require-provenance
  if [ "$rc" = 0 ] || [ -e "$case_dir/home/.local/bin/agent-hub" ] || [ -s "$case_dir/out" ]; then
    bad=$((bad + 1))
    echo "  cut of the last line at $cut_at bytes ran or exited 0 (rc=$rc)"
  fi
  cut_at=$((cut_at + 1))
done
check "no cut of the last line runs anything" [ "$bad" = 0 ]
total=$(wc -l <"$case_dir/body.sh")
for keep in 5 40 80 120 160 200; do
  [ "$keep" -lt "$total" ] || continue
  head -n "$keep" "$case_dir/body.sh" >"$case_dir/cut.sh"
  run_piped "$case_dir/cut.sh" --no-service
  [ ! -e "$case_dir/home/.local/bin/agent-hub" ] || bad=$((bad + 1))
done
check "no cut inside the functions runs anything" [ "$bad" = 0 ]
check "nothing was downloaded" is_empty "$root/requests.log"

# --- upgrade: running hub, failure, lock ------------------------------------

new_case upgrade-live
run_install --yes --version v1.0.0 --no-service
STUB_LOG="$case_dir/live.log" "$case_dir/home/.local/bin/agent-hub" serve &
live_pid=$!
STUB_STATUS_RC=0 run_install --yes
kill "$live_pid" 2>/dev/null
check "upgrade over a live binary succeeds" rc_is 0
check "installed the new version" installed_is 1.1.0
check "previous binary is the old version" prev_is 1.0.0
check "service reinstalled and hub restarted" file_has "$case_dir/log" '^restart$'
check "no staging files left in the bin dir" no_leftovers "$case_dir/home/.local/bin"
check "lock released" is_absent "$case_dir/home/.local/state/agent-hub/install.lock"

new_case upgrade-notready
run_install --yes --version v1.0.0 --no-service
STUB_STATUS_RC=1 run_install --yes
check "running but not ready counts as running: restarted" file_has "$case_dir/log" '^restart$'

new_case upgrade-stopped
run_install --yes --version v1.0.0 --no-service
STUB_STATUS_RC=3 run_install --yes
check "stopped hub is not restarted" file_lacks "$case_dir/log" '^restart$'

new_case upgrade-nosvc-running
run_install --yes --version v1.0.0 --no-service
STUB_STATUS_RC=0 run_install --yes --no-service
check "binary-only upgrade leaves the hub alone" file_lacks "$case_dir/log" '^restart$'
check "says how to switch" file_has "$case_dir/out" 'agent-hub restart'

new_case backup-fails
run_install --yes --version v1.0.0 --no-service
cat >"$case_dir/stubs/cp" <<STUB
#!/bin/sh
case "\$*" in *prev*) exit 1 ;; esac
exec $(command -v cp) "\$@"
STUB
chmod 755 "$case_dir/stubs/cp"
run_install --yes
check "no backup, no upgrade: exit 1" rc_is 1
check "says why" file_has "$case_dir/err" 'previous binary'
check "the installed binary is untouched" installed_is 1.0.0
check "no staging files left in the bin dir" no_leftovers "$case_dir/home/.local/bin"
check "lock released after the failure" is_absent "$case_dir/home/.local/state/agent-hub/install.lock"

new_case bindir-readonly
run_install --yes --version v1.0.0 --no-service
if [ "$(id -u)" != 0 ]; then
  chmod 555 "$case_dir/home/.local/bin"
  run_install --yes
  chmod 755 "$case_dir/home/.local/bin"
  check "a bin dir that cannot be written fails with exit 1" rc_is 1
  check "the installed binary still runs the old version" installed_is 1.0.0
fi

new_case stale-symlink
mkdir -p "$case_dir/home/.local/bin"
printf 'keep\n' >"$case_dir/victim"
ln -s "$case_dir/victim" "$case_dir/home/.local/bin/.agent-hub.new"
run_install --yes --no-service
check "install works beside a planted symlink" rc_is 0
check "the symlink target was not written" file_has "$case_dir/victim" '^keep$'

new_case lock
mkdir -p "$case_dir/home/.local/state/agent-hub/install.lock"
sleep 30 &
holder=$!
printf '%s\n' "$holder" >"$case_dir/home/.local/state/agent-hub/install.lock/pid"
run_install --yes --no-service
check "a live holder blocks a second install" rc_is 1
check "says another install runs" file_has "$case_dir/err" 'another install'
check "nothing installed" is_absent "$case_dir/home/.local/bin/agent-hub"
check "the holder's lock is still there" is_dir "$case_dir/home/.local/state/agent-hub/install.lock"
kill "$holder" 2>/dev/null
wait "$holder" 2>/dev/null
run_install --yes --no-service
check "a lock whose owner is gone is taken over" rc_is 0

new_case lock-race
mkdir -p "$case_dir/home/.local/state/agent-hub/install.lock"
dead=$(sh -c 'echo $$')
printf '%s\n' "$dead" >"$case_dir/home/.local/state/agent-hub/install.lock/pid"
race_a=$case_dir/race-a
race_b=$case_dir/race-b
mkdir -p "$race_a" "$race_b"
race_pids=
for who in a b; do
  (
    env -i PATH="$case_path" HOME="$case_dir/home" \
      XDG_CACHE_HOME="$case_dir/home/.cache" XDG_STATE_HOME="$case_dir/home/.local/state" \
      XDG_DATA_HOME="$case_dir/home/.local/share" XDG_CONFIG_HOME="$case_dir/home/.config" \
      SHELL=/bin/sh STUB_LOG="$case_dir/log" \
      AGENT_HUB_DOWNLOAD_BASE="http://127.0.0.1:$port/download" \
      AGENT_HUB_LATEST_URL="http://127.0.0.1:$port/releases/latest" \
      http_proxy=http://127.0.0.1:9 https_proxy=http://127.0.0.1:9 \
      NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 \
      sh "$script" --yes --no-service >"$case_dir/race-$who/out" 2>"$case_dir/race-$who/err"
    echo $? >"$case_dir/race-$who/rc"
  ) &
  race_pids="$race_pids $!"
done
# shellcheck disable=SC2086
wait $race_pids
race_rcs=$(cat "$race_a/rc" "$race_b/rc" | sort | tr '\n' ' ')
rc=0
race_ok() { [ "$race_rcs" = "0 1 " ] || [ "$race_rcs" = "0 0 " ]; }
race_one_install() { [ "$(cat "$race_a/out" "$race_b/out" | grep -c '^Installed agent-hub')" = 1 ]; }
check "two contenders on a dead lock end 0 and 1 (or 0 and 0)" race_ok
check "exactly one of them installed" race_one_install
check "the lock is released at the end" is_absent "$case_dir/home/.local/state/agent-hub/install.lock"
check "the binary is installed" is_exec "$case_dir/home/.local/bin/agent-hub"

# --- provenance and an installed copy ---------------------------------------

new_case prov-installed-nogh
make_tools "$case_dir/tools"
real_curl=$(command -v curl)
ln -sf "$real_curl" "$case_dir/tools/curl"
case_path="$case_dir/tools"
rm -f "$case_dir/stubs/gh"
run_install --yes --no-service
check "plain install without gh works" rc_is 0
run_install --yes --no-service --require-provenance
check "required provenance with no gh does not pass on the version string" rc_is 1
check "says it needs gh" file_has "$case_dir/err" 'needs the gh CLI'

new_case prov-installed-gh
run_install --yes --no-service
: >"$case_dir/gh.log"
run_install --yes --no-service --require-provenance
check "identical bytes verified against the release: exit 0" rc_is 0
check "says it matches the verified release" file_has "$case_dir/out" 'matches the verified release'
check "gh checked the archive" file_has "$case_dir/gh.log" '^attestation verify '
printf '# tampered\n' >>"$case_dir/home/.local/bin/agent-hub"
run_install --yes --no-service --require-provenance
check "same version string, different bytes: reinstalled" rc_is 0
check "now identical to the release" same_file "$case_dir/home/.local/bin/agent-hub" \
  "$root/stage/agent-hub-v1.1.0-$target/agent-hub"

# --- wget only --------------------------------------------------------------

new_case wget-only
make_tools "$case_dir/tools"
real_curl=$(command -v curl)
cat >"$case_dir/tools/wget" <<STUB
#!/bin/sh
printf '%s\n' "\$*" >>"$case_dir/wget.log"
dest=
while [ \$# -gt 0 ]; do
  case "\$1" in
    -O) dest=\$2; shift ;;
    --) shift; break ;;
  esac
  shift
done
exec $real_curl -fsS -o "\$dest" "\$1"
STUB
chmod 755 "$case_dir/tools/wget"
case_path="$case_dir/tools"
run_install --yes --no-service
check "latest discovery without curl is a clear usage error" rc_is 2
check "names curl and --version" file_has "$case_dir/err" 'needs curl; pass --version'
check "nothing installed" is_absent "$case_dir/home/.local/bin/agent-hub"
run_install --yes --no-service --version v1.0.0
check "a pinned version installs with wget" rc_is 0
check "installed the pinned version" installed_is 1.0.0
check "wget did the downloads" file_has "$case_dir/wget.log" 'agent-hub-v1.0.0'

# --- hostile or malformed input ----------------------------------------------

new_case bad-version
for v in 'v1.0.0/../../x' 'v1.0.0?x=1' 'v1.{a,b}' 'v1.0.0 --output=x' 'v1.0.0%2F' '1.0.0#frag' 'x1.0.0'; do
  run_install --yes --no-service --version "$v"
  check "version $v is a usage error" rc_is 2
done
check "no request was made" is_empty "$root/requests.log"
check "nothing created in HOME" is_absent "$case_dir/home/.local"

new_case bad-override
T_DL_BASE='--output=/tmp/x' run_install --yes --no-service --version v1.0.0
check "a download base that is not a URL is a usage error" rc_is 2
T_LATEST='-oops' run_install --yes --no-service
check "a latest URL that is not a URL is a usage error" rc_is 2

new_case bad-redirect
printf 'v1.1.0/../../evil\n' >"$root/LATEST"
run_install --yes --no-service
printf 'v1.1.0\n' >"$root/LATEST"
check "a redirect naming a path is refused" rc_is 1
check "only the redirect was read" requests_are "HEAD /releases/latest"

new_case nested-redirect
printf 'v1.1.0/agent-hub-v2.0.0\n' >"$root/LATEST"
run_install --yes --no-service
printf 'v1.1.0\n' >"$root/LATEST"
check "a nested redirect path is refused" rc_is 1
check "nothing installed from a nested redirect" is_absent "$case_dir/home/.local/bin/agent-hub"
check "only the nested redirect was read" requests_are "HEAD /releases/latest"

new_case missing-values
run_install --bin-dir --no-service
check "--bin-dir swallowing the next flag is exit 2" rc_is 2
run_install --bind
check "--bind with no value is exit 2" rc_is 2
run_install --version=
check "--version= is exit 2" rc_is 2
run_install --boot --login
check "--boot with --login is exit 2" rc_is 2
check "no side effects" is_absent "$case_dir/home/.local"

# --- paths and quoting -------------------------------------------------------

new_case advice-posix
awkward="$case_dir/my dir/\$x'y"
run_install --yes --no-service --bin-dir "$awkward"
check "installed in the awkward dir" is_exec "$awkward/agent-hub"
adv=$(sed -n 's/^  \(export PATH=.*\)$/\1/p' "$case_dir/out")
got=$(sh -c "PATH=/bin:/usr/bin; $adv; printf '%s' \"\$PATH\"")
check "pasting the advice yields exactly that directory first" [ "$got" = "$awkward:/bin:/usr/bin" ]

new_case advice-fish
awkward="$case_dir/my dir/\$x'y"
T_SHELL=/usr/bin/fish run_install --yes --no-service --bin-dir "$awkward"
check "fish advice is one quoted word" file_has "$case_dir/out" "^  fish_add_path '$case_dir/my dir/\$x\\\\'y'\$"

new_case relative-bindir
cd "$case_dir" || exit 1
run_install --yes --no-service --bin-dir rel/bin/
cd "$here" || exit 1
check "relative dir resolved against the working directory" is_exec "$case_dir/rel/bin/agent-hub"
check "advice names the absolute path" file_has "$case_dir/out" "$case_dir/rel/bin is not on PATH"

# --- Darwin and Rosetta -------------------------------------------------------

darwin_stubs() { # TRANSLATED
  cat >"$case_dir/stubs/uname" <<'STUB'
#!/bin/sh
case "$1" in
  -s) echo Darwin ;;
  -m) echo x86_64 ;;
esac
STUB
  printf '#!/bin/sh\necho %s\n' "$1" >"$case_dir/stubs/sysctl"
  chmod 755 "$case_dir/stubs/uname" "$case_dir/stubs/sysctl"
}

new_case rosetta
mac_home="$case_dir/home"
darwin_stubs 1
run_install --yes --no-service --version v1.0.0
check "a translated x86_64 shell takes the arm64 build" rc_is 0
check "requested the aarch64 archive" file_has "$root/requests.log" 'aarch64-apple-darwin.tar.gz'
run_install --yes --no-service
check "upgrade on macOS paths with a space in the state dir" rc_is 0
check "previous binary kept under Application Support" same_file \
  "$mac_home/Library/Application Support/agent-hub-state/agent-hub.prev" \
  "$root/stage/agent-hub-v1.0.0-aarch64-apple-darwin/agent-hub"

new_case native-x86-mac
darwin_stubs 0
run_install --yes --no-service
check "a native x86_64 Mac takes the x86_64 build" file_has "$root/requests.log" 'x86_64-apple-darwin.tar.gz'

printf 'install tests: %s passed, %s failed\n' "$passes" "$fails"
[ "$fails" = 0 ]
