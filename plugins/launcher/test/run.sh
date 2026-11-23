#!/bin/sh
# Behaviour tests for plugins/launcher/agent-hub-launcher against a local
# release server and stub binaries. Everything lives under target/tmp; HOME and
# the XDG directories point into it.
#
#   sh plugins/launcher/test/run.sh

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
src_launcher="$repo/plugins/launcher/agent-hub-launcher"
mkdir -p "$repo/target/tmp"
root=$(mktemp -d "$repo/target/tmp/launcher-test.XXXXXX") || exit 1
server_pid=
pids=

cleanup() {
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null
  for p in $pids; do kill "$p" 2>/dev/null; done
  rm -rf "${root:?}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

fails=0
passes=0

for tool in python3 curl tar; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "launcher tests need $tool"
    exit 1
  }
done

VERSION=$(sed -n 's/^VERSION=\([0-9][0-9.]*\).*/\1/p' "$src_launcher")
[ -n "$VERSION" ] || {
  echo "no VERSION in the launcher"
  exit 1
}

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
if [ "$os_part" = apple-darwin ] && [ "$arch_part" = x86_64 ] &&
  [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
  arch_part=aarch64
fi
target="$arch_part-$os_part"
real_uname=$(command -v uname)
real_curl=$(command -v curl)
host=$(uname -n)

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# --- release fixtures ----------------------------------------------------------

stub_body() {
  cat <<STUB
#!/bin/sh
case "\$1" in
  version | --version) echo "agent-hub $VERSION (stub)"; exit 0 ;;
esac
echo "stub ran: \$*"
STUB
}

rel="$root/download/agent-hub-v$VERSION"
mkdir -p "$rel"

# A Unix archive: <stem>/agent-hub.
stem="agent-hub-v$VERSION-$target"
mkdir -p "$root/stage/$stem"
stub_body >"$root/stage/$stem/agent-hub"
chmod 755 "$root/stage/$stem/agent-hub"
tar -czf "$rel/$stem.tar.gz" -C "$root/stage" "$stem"
printf '%s  %s.tar.gz\n' "$(sha256_of "$rel/$stem.tar.gz")" "$stem" >"$rel/$stem.sha256"
good_sum=$(cat "$rel/$stem.sha256")

# The Windows asset: a zip holding <stem>/agent-hub.exe, shaped as the release
# builds it. Built with python so the harness needs no zip tool.
wstem="agent-hub-v$VERSION-x86_64-pc-windows-msvc"
mkdir -p "$root/stage/$wstem"
stub_body >"$root/stage/$wstem/agent-hub.exe"
python3 - "$root/stage" "$wstem" "$rel/$wstem.zip" <<'PY'
import os, sys, zipfile
stage, name, out = sys.argv[1:4]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.write(os.path.join(stage, name, "agent-hub.exe"), name + "/agent-hub.exe")
PY
printf '%s  %s.zip\n' "$(sha256_of "$rel/$wstem.zip")" "$wstem" >"$rel/$wstem.sha256"

printf 'v%s\n' "$VERSION" >"$root/LATEST"
python3 "$repo/tests/install/server.py" "$root" "$root/port" &
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
base="http://127.0.0.1:$port/download"

# The shipped launcher has no way to change its download base, and it only
# talks HTTPS. The tests run a copy with the base pointed at the local server,
# plain HTTP allowed, a short probe timeout, the temp fallback moved under the
# test root, and a pause point the lock-breaking cases can wait on.
mkdir -p "$root/bin" "$root/tmpfs"
# shellcheck disable=SC2016 # the definition is written out unexpanded
pause_def='test_pause() { [ "$LAUNCHER_TEST_PAUSE_AT" = "$1" ] || return 0; : >"$LAUNCHER_TEST_GATE.reached"; while [ ! -e "$LAUNCHER_TEST_GATE" ]; do sleep 0.1; done; }'
sed -e "s|^base=.*|base=$base|" \
  -e 's|^tls_only=1|tls_only=0|' \
  -e 's|^probe_timeout=.*|probe_timeout=1|' \
  -e "s@^test_pause() .*@$pause_def@" \
  -e "s|/tmp/agent-hub-|$root/tmpfs/agent-hub-|" \
  "$src_launcher" >"$root/bin/agent-hub-launcher"
chmod 755 "$root/bin/agent-hub-launcher"
launcher="$root/bin/agent-hub-launcher"
# A copy whose release server is a dead port.
mkdir -p "$root/bin/offline"
sed -e "s|^base=.*|base=http://127.0.0.1:1|" -e 's|^tls_only=1|tls_only=0|' \
  -e "s|/tmp/agent-hub-|$root/tmpfs/agent-hub-|" "$src_launcher" >"$root/bin/offline/agent-hub-launcher"
chmod 755 "$root/bin/offline/agent-hub-launcher"
launcher_off="$root/bin/offline/agent-hub-launcher"
go_offline() { launcher_cur=$launcher_off; }

# --- case plumbing ------------------------------------------------------------------

