#!/bin/sh
# Install, upgrade or remove Agent Hub for the current user.
#
#   curl -fsSL https://hub.agync.dev/install.sh | sh
#   curl -fsSL https://hub.agync.dev/install.sh | sh -s -- [flags]
#
# The script downloads a release archive, checks it, places the binary and
# hands over to `agent-hub service install`. The binary owns the lifecycle.
# Every function is defined first and the last line is one compound command,
# so a download cut off anywhere is a syntax error and runs nothing.

REPO="agync/agent-hub"

usage() {
  cat <<'USAGE'
Usage: install.sh [flags]

  --version vX.Y.Z      install a release instead of the latest   (AGENT_HUB_VERSION)
  --bin-dir DIR         where the binary goes                     (AGENT_HUB_BIN_DIR)
  --no-service          binary only, no service                   (AGENT_HUB_NO_SERVICE=1)
  --bind ADDR           passed to `agent-hub service install`
  --boot | --login      passed to `agent-hub service install`
  --require-provenance  fail unless `gh attestation verify` passes
  --force               reinstall the version that is already there
  --uninstall [--purge] run `agent-hub uninstall`
  --yes                 no prompts
  --allow-root          install as root anyway
  -h, --help            show this text

Finding the latest release needs curl; with only wget, pass --version.

Environment: AGENT_HUB_DOWNLOAD_BASE overrides the release download URL,
AGENT_HUB_LATEST_URL the page whose redirect names the latest release.
Both must be http or https URLs.

Exit: 0 done, 1 download, checksum, provenance or service failure,
2 usage, 3 unsupported platform or a release without service commands.
USAGE
}

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
  code=$1
  shift
  printf 'error: %s\n' "$*" >&2
  exit "$code"
}

have() { command -v "$1" >/dev/null 2>&1; }

detect_platform() {
  os=$(uname -s 2>/dev/null) || os=unknown
  arch=$(uname -m 2>/dev/null) || arch=unknown
  case "$os" in
    Linux) os_part=unknown-linux-musl ;;
    Darwin) os_part=apple-darwin ;;
    MINGW* | MSYS* | CYGWIN*)
      die 3 "this is Windows: run install.ps1 in PowerShell, or use WSL"
      ;;
    *) die 3 "unsupported operating system: $os" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch_part=x86_64 ;;
    aarch64 | arm64) arch_part=aarch64 ;;
    *) die 3 "unsupported architecture: $arch (releases cover x86_64 and arm64)" ;;
  esac
  # An x86_64 shell under Rosetta on Apple silicon: take the native build.
  if [ "$os" = Darwin ] && [ "$arch_part" = x86_64 ]; then
    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
      arch_part=aarch64
    fi
  fi
  target="$arch_part-$os_part"
}

