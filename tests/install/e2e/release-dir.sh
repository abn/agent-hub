#!/bin/sh
# Lay out a local release for the install scripts: the given binary packed the
# way binaries.yml packs it, with its checksum, under the directory the scripts
# expect (<out>/agent-hub-vX.Y.Z/). Prints the version, without the v.
#
#   release-dir.sh BIN OUT [TARGET]
#
# The version is the binary's own, so the scripts' post-install check passes.
set -eu

bin=${1:?usage: release-dir.sh BIN OUT [TARGET]}
out=${2:?usage: release-dir.sh BIN OUT [TARGET]}

version=$("$bin" --version | sed -n '1s/^agent-hub \([^ ]*\).*/\1/p')
[ -n "$version" ] || { echo "release-dir: no version from $bin" >&2; exit 1; }

if [ -n "${3:-}" ]; then
  target=$3
else
  case "$(uname -s)" in
    Linux) os_part=unknown-linux-musl ;;
    Darwin) os_part=apple-darwin ;;
    *) echo "release-dir: unsupported host" >&2; exit 1 ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch_part=x86_64 ;;
    aarch64 | arm64) arch_part=aarch64 ;;
    *) echo "release-dir: unsupported arch" >&2; exit 1 ;;
  esac
  target=$arch_part-$os_part
fi

name=agent-hub-v$version-$target
rel=$out/agent-hub-v$version
stage=$out/stage/$name
mkdir -p "$rel" "$stage"
cp "$bin" "$stage/agent-hub"
# A debug build is hundreds of megabytes; the symbols are not under test.
strip "$stage/agent-hub" 2>/dev/null || true
chmod 755 "$stage/agent-hub"
tar -czf "$rel/$name.tar.gz" -C "$out/stage" "$name"
if command -v sha256sum >/dev/null 2>&1; then
  sum=$(sha256sum "$rel/$name.tar.gz" | cut -d ' ' -f 1)
else
  sum=$(shasum -a 256 "$rel/$name.tar.gz" | cut -d ' ' -f 1)
fi
printf '%s  %s.tar.gz\n' "$sum" "$name" >"$rel/$name.sha256"
rm -rf "$out/stage"
printf '%s\n' "$version"