# new_case NAME: a fresh HOME, cache and stubs directory, and a clean request log.
new_case() {
  case_name=$1
  c="$root/case-$1"
  mkdir -p "$c/home" "$c/stubs"
  : >"$root/requests.log"
  cache="$c/home/.cache/agent-hub/launcher/$VERSION"
  dir="$cache/$target"
  (umask 077 && mkdir -p "$dir")
  # A uname that can be told to lie, and a curl that can be told to dawdle.
  cat >"$c/stubs/uname" <<STUB
#!/bin/sh
if [ "\$1" = -s ] && [ -n "\$FAKE_S" ]; then echo "\$FAKE_S"; exit 0; fi
if [ "\$1" = -m ] && [ -n "\$FAKE_M" ]; then echo "\$FAKE_M"; exit 0; fi
exec "$real_uname" "\$@"
STUB
  cat >"$c/stubs/curl" <<STUB
#!/bin/sh
[ -z "\$CURL_DELAY" ] || sleep "\$CURL_DELAY"
exec "$real_curl" "\$@"
STUB
  chmod 755 "$c/stubs/uname" "$c/stubs/curl"
  unset FAKE_S FAKE_M CURL_DELAY LAUNCHER_TEST_PAUSE_AT LAUNCHER_TEST_GATE
  launcher_cur=$launcher
}

# run_launcher ARGS...: the launcher as a harness runs it, stdin empty.
run_launcher() {
  env HOME="$c/home" XDG_CACHE_HOME="$c/home/.cache" PATH="$c/stubs:$PATH" \
    sh "$launcher_cur" "$@" </dev/null
}

# hook_as SESSION ARGS...: the launcher with a hook's JSON on stdin.
hook_json() {
  json=$1
  shift
  printf '%s' "$json" | env HOME="$c/home" XDG_CACHE_HOME="$c/home/.cache" \
    PATH="$c/stubs:$PATH" sh "$launcher_cur" "$@"
}

hook_as() {
  sess=$1
  shift
  printf '{"session_id":"%s"}' "$sess" | env HOME="$c/home" XDG_CACHE_HOME="$c/home/.cache" \
    PATH="$c/stubs:$PATH" sh "$launcher_cur" "$@"
}

archive_gets() {
  grep -c "GET .*$stem.tar.gz" "$root/requests.log" 2>/dev/null || true
}

wait_for() {
  # wait_for PATH [SECONDS]
  w=0
  limit=$((${2:-10} * 10))
  while [ ! -e "$1" ] && [ $w -lt $limit ]; do
    sleep 0.1
    w=$((w + 1))
  done
  [ -e "$1" ]
}

wait_gone() {
  w=0
  while [ -e "$1" ] && [ $w -lt 100 ]; do
    sleep 0.1
    w=$((w + 1))
  done
  [ ! -e "$1" ]
}

pass() {
  passes=$((passes + 1))
  printf 'ok   %s: %s\n' "$case_name" "$1"
}

fail() {
  fails=$((fails + 1))
  printf 'FAIL %s: %s\n' "$case_name" "$1"
}

check() {
  # check DESCRIPTION CONDITION-COMMAND...
  desc=$1
  shift
  if "$@"; then pass "$desc"; else fail "$desc"; fi
}

is_empty() { [ -z "$1" ]; }
eq() { [ "$1" = "$2" ]; }
contains() { case "$1" in *"$2"*) return 0 ;; esac; return 1; }
lacks() { case "$1" in *"$2"*) return 1 ;; esac; return 0; }

# make_lock OWNER-LINE: a lock directory as another process would leave it.
make_lock() {
  mkdir -p "$dir/install.lock"
  if [ -n "$1" ]; then printf '%s\n' "$1" >"$dir/install.lock/owner"; fi
}

old_ms() { printf '%s000' "$(($(date +%s) - 3600))"; }
fresh_ms() { printf '%s000' "$(date +%s)"; }

dead_pid() {
  sh -c 'exit 0' &
  d=$!
  wait "$d"
  echo "$d"
}

# live_pid sets lp: a process that stays alive until the run ends.
live_pid() {
  sleep 120 >/dev/null 2>&1 &
  lp=$!
  pids="$pids $lp"
}

installing_line="The Agent Hub binary is installing; hub context arrives on the next prompt."

# --- cases ------------------------------------------------------------------------------

new_case warm
mkdir -p "$dir"
stub_body >"$dir/agent-hub"
chmod 755 "$dir/agent-hub"
go_offline
out=$(run_launcher mcp --proxy)
check "execs the cached binary with the arguments" eq "$out" "stub ran: mcp --proxy"
check "needs no network" is_empty "$(cat "$root/requests.log")"