resolve_dirs() {
  [ -n "${HOME:-}" ] || die 2 "HOME is not set"
  if [ "$os" = Darwin ]; then
    cache_root="$HOME/Library/Caches"
    state_dir="$HOME/Library/Application Support/agent-hub-state"
  else
    cache_root="${XDG_CACHE_HOME:-$HOME/.cache}"
    state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/agent-hub"
  fi
  bin_dir="${bin_dir:-$HOME/.local/bin}"
  # An absolute, slash-free-ended path, so advice and the PATH check are exact.
  case "$bin_dir" in
    /*) ;;
    *) bin_dir="$PWD/$bin_dir" ;;
  esac
  while [ "$bin_dir" != / ] && [ "${bin_dir%/}" != "$bin_dir" ]; do
    bin_dir=${bin_dir%/}
  done
}

# acquire_lock: one install per user at a time. A lock whose owner is gone is
# taken over.
acquire_lock() {
  mkdir -p "$state_dir" || die 1 "cannot create $state_dir"
  lock="$state_dir/install.lock"
  tries=0
  while ! mkdir "$lock" 2>/dev/null; do
    holder=$(cat "$lock/pid" 2>/dev/null || true)
    if [ -z "$holder" ]; then
      sleep 1
      holder=$(cat "$lock/pid" 2>/dev/null || true)
    fi
    if [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null; then
      die 1 "another install is running (pid $holder)"
    fi
    tries=$((tries + 1))
    [ "$tries" -le 3 ] || die 1 "cannot take the install lock $lock"
    # Rename the stale lock away before deleting it: the rename is atomic and
    # only one contender wins it, so a fresh lock is never the one removed.
    if mv "$lock" "$lock.stale.$$" 2>/dev/null; then
      # The rename may have caught a lock another contender took in between:
      # put that one back and look again.
      if [ "$(cat "$lock.stale.$$/pid" 2>/dev/null || true)" = "$holder" ]; then
        rm -rf "$lock.stale.$$"
      else
        mv "$lock.stale.$$" "$lock" 2>/dev/null || rm -rf "$lock.stale.$$"
      fi
    fi
  done
  have_lock=1
  printf '%s\n' "$$" >"$lock/pid"
}

# Runs from the EXIT trap.
# shellcheck disable=SC2317
cleanup() {
  [ -z "$work" ] || rm -rf "$work"
  [ -z "$stage" ] || rm -f "$stage"
  [ -z "$prev_tmp" ] || rm -f "$prev_tmp"
  if [ "$have_lock" = 1 ] && [ "$(cat "$lock/pid" 2>/dev/null || true)" = "$$" ]; then
    rm -rf "$lock"
  fi
}

fetch() {
  # fetch URL DEST
  if have curl; then
    curl -fsSLg --retry 3 -o "$2" --url "$1"
  elif have wget; then
    wget -q -O "$2" -- "$1"
  else
    die 1 "neither curl nor wget is installed"
  fi
}

# valid_version TAG: v, a digit, then only letters, digits and . _ + -
valid_version() {
  case "$1" in
    v[0-9]*) ;;
    *) return 1 ;;
  esac
  case "$1" in
    *[!0-9A-Za-z._+-]*) return 1 ;;
  esac
  return 0
}

discover_version() {
  # BusyBox wget cannot stop at a redirect, so discovery is curl only.
  have curl || die 2 "finding the latest release needs curl; pass --version vX.Y.Z instead"
  loc=$(curl -fsSIg -o /dev/null -w '%{redirect_url}' --url "$latest_url") ||
    die 1 "could not read the latest release from $latest_url"
  path=${loc%%[?#]*}
  case "$path" in
    */releases/tag/agent-hub-v*) ;;
    *) die 1 "could not tell the latest release from the redirect: ${loc:-empty}" ;;
  esac
  tag=${path##*/releases/tag/}
  case "$tag" in
    */*) die 1 "could not tell the latest release from the redirect: ${loc:-empty}" ;;
  esac
  tag=${tag#agent-hub-}
  valid_version "$tag" || die 1 "the latest release has an unusable name: $tag"
  version=$tag
}

normalise_version() {
  case "$version" in
    v*) ;;
    [0-9]*) version="v$version" ;;
  esac
  valid_version "$version" || die 2 "not a release version: $version"
}

installed_version() {
  # Prints "1.13.0" for "agent-hub 1.13.0 (abc123)", nothing when absent.
  [ -x "$bin_dir/agent-hub" ] || return 0
  "$bin_dir/agent-hub" --version 2>/dev/null | sed -n '1s/^agent-hub \([^ ]*\).*/\1/p'
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif have shasum; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  elif have openssl; then
    openssl dgst -sha256 "$1" | sed 's/^.*= *//'
  else
    die 1 "no sha256 tool found (sha256sum, shasum or openssl); refusing to install unchecked"
  fi
}

verify_checksum() {
  expected=$(cut -d ' ' -f 1 "$work/$name.sha256" | tr 'A-F' 'a-f')
  actual=$(sha256_of "$work/$archive")
  [ -n "$expected" ] || die 1 "the checksum file is empty"
  [ "$expected" = "$actual" ] || die 1 "checksum mismatch for $archive (expected $expected, got $actual)"
  say "checksum: ok"
}

verify_provenance() {
  if have gh; then
    if gh attestation verify "$work/$archive" --repo "$REPO" >/dev/null 2>&1; then
      say "provenance: verified (gh attestation)"
    elif [ "$require_provenance" = 1 ]; then
      die 1 "provenance check failed for $archive"
    else
      warn "provenance: not verified (gh attestation verify failed)"
    fi
  elif [ "$require_provenance" = 1 ]; then
    die 1 "--require-provenance needs the gh CLI, which is not installed"
  else
    say "provenance: not checked (gh not installed)"
  fi
}

# quote_sh / quote_fish: a path as one literal word for the target shell.
quote_sh() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
quote_fish() { printf "'%s'" "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e "s/'/\\\\'/g")"; }

