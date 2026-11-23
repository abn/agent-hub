#!/bin/sh
# shellcheck disable=SC2015,SC2016  # A && B || C and the inner sh -c strings are deliberate
# The Linux service e2e, run as an ordinary user inside the systemd container
# that linux.sh starts. It installs a release served on 127.0.0.1:8099 with the
# real install.sh and drives the service lifecycle through the real systemd
# user manager. Exit 0 means every check held.
#
#   linux-inner.sh VERSION

version=${1:?usage: linux-inner.sh VERSION}
script=/e2e/deploy/install.sh
export AGENT_HUB_DOWNLOAD_BASE=http://127.0.0.1:8099
bin=$HOME/.local/bin/agent-hub
unit=$HOME/.config/systemd/user/agent-hub.service
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
# expect_out NAME PATTERN CMD...: CMD succeeds and its output holds PATTERN.
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
  while [ $i -lt 40 ]; do
    ready >/dev/null 2>&1 && return 0
    sleep 0.5
    i=$((i + 1))
  done
  return 1
}
wait_down() {
  i=0
  while [ $i -lt 40 ]; do
    ready >/dev/null 2>&1 || return 0
    sleep 0.5
    i=$((i + 1))
  done
  return 1
}
# The login the script stands in for has no session bus variables, which is
# the case the installer must cope with; the checks below need them.
sc() { XDG_RUNTIME_DIR=/run/user/$(id -u) systemctl --user "$@"; }
main_pid() { sc show -p MainPID --value agent-hub; }

# 1. A first install registers, starts and answers.
sh "$script" --version "v$version" --yes || fail "install.sh exit $?"
check "the binary is placed" test -x "$bin"
check "the unit is written" test -f "$unit"
check "the unit says systemd manages it" grep -q 'AGENT_HUB_MANAGED_BY=systemd' "$unit"
check "the service is active" sc is-active --quiet agent-hub
check "the hub answers /readyz" wait_ready
check "status says running" "$bin" status
check "linger is on, so the service outlives the login" \
  sh -c 'loginctl show-user "$(id -un)" -p Linger | grep -q "Linger=yes"'
pid1=$(main_pid)

# 2. The same version again does nothing; --force reinstalls over a running hub.
expect_out "the same version is a no-op" "already installed" \
  sh "$script" --version "v$version" --yes
sh "$script" --version "v$version" --yes --force || fail "forced reinstall exit $?"
check "the hub answers after a forced reinstall" wait_ready
pid2=$(main_pid)
[ -n "$pid2" ] && [ "$pid2" != "$pid1" ] && pass "the forced reinstall restarted the hub" ||
  fail "the forced reinstall kept pid $pid1"

# 3. stop drains and checkpoints; start and restart bring it back.
expect_out "stop checkpoints the store" "store checkpointed" "$bin" stop
check "the hub is down after stop" wait_down
"$bin" status >/dev/null 2>&1
[ $? -eq 3 ] && pass "status exits 3 when stopped" || fail "status exit after stop"
expect_out "start brings the service back" "started\|running" "$bin" start
check "the hub answers after start" wait_ready
pid3=$(main_pid)
"$bin" restart >/dev/null 2>&1 || fail "restart exit $?"
check "the hub answers after restart" wait_ready
pid4=$(main_pid)
[ -n "$pid4" ] && [ "$pid4" != "$pid3" ] && pass "restart is a new process" ||
  fail "restart kept pid $pid3"

# 4. A crash is restarted by systemd.
kill -9 "$pid4" 2>/dev/null
check "the hub is back after kill -9" sh -c '
  i=0
  while [ $i -lt 60 ]; do
    p=$(XDG_RUNTIME_DIR=/run/user/$(id -u) systemctl --user show -p MainPID --value agent-hub)
    [ -n "$p" ] && [ "$p" != 0 ] && [ "$p" != '"$pid4"' ] &&
      curl -fsS --max-time 3 http://127.0.0.1:8080/readyz >/dev/null 2>&1 && exit 0
    sleep 0.5
    i=$((i + 1))
  done
  exit 1'

# 5. Logs come from the journal.
expect_out "logs print something" "." "$bin" logs -n 5

# 6. Uninstall with purge leaves nothing behind.
sh "$script" --uninstall --purge --yes || fail "uninstall exit $?"
check "the unit is gone" test ! -e "$unit"
check "the binary is gone" test ! -e "$bin"
unit_gone() { [ "$(sc show -p LoadState --value agent-hub 2>/dev/null)" = not-found ]; }
check "the service is not loaded" unit_gone
check "the hub is down" wait_down
check "the data directory is gone" test ! -e "$HOME/.local/share/agent-hub/hub.db"

printf '\n%s failed\n' "$fails"
[ "$fails" -eq 0 ]