new_case path-reuse
mkdir -p "$c/stubs"
stub_body >"$c/stubs/agent-hub"
chmod 755 "$c/stubs/agent-hub"
go_offline
out=$(run_launcher hook prompt --harness claude)
check "a PATH binary of the same version runs" eq "$out" "stub ran: hook prompt --harness claude"
check "nothing was installed" test ! -e "$dir/agent-hub"
sed "s/$VERSION/0.0.1/" "$c/stubs/agent-hub" >"$c/stubs/agent-hub.new"
mv "$c/stubs/agent-hub.new" "$c/stubs/agent-hub"
chmod 755 "$c/stubs/agent-hub"
out=$(printf '' | run_launcher hook prompt --harness claude)
check "a PATH binary of another version is not used" contains "$out" "installing"

new_case cold
CURL_DELAY=3
export CURL_DELAY
t0=$(date +%s)
out=$(hook_as s1 hook prompt --harness claude)
t1=$(date +%s)
unset CURL_DELAY
expect="{\"hookSpecificOutput\":{\"hookEventName\":\"UserPromptSubmit\",\"additionalContext\":\"$installing_line\"}}"
check "prints the installing line in the Claude shape" eq "$out" "$expect"
check "returns at once, with the install still running" test "$((t1 - t0))" -lt 3
check "the install holds the lock" wait_for "$dir/install.lock" 2
check "no binary yet" test ! -e "$dir/agent-hub"
check "the install finishes in the background" wait_for "$dir/agent-hub" 20
check "the binary is executable" test -x "$dir/agent-hub"
check "the lock is released" wait_gone "$dir/install.lock"
check "no temp directory is left" is_empty "$(find "$dir" -mindepth 1 -maxdepth 1 -name '.tmp*')"
out=$(run_launcher x y)
check "the next call runs it" eq "$out" "stub ran: x y"

new_case cold-antigravity
CURL_DELAY=1
export CURL_DELAY
out=$(hook_as s1 hook prompt --harness antigravity)
unset CURL_DELAY
check "prints the installing line in the Antigravity shape" eq "$out" "{\"injectSteps\":[{\"ephemeralMessage\":\"$installing_line\"}]}"
wait_for "$dir/agent-hub" 20
new_case no-context
go_offline
out=$(hook_as s1 hook session-end --harness claude)
check "an event with no context is a no-op" eq "$out" "{}"

new_case token-cold
go_offline
out=$(run_launcher token --headers --agent claude-code)
check "headers carry the agent and no Authorization" eq "$out" '{"X-Agent-Id":"claude-code"}'

new_case concurrent
CURL_DELAY=2
export CURL_DELAY
hook_as a hook prompt --harness claude >/dev/null &
p1=$!
hook_as b hook prompt --harness claude >/dev/null &
p2=$!
wait "$p1" "$p2"
unset CURL_DELAY
check "the install completes" wait_for "$dir/agent-hub" 20
wait_gone "$dir/install.lock" >/dev/null
check "one archive download for two launches" eq "$(archive_gets)" 1

new_case lock-live-old
live_pid
make_lock "$lp $host $(old_ms)"
run_launcher --install
check "an old lock with a live pid is not broken" test -d "$dir/install.lock"
check "and nothing is fetched" eq "$(archive_gets)" 0
check "and no binary appears" test ! -e "$dir/agent-hub"

new_case lock-dead-young
make_lock "$(dead_pid) $host $(fresh_ms)"
run_launcher --install
check "a young lock is not broken, even with a dead pid" test -d "$dir/install.lock"
check "and nothing is fetched" eq "$(archive_gets)" 0

new_case lock-dead-old
make_lock "$(dead_pid) $host $(old_ms)"
run_launcher --install
check "an old lock with a dead pid is broken and the install runs" test -x "$dir/agent-hub"
check "the lock is released" test ! -d "$dir/install.lock"
check "the broken lock leaves no stale directory" is_empty "$(find "$dir" -mindepth 1 -maxdepth 1 -name '*lock.stale*')"

new_case lock-other-host
live_pid
make_lock "$lp some-other-host $(old_ms)"
run_launcher --install
check "an old lock from another host is broken" test -x "$dir/agent-hub"

new_case lock-no-owner-young
make_lock ""
run_launcher --install
check "a young lock with no owner is not broken" test -d "$dir/install.lock"
check "and nothing is fetched" eq "$(archive_gets)" 0

new_case lock-no-owner-old
make_lock ""
touch -t 202001010000 "$dir/install.lock"
run_launcher --install
check "an old lock with no owner is broken" test -x "$dir/agent-hub"

new_case lock-unparsable-owner
make_lock "garbage"
touch -t 202001010000 "$dir/install.lock"
run_launcher --install
check "an old lock with an unparsable owner is broken" test -x "$dir/agent-hub"

new_case lock-race
make_lock "$(dead_pid) $host $(old_ms)"
CURL_DELAY=1
export CURL_DELAY
racers=
for _ in 1 2 3 4; do
  run_launcher --install &
  racers="$racers $!"
done
for r in $racers; do wait "$r"; done
unset CURL_DELAY
check "four breakers, one install" eq "$(archive_gets)" 1
check "the binary is in place" test -x "$dir/agent-hub"
check "no lock is left" test ! -d "$dir/install.lock"