# place_binary: stage beside the target, keep the old binary, then rename over.
place_binary() {
  mkdir -p "$bin_dir" || die 1 "cannot create $bin_dir"
  stage=$(mktemp "$bin_dir/.agent-hub.new.XXXXXX") || die 1 "cannot write to $bin_dir"
  cp "$new_bin" "$stage" || die 1 "cannot write to $bin_dir"
  chmod 755 "$stage" || die 1 "cannot write to $bin_dir"
  if [ -e "$bin_dir/agent-hub" ]; then
    prev_tmp=$(mktemp "$state_dir/.agent-hub.prev.XXXXXX") ||
      die 1 "cannot keep the previous binary in $state_dir; nothing was changed"
    if cp "$bin_dir/agent-hub" "$prev_tmp" && chmod 755 "$prev_tmp" &&
      mv -f "$prev_tmp" "$state_dir/agent-hub.prev"; then
      prev_tmp=
    else
      die 1 "cannot keep the previous binary in $state_dir; nothing was changed"
    fi
  fi
  mv -f "$stage" "$bin_dir/agent-hub" || die 1 "cannot place the binary in $bin_dir"
  stage=
}

path_advice() {
  case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *)
      case "${SHELL:-}" in
        */fish) line="fish_add_path $(quote_fish "$bin_dir")" ;;
        *) line="export PATH=$(quote_sh "$bin_dir"):\"\$PATH\"" ;;
      esac
      say "$bin_dir is not on PATH. Add it with:"
      say "  $line"
      ;;
  esac
  other=$(command -v agent-hub 2>/dev/null || true)
  if [ -n "$other" ] && [ "$other" != "$bin_dir/agent-hub" ]; then
    warn "another agent-hub shadows this one: $other"
  fi
}

do_uninstall() {
  bin="$bin_dir/agent-hub"
  [ -x "$bin" ] || bin=$(command -v agent-hub 2>/dev/null || true)
  if [ -z "$bin" ]; then
    say "agent-hub is not installed, nothing to remove"
    exit 0
  fi
  set -- uninstall
  [ "$purge" = 1 ] && set -- "$@" --purge
  [ "$yes" = 1 ] && set -- "$@" --yes
  # Under `curl | sh` stdin is the script; prompts need the terminal.
  if [ "$yes" != 1 ] && (: </dev/tty) 2>/dev/null; then
    "$bin" "$@" </dev/tty
  else
    "$bin" "$@"
  fi
  exit $?
}

install_service() {
  set -- service install --yes
  [ -n "$bind" ] && set -- "$@" --bind "$bind"
  [ -n "$boot" ] && set -- "$@" "$boot"
  "$bin_dir/agent-hub" "$@" </dev/null
  rc=$?
  case "$rc" in
    0) ;;
    69)
      say "No service manager is reachable here, so the binary is installed without a service."
      say "Run it with: agent-hub start"
      ;;
    *)
      die 1 "agent-hub service install failed (exit $rc); fix the cause, then re-run this installer with the same flags or run: agent-hub service install"
      ;;
  esac
}

# has_service BIN: releases before the native lifecycle have no service
# commands, and `service --help` is the cheapest probe that tells.
has_service() {
  "$1" service --help >/dev/null 2>&1 </dev/null
}

# require_service BIN WANT: refuse a service setup the binary cannot do.
require_service() {
  has_service "$1" ||
    die 3 "agent-hub $2 has no service commands, which came after v1.12.0; install a newer release, or pass --no-service to place the binary only"
}

# check_new_binary: the unpacked binary must be the release that was asked for.
# A release old enough to print no version at all may still be placed, but
# only without a service.
check_new_binary() {
  got=$("$new_bin" --version 2>/dev/null </dev/null | sed -n '1s/^agent-hub \([^ ]*\).*/\1/p')
  if [ "$no_service" != 1 ]; then
    require_service "$new_bin" "$want"
  fi
  if [ -z "$got" ] && [ "$no_service" = 1 ]; then
    warn "agent-hub $want does not report its version"
  elif [ "$got" != "$want" ]; then
    die 1 "the unpacked binary reports ${got:-no version}, not agent-hub $want; refusing to install it"
  fi
}

# same_version: the binary is already there. The service step still runs, as it
# is idempotent, so a failed first run or a changed --bind takes effect.
same_version() {
  if [ "$no_service" = 1 ]; then
    return 0
  fi
  require_service "$bin_dir/agent-hub" "$want"
  install_service
}

# running_state: 0 and 1 from `status` both mean a hub is running.
running_state() {
  "$bin_dir/agent-hub" status >/dev/null 2>&1 </dev/null
  case $? in
    0 | 1) return 0 ;;
    *) return 1 ;;
  esac
}

