#!/usr/bin/env bash
# Lays down the docs renderer in site/.toolchain for a host with no container
# runtime, which is what a Cloudflare build image is: the pinned okf-wiki
# release, verified against the checksums committed in okf-wiki.sha256. The
# release archive carries the Mermaid runtime the docs' diagrams need. The
# Makefile picks the directory up on its own.
#
# VERSION is the release the pinned container image carries. To move it, change VERSION and
# replace okf-wiki.sha256 with the new release's checksums.txt.
set -euo pipefail

VERSION="v1.2.0"
REPO="https://github.com/abn/okf-wiki"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$SCRIPT_DIR/.toolchain"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=linux_amd64 ;;
  Linux-aarch64 | Linux-arm64) target=linux_arm64 ;;
  Darwin-x86_64) target=darwin_amd64 ;;
  Darwin-arm64) target=darwin_arm64 ;;
  *) echo "error: no okf-wiki release for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

# Scratch space beside the toolchain, not the system temp directory.
work="$SCRIPT_DIR/.toolchain-work"
rm -rf "$work"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

echo "==> Fetching okf-wiki $VERSION ($target)"
archive="okf-wiki_${target}.tar.gz"
curl -fsSL --retry 3 -o "$work/$archive" "$REPO/releases/download/$VERSION/$archive"
if command -v sha256sum >/dev/null; then sha=(sha256sum); else sha=(shasum -a 256); fi
(cd "$work" && grep " $archive\$" "$SCRIPT_DIR/okf-wiki.sha256" | "${sha[@]}" --check --status -) \
  || { echo "error: $archive does not match okf-wiki.sha256" >&2; exit 1; }
tar -xzf "$work/$archive" -C "$work"

rm -rf "$OUT"
mkdir -p "$OUT/bin"
install -m 0755 "$work/okf-wiki_${target}/okf-wiki" "$OUT/bin/okf-wiki"
cp -r "$work/okf-wiki_${target}/mermaid" "$OUT/mermaid"

echo "==> Renderer ready: $OUT"