new_case lock-breaker-paused
make_lock "$(dead_pid) $host $(old_ms)"
gate="$c/gate"
LAUNCHER_TEST_PAUSE_AT=stale-checked LAUNCHER_TEST_GATE="$gate" run_launcher --install &
pa=$!
pids="$pids $pa"
check "breaker A reads the stale owner and pauses" wait_for "$gate.reached" 10
CURL_DELAY=4
export CURL_DELAY
run_launcher --install &
pb=$!
unset CURL_DELAY
stale_owner=$(cat "$dir/install.lock/owner")
w=0
while [ "$(cat "$dir/install.lock/owner" 2>/dev/null)" = "$stale_owner" ] && [ $w -lt 100 ]; do sleep 0.1; w=$((w + 1)); done
owner_b=$(cat "$dir/install.lock/owner" 2>/dev/null)
check "breaker B broke the lock and holds a fresh one" contains "$owner_b" " $host "
: >"$gate"
wait "$pa"
check "B's lock survives A resuming" eq "$(cat "$dir/install.lock/owner" 2>/dev/null)" "$owner_b"
check "A left no break directory behind" test ! -d "$dir/install.lock.break"
wait "$pb"
check "B finishes the one install" test -x "$dir/agent-hub"
check "one archive download" eq "$(archive_gets)" 1

# The break guard belongs to whoever holds it.
new_case guard-owner-lives
make_lock "$(dead_pid) $host $(old_ms)"
gate="$c/gate"
LAUNCHER_TEST_PAUSE_AT=guard-held LAUNCHER_TEST_GATE="$gate" run_launcher --install &
pa=$!
pids="$pids $pa"
check "breaker A takes the guard and pauses" wait_for "$gate.reached" 10
guard_owner=$(cat "$dir/install.lock.break/owner" 2>/dev/null)
check "the guard names A" contains "$guard_owner" " $host "
touch -t 202001010000 "$dir/install.lock.break"
run_launcher --install
check "a second breaker gives up and fetches nothing" eq "$(archive_gets)" 0
check "the guard is not reaped while its owner lives, however old" eq "$(cat "$dir/install.lock.break/owner" 2>/dev/null)" "$guard_owner"
check "the stale lock is still there for A" test -d "$dir/install.lock"
: >"$gate"
wait "$pa"
check "A then breaks the lock and installs once" test -x "$dir/agent-hub"
check "one archive download" eq "$(archive_gets)" 1
check "no guard or lock is left" test ! -e "$dir/install.lock.break" -a ! -e "$dir/install.lock"

new_case guard-lost
make_lock "$(dead_pid) $host $(old_ms)"
gate="$c/gate"
live_pid
LAUNCHER_TEST_PAUSE_AT=guard-held LAUNCHER_TEST_GATE="$gate" run_launcher --install &
pa=$!
pids="$pids $pa"
check "breaker A takes the guard and pauses" wait_for "$gate.reached" 10
rm -rf "${dir:?}/install.lock.break"
mkdir "$dir/install.lock.break"
printf '%s %s %s\n' "$lp" "$host" "$(fresh_ms)" >"$dir/install.lock.break/owner"
: >"$gate"
wait "$pa"
check "A sees the guard is no longer its own and leaves the lock" test -d "$dir/install.lock"
check "A leaves the other owner's guard" eq "$(cut -d ' ' -f 1 "$dir/install.lock.break/owner")" "$lp"
check "and installs nothing" eq "$(archive_gets)" 0

new_case guard-ownerless-young
make_lock "$(dead_pid) $host $(old_ms)"
mkdir "$dir/install.lock.break"
run_launcher --install
check "a young guard with no owner file is left alone" test -d "$dir/install.lock.break"
check "and the stale lock stays" test -d "$dir/install.lock"
check "and nothing is fetched" eq "$(archive_gets)" 0

new_case guard-ownerless-old
make_lock "$(dead_pid) $host $(old_ms)"
mkdir "$dir/install.lock.break"
touch -t 202001010000 "$dir/install.lock.break"
run_launcher --install
check "an ownerless guard older than 60 s is reaped and the install runs" test -x "$dir/agent-hub"
check "no guard is left" test ! -e "$dir/install.lock.break"
check "no stale directory is left" is_empty "$(find "$dir" -mindepth 1 -maxdepth 1 -name '*stale*')"

new_case guard-dead-owner
make_lock "$(dead_pid) $host $(old_ms)"
mkdir "$dir/install.lock.break"
printf '%s %s %s\n' "$(dead_pid)" "$host" "$(fresh_ms)" >"$dir/install.lock.break/owner"
run_launcher --install
check "a guard whose pid is dead is reaped at once" test -x "$dir/agent-hub"

new_case guard-other-host
make_lock "$(dead_pid) $host $(old_ms)"
mkdir "$dir/install.lock.break"
printf '%s other-host %s\n' "$$" "$(fresh_ms)" >"$dir/install.lock.break/owner"
run_launcher --install
check "a young guard from another host is left alone" test -d "$dir/install.lock.break"
printf '%s other-host %s\n' "$$" "$(old_ms)" >"$dir/install.lock.break/owner"
run_launcher --install
check "an old guard from another host is reaped" test -x "$dir/agent-hub"