main() {
  version=${AGENT_HUB_VERSION:-}
  bin_dir=${AGENT_HUB_BIN_DIR:-}
  no_service=0
  [ "${AGENT_HUB_NO_SERVICE:-}" = 1 ] && no_service=1
  bind=
  boot=
  require_provenance=0
  force=0
  uninstall=0
  purge=0
  yes=0
  allow_root=0
  work=
  stage=
  prev_tmp=
  have_lock=0
  lock=

  while [ $# -gt 0 ]; do
    case "$1" in
      --version | --bin-dir | --bind)
        case "${2-}" in
          '' | -*) die 2 "$1 needs a value" ;;
        esac
        case "$1" in
          --version) version=$2 ;;
          --bin-dir) bin_dir=$2 ;;
          --bind) bind=$2 ;;
        esac
        shift
        ;;
      --version=* | --bin-dir=* | --bind=*)
        [ -n "${1#*=}" ] || die 2 "${1%%=*} needs a value"
        case "$1" in
          --version=*) version=${1#*=} ;;
          --bin-dir=*) bin_dir=${1#*=} ;;
          --bind=*) bind=${1#*=} ;;
        esac
        ;;
      --no-service) no_service=1 ;;
      --boot | --login)
        [ -z "$boot" ] || [ "$boot" = "$1" ] || die 2 "--boot and --login exclude each other"
        boot=$1
        ;;
      --require-provenance) require_provenance=1 ;;
      --force) force=1 ;;
      --uninstall) uninstall=1 ;;
      --purge) purge=1 ;;
      --yes | -y) yes=1 ;;
      --allow-root) allow_root=1 ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        usage >&2
        die 2 "unknown flag: $1"
        ;;
    esac
    shift
  done

  [ "$purge" = 0 ] || [ "$uninstall" = 1 ] || die 2 "--purge only goes with --uninstall"
  base="${AGENT_HUB_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download}"
  latest_url="${AGENT_HUB_LATEST_URL:-https://github.com/$REPO/releases/latest}"
  for u in "$base" "$latest_url"; do
    case "$u" in
      http://* | https://*) ;;
      *) die 2 "not an http or https URL: $u" ;;
    esac
  done
  base=${base%/}
  [ -z "$version" ] || normalise_version
  if [ "$(id -u)" = 0 ] && [ "$allow_root" != 1 ]; then
    die 2 "this is a per-user install; run it as your own user, or pass --allow-root"
  fi

  detect_platform
  resolve_dirs
  [ "$uninstall" = 1 ] && do_uninstall

  trap cleanup EXIT
  trap 'exit 130' INT TERM
  acquire_lock

  if [ -z "$version" ]; then
    discover_version
  fi
  want=${version#v}

  have_version=$(installed_version)
  if [ -n "$have_version" ] && [ "$have_version" = "$want" ] && [ "$force" != 1 ] &&
    [ "$require_provenance" != 1 ]; then
    say "agent-hub $want is already installed in $bin_dir (use --force to reinstall)"
    same_version
    exit 0
  fi

  name="agent-hub-$version-$target"
  archive="$name.tar.gz"
  url="$base/agent-hub-$version"

  mkdir -p "$cache_root/agent-hub" || die 1 "cannot create $cache_root/agent-hub"
  work=$(mktemp -d "$cache_root/agent-hub/install.XXXXXX") || die 1 "cannot create a download directory"

  say "Downloading agent-hub $want for $target"
  fetch "$url/$archive" "$work/$archive" || die 1 "download failed: $url/$archive"
  fetch "$url/$name.sha256" "$work/$name.sha256" || die 1 "download failed: $url/$name.sha256"
  verify_checksum
  verify_provenance

  tar -xzf "$work/$archive" -C "$work" || die 1 "cannot unpack $archive"
  new_bin="$work/$name/agent-hub"
  [ -f "$new_bin" ] || die 1 "the archive has no agent-hub binary"
  chmod 755 "$new_bin"
  check_new_binary

  # With provenance required, the installed bytes count only if they are the
  # verified release's bytes.
  if [ "$have_version" = "$want" ] && [ "$force" != 1 ] && cmp -s "$new_bin" "$bin_dir/agent-hub"; then
    say "agent-hub $want is already installed in $bin_dir and matches the verified release"
    same_version
    exit 0
  fi

  was_running=0
  if [ -n "$have_version" ] && running_state; then
    was_running=1
  fi

  place_binary
  if [ -n "$have_version" ]; then
    say "Upgraded agent-hub $have_version to $want ($bin_dir/agent-hub)"
    say "Previous binary kept at $state_dir/agent-hub.prev"
  else
    say "Installed agent-hub $want to $bin_dir/agent-hub"
  fi
  path_advice

  if [ "$no_service" = 1 ]; then
    say "Binary only: no service was installed."
    if [ "$was_running" = 1 ]; then
      say "A running agent-hub still runs the old version. Switch with: agent-hub restart"
    fi
    return 0
  fi
  install_service
  if [ "$was_running" = 1 ]; then
    "$bin_dir/agent-hub" restart </dev/null || die 1 "agent-hub restart failed"
  fi
  return 0
}

{ main "$@"; exit $?; }