new_case bad-checksum
printf '%s  %s.tar.gz\n' 0000000000000000000000000000000000000000000000000000000000000000 "$stem" >"$rel/$stem.sha256"
run_launcher --install
rc=$?
check "the install fails" test "$rc" -ne 0
check "no binary is left" test ! -e "$dir/agent-hub"
check "launcher-error says why" eq "$(cat "$dir/launcher-error" 2>/dev/null)" "checksum mismatch"
check "no temp directory is left" is_empty "$(find "$dir" -mindepth 1 -maxdepth 1 -name '.tmp*')"
check "the lock is released" test ! -d "$dir/install.lock"
gets=$(archive_gets)
out=$(hook_as s1 hook prompt --harness claude)
check "the hook says hooks are off" contains "$out" "Hooks are off: checksum mismatch. Use the hub tools directly."
out=$(hook_as s1 hook prompt --harness claude)
check "and says it once per session" eq "$out" "{}"
out=$(hook_as s2 hook prompt --harness claude)
check "again for another session" contains "$out" "Hooks are off"
out=$(hook_json '{"conversationId":"c1","hook":"pre"}' hook prompt --harness antigravity)
check "an Antigravity conversation hears it" contains "$out" "Hooks are off"
out=$(hook_json '{"conversationId":"c1","hook":"pre"}' hook prompt --harness antigravity)
check "and once per conversation" eq "$out" "{}"
out=$(hook_json '{"conversationId":"c2"}' hook prompt --harness antigravity)
check "again for a new conversation" contains "$out" "Hooks are off"
check "no retry inside the hour" eq "$(archive_gets)" "$gets"
printf '%s\n' "$good_sum" >"$rel/$stem.sha256"
touch -t 202001010000 "$dir/launcher-error"
out=$(hook_as s3 hook prompt --harness claude)
check "after an hour a hook retries" contains "$out" "installing"
check "and the retry installs" wait_for "$dir/agent-hub" 20

new_case unsupported
FAKE_S=Plan9
FAKE_M=mips
export FAKE_S FAKE_M
out=$(hook_as s1 hook prompt --harness claude)
unset FAKE_S FAKE_M
check "unsupported platform turns hooks off" contains "$out" "Hooks are off: unsupported platform Plan9-mips. Use the hub tools directly."
check "nothing is fetched" eq "$(archive_gets)" 0
check "the reason is recorded" eq "$(cat "$cache/unsupported/launcher-error")" "unsupported platform Plan9-mips"
FAKE_S=Plan9
FAKE_M=mips
export FAKE_S FAKE_M
out=$(hook_as s1 hook prompt --harness claude)
unset FAKE_S FAKE_M
check "a second call in the session stays quiet" eq "$out" "{}"

new_case offline
go_offline
run_launcher --install
check "an unreachable host fails" test ! -e "$dir/agent-hub"
check "launcher-error names the url" contains "$(cat "$dir/launcher-error")" "download failed: http://127.0.0.1:1/agent-hub-v$VERSION/$stem.tar.gz"
out=$(hook_as s1 hook prompt --harness claude)
check "the hook reports it" contains "$out" "Hooks are off: download failed"

if command -v unzip >/dev/null 2>&1; then
  new_case windows
  FAKE_S=MINGW64_NT-10.0
  FAKE_M=x86_64
  export FAKE_S FAKE_M
  wdir="$cache/x86_64-pc-windows-msvc"
  run_launcher --install
  check "the zip installs agent-hub.exe" test -x "$wdir/agent-hub.exe"
  check "and no bare agent-hub" test ! -e "$wdir/agent-hub"
  out=$(run_launcher a b)
  unset FAKE_S FAKE_M
  check "the launcher runs agent-hub.exe" eq "$out" "stub ran: a b"
else
  printf 'skip windows: no unzip on this host\n'
fi

# --- the cache ------------------------------------------------------------

# shellcheck disable=SC2012 # the mode string is the point; stat flags differ per OS
mode_of() { ls -ld "$1" | cut -c1-10; }
uid=$(id -u)

new_case cache-created-private
rm -rf "${c:?}/home/.cache"
run_launcher --install
for d in "$c/home/.cache/agent-hub" "$c/home/.cache/agent-hub/launcher" "$cache" "$dir"; do
  check "${d#"$c/home/.cache/"} is mode 0700" eq "$(mode_of "$d")" "drwx------"
done
check "the binary is installed" test -x "$dir/agent-hub"

new_case cache-no-home
rm -rf "${root:?}"/tmpfs/agent-hub-*
env -u HOME -u XDG_CACHE_HOME PATH="$c/stubs:$PATH" sh "$launcher" --install </dev/null
fb="$root/tmpfs/agent-hub-$uid"
check "without HOME it installs under a per-user directory" test -x "$fb/launcher/$VERSION/$target/agent-hub"
check "that directory is mode 0700" eq "$(mode_of "$fb")" "drwx------"
check "and no .cache is made beside it" test ! -e "$root/tmpfs/.cache"

new_case cache-fallback-loose
rm -rf "${root:?}"/tmpfs/agent-hub-*
mkdir -m 755 "$fb"
out=$(printf '{"session_id":"s1"}' | env -u HOME -u XDG_CACHE_HOME PATH="$c/stubs:$PATH" sh "$launcher" hook prompt --harness claude)
check "a fallback directory open to others is refused" contains "$out" "the cache directory is not private"
check "and is not used" test ! -e "$fb/launcher"
check "and is left as it was" eq "$(mode_of "$fb")" "drwxr-xr-x"

new_case cache-fallback-symlink
rm -rf "${root:?}"/tmpfs/agent-hub-*
mkdir -p "$c/evil"
ln -s "$c/evil" "$fb"
env -u HOME -u XDG_CACHE_HOME PATH="$c/stubs:$PATH" sh "$launcher" --install </dev/null
check "a symlinked fallback is refused" is_empty "$(ls -A "$c/evil")"
rm -f "${fb:?}"

new_case cache-relative-xdg
(cd "$c" && env HOME="$c/home" XDG_CACHE_HOME=relcache PATH="$c/stubs:$PATH" sh "$launcher" --install </dev/null)
check "a relative XDG_CACHE_HOME is ignored" test ! -e "$c/relcache"
check "and HOME/.cache is used" test -x "$dir/agent-hub"

new_case cache-symlinked-version-dir
mkdir -p "$c/evil/$target"
stub_body >"$c/evil/$target/agent-hub"
chmod 700 "$c/evil" "$c/evil/$target"
chmod 755 "$c/evil/$target/agent-hub"
rm -rf "${cache:?}"
ln -s "$c/evil" "$cache"
out=$(run_launcher x y 2>/dev/null)
check "a binary behind a symlinked cache directory is not run" is_empty "$out"
out=$(hook_as s1 hook prompt --harness claude)
check "the hook says the cache is not private" contains "$out" "the cache directory is not private"
check "nothing was fetched" eq "$(archive_gets)" 0

new_case cache-loose-dir
stub_body >"$dir/agent-hub"
chmod 755 "$dir/agent-hub"
chmod 775 "$dir"
out=$(run_launcher x y 2>/dev/null)
check "a group-writable cache directory is not trusted" is_empty "$out"
chmod 700 "$dir"
out=$(run_launcher x y)
check "and is trusted again at 0700" eq "$out" "stub ran: x y"

new_case bin-is-symlink
stub_body >"$c/real-stub"
chmod 755 "$c/real-stub"
ln -s "$c/real-stub" "$dir/agent-hub"
out=$(run_launcher x y 2>/dev/null)
check "a symlinked binary is not run" is_empty "$out"
wait_for "$dir/install.lock" 5 >/dev/null
wait_gone "$dir/install.lock" >/dev/null
check "the install replaces it with a regular file" test -f "$dir/agent-hub" -a ! -L "$dir/agent-hub"
# shellcheck disable=SC2016 # the line is compared literally
check "and leaves the link's target alone" eq "$(sed -n 2p "$c/real-stub")" 'case "$1" in'

new_case bin-is-directory
mkdir "$dir/agent-hub"
out=$(run_launcher x y 2>/dev/null)
check "a directory named agent-hub is not run" is_empty "$out"
wait_for "$dir/install.lock" 5 >/dev/null
wait_gone "$dir/install.lock" >/dev/null
check "the install replaces it with the binary" test -f "$dir/agent-hub"

# --- the download --------------------------------------------------------------

new_case shipped-https-only
cat >"$c/stubs/curl" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$CURL_LOG"
exit 22
STUB
chmod 755 "$c/stubs/curl"
env HOME="$c/home" XDG_CACHE_HOME="$c/home/.cache" PATH="$c/stubs:$PATH" CURL_LOG="$c/curl.log" \
  AGENT_HUB_LAUNCHER_BASE="$base" sh "$src_launcher" --install </dev/null
curl_args=$(cat "$c/curl.log" 2>/dev/null)
check "the shipped launcher ignores a download-base variable" is_empty "$(cat "$root/requests.log")"
check "and asks GitHub for its own version" contains "$curl_args" "https://github.com/agync/agent-hub/releases/download/agent-hub-v$VERSION/$stem.tar.gz"
check "over HTTPS only" contains "$curl_args" "--proto =https"
check "including redirects" contains "$curl_args" "--proto-redir =https"
check "a failed download is recorded" contains "$(cat "$dir/launcher-error")" "download failed: https://github.com/"

# --- the archive ------------------------------------------------------------------

cat >"$root/mkarch.py" <<'PY'
import io, os, sys, tarfile, zipfile
mode, out, stem, exe, evil = sys.argv[1:6]
body = b"#!/bin/sh\necho pwned\n"
member = stem + "/" + exe

def add(t, name, typ="file", link=None):
    ti = tarfile.TarInfo(name)
    ti.mode = 0o755
    if typ == "file":
        ti.size = len(body)
        t.addfile(ti, io.BytesIO(body))
    elif typ == "sym":
        ti.type, ti.linkname = tarfile.SYMTYPE, link
        t.addfile(ti)
    elif typ == "hard":
        ti.type, ti.linkname = tarfile.LNKTYPE, link
        t.addfile(ti)

if mode.startswith("tar-"):
    with tarfile.open(out, "w:gz") as t:
        if mode == "tar-traversal":
            add(t, "../evil")
            add(t, member)
        elif mode == "tar-symlink":
            add(t, member, "sym", os.path.join(evil, "target"))
        elif mode == "tar-hardlink":
            add(t, stem + "/other")
            add(t, member, "hard", stem + "/other")
        elif mode == "tar-duplicate":
            add(t, member)
            add(t, member)
        elif mode == "tar-symlinked-dir":
            add(t, stem, "sym", evil)
            add(t, member)
        elif mode == "tar-absolute":
            add(t, os.path.join(evil, "abs"))
            add(t, member)
else:
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        if mode == "zip-traversal":
            z.writestr("../evil", body)
            z.writestr(member, body)
        elif mode == "zip-symlink":
            zi = zipfile.ZipInfo(member)
            zi.create_system = 3
            zi.external_attr = (0o120777 << 16)
            z.writestr(zi, os.path.join(evil, "target"))
PY

cp "$rel/$stem.tar.gz" "$root/good.tar.gz"
cp "$rel/$stem.sha256" "$root/good.tar.sha256"

bad_tar() {
  new_case "archive-$1"
  evil="$c/evil"
  mkdir -p "$evil"
  python3 "$root/mkarch.py" "$2" "$rel/$stem.tar.gz" "$stem" agent-hub "$evil"
  printf '%s  %s.tar.gz\n' "$(sha256_of "$rel/$stem.tar.gz")" "$stem" >"$rel/$stem.sha256"
  run_launcher --install
  check "the install fails" test ! -e "$dir/agent-hub"
  check "launcher-error says the archive is unsafe" contains "$(cat "$dir/launcher-error" 2>/dev/null)" "unsafe"
  check "nothing is written outside the staging directory" is_empty "$(find "$evil" "$dir" -mindepth 1 -maxdepth 1 ! -name launcher-error)"
  check "no temp directory is left" is_empty "$(find "$dir" -mindepth 1 -maxdepth 1 -name '.tmp*')"
  check "the lock is released" test ! -d "$dir/install.lock"
  cp "$root/good.tar.gz" "$rel/$stem.tar.gz"
  cp "$root/good.tar.sha256" "$rel/$stem.sha256"
}
bad_tar traversal tar-traversal
bad_tar symlink-member tar-symlink
bad_tar hardlink-member tar-hardlink
bad_tar duplicate-member tar-duplicate
bad_tar symlinked-dir tar-symlinked-dir
bad_tar absolute-name tar-absolute

if command -v unzip >/dev/null 2>&1; then
  cp "$rel/$wstem.zip" "$root/good.zip"
  cp "$rel/$wstem.sha256" "$root/good.zip.sha256"
  bad_zip() {
    new_case "archive-$1"
    FAKE_S=MINGW64_NT-10.0
    FAKE_M=x86_64
    export FAKE_S FAKE_M
    wdir="$cache/x86_64-pc-windows-msvc"
    evil="$c/evil"
    mkdir -p "$evil"
    python3 "$root/mkarch.py" "$2" "$rel/$wstem.zip" "$wstem" agent-hub.exe "$evil"
    printf '%s  %s.zip\n' "$(sha256_of "$rel/$wstem.zip")" "$wstem" >"$rel/$wstem.sha256"
    run_launcher --install
    unset FAKE_S FAKE_M
    check "the install fails" test ! -e "$wdir/agent-hub.exe"
    check "launcher-error says the archive is unsafe" contains "$(cat "$wdir/launcher-error" 2>/dev/null)" "unsafe"
    check "nothing is written outside the staging directory" is_empty "$(find "$evil" "$wdir" -mindepth 1 -maxdepth 1 ! -name launcher-error)"
    cp "$root/good.zip" "$rel/$wstem.zip"
    cp "$root/good.zip.sha256" "$rel/$wstem.sha256"
  }
  bad_zip zip-traversal zip-traversal
  bad_zip zip-symlink zip-symlink
fi

# --- the checksum ----------------------------------------------------------------------

good_hash=$(cut -d ' ' -f 1 "$root/good.tar.sha256")
bad_sum() {
  # bad_sum NAME EXPECTED-REASON: the checksum file is already in place.
  run_launcher --install
  check "$1: the install fails" test ! -e "$dir/agent-hub"
  check "$1: launcher-error says why" contains "$(cat "$dir/launcher-error" 2>/dev/null)" "$2"
  check "$1: no temp directory is left" is_empty "$(find "$dir" -mindepth 1 -maxdepth 1 -name '.tmp*')"
  check "$1: the lock is released" test ! -d "$dir/install.lock"
  cp "$root/good.tar.sha256" "$rel/$stem.sha256"
}
new_case sum-missing
mv "$rel/$stem.sha256" "$root/held.sha256"
bad_sum missing "download failed"
new_case sum-empty
: >"$rel/$stem.sha256"
bad_sum empty "malformed"
new_case sum-truncated
printf '%s' "$(printf '%s' "$good_hash" | cut -c1-40)" >"$rel/$stem.sha256"
bad_sum truncated "malformed"
new_case sum-digest-only
printf '%s\n' "$good_hash" >"$rel/$stem.sha256"
bad_sum "digest only" "malformed"
new_case sum-not-hex
printf '%s  %s.tar.gz\n' "$(printf 'z%.0s' $(seq 64))" "$stem" >"$rel/$stem.sha256"
bad_sum "not hex" "malformed"
new_case sum-two-records
{
  cat "$root/good.tar.sha256"
  printf '%s  other.tar.gz\n' "$good_hash"
} >"$rel/$stem.sha256"
bad_sum "two records" "malformed"
new_case sum-other-asset
printf '%s  agent-hub-v%s-other.tar.gz\n' "$good_hash" "$VERSION" >"$rel/$stem.sha256"
bad_sum "another asset" "another asset"
new_case sum-binary-marker
printf '%s *%s.tar.gz\n' "$good_hash" "$stem" >"$rel/$stem.sha256"
run_launcher --install
check "a binary-mode marker before the name is accepted" test -x "$dir/agent-hub"
cp "$root/good.tar.sha256" "$rel/$stem.sha256"

# --- finding a binary on PATH -------------------------------------------------------

new_case path-stdin-kept
cat >"$c/stubs/agent-hub" <<STUB
#!/bin/sh
if [ "\$1" = version ]; then cat >/dev/null; echo "agent-hub $VERSION (abc1234)"; exit 0; fi
printf 'stdin: %s\n' "\$(cat)"
STUB
chmod 755 "$c/stubs/agent-hub"
out=$(hook_as s1 hook prompt --harness claude)
check "the probe leaves the hook's stdin for the binary" eq "$out" 'stdin: {"session_id":"s1"}'

new_case path-wrong-line
cat >"$c/stubs/agent-hub" <<STUB
#!/bin/sh
echo "not agent-hub $VERSION"
STUB
chmod 755 "$c/stubs/agent-hub"
out=$(hook_as s1 hook prompt --harness claude)
check "a line that only mentions the version is not a match" contains "$out" "installing"
wait_for "$dir/agent-hub" 20 >/dev/null
wait_gone "$dir/install.lock" >/dev/null

new_case path-hangs
cat >"$c/stubs/agent-hub" <<'STUB'
#!/bin/sh
exec sleep 30
STUB
chmod 755 "$c/stubs/agent-hub"
t0=$(date +%s)
out=$(hook_as s1 hook prompt --harness claude)
t1=$(date +%s)
check "a PATH binary that hangs does not hold the hook" test "$((t1 - t0))" -lt 10
check "and is not used" contains "$out" "installing"
wait_for "$dir/agent-hub" 20 >/dev/null
wait_gone "$dir/install.lock" >/dev/null

new_case path-glob
cat >"$c/stubs/agent-hub" <<'STUB'
#!/bin/sh
echo '* agent-hub'
STUB
chmod 755 "$c/stubs/agent-hub"
out=$(hook_as s1 hook prompt --harness claude)
check "glob characters in the output are inert" contains "$out" "installing"
wait_for "$dir/agent-hub" 20 >/dev/null
wait_gone "$dir/install.lock" >/dev/null

# --- the packaged copies --------------------------------------------------------------------------

new_case dist
stage="$c/dist/agent-hub-antigravity-test"
if (cd "$repo" && make -s ANTIGRAVITY_STAGE="$stage" plugins/antigravity-dist) >"$c/make.log" 2>&1; then
  pass "make plugins/antigravity-dist succeeds"
else
  fail "make plugins/antigravity-dist succeeds"
  cat "$c/make.log"
fi
archive="$stage.tar.gz"
check "it produced the archive in the fresh output directory" test -f "$archive"
listing=$(tar -tvzf "$archive" 2>/dev/null | grep 'agent-hub/bin/agent-hub-launcher$')
check "the Antigravity archive holds the launcher" contains "$listing" "agent-hub/bin/agent-hub-launcher"
check "and it is executable" contains "$(printf '%s' "$listing" | cut -c1-4)" "-rwx"
for d in claude antigravity; do
  check "plugins/$d/bin carries the same launcher" cmp -s "$src_launcher" "$repo/plugins/$d/bin/agent-hub-launcher"
  check "plugins/$d/bin launcher is executable" test -x "$repo/plugins/$d/bin/agent-hub-launcher"
done

printf '\n%s passed, %s failed\n' "$passes" "$fails"
[ "$fails" -eq 0 ]
