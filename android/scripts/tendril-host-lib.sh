#!/usr/bin/env bash
# Builds the HOST (x86_64) Tendril engine and its JNI shim, so the real-hub test can run a real Tendril
# initiator in a JVM. Output: android/build/tendril-host/libtendril_jni.so. Same inputs as tendril-aar.sh.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ref="${TENDRIL_REF:-68c215b}"
dir="${TENDRIL_DIR:-}"
if [ -z "$dir" ] || [ ! -d "$dir/.git" ]; then
  printf 'android/test-hub: set TENDRIL_DIR to a Tendril git checkout.\n' >&2
  exit 1
fi
jdk="${JAVA_HOME:-}"
[ -x "$jdk/bin/javac" ] || jdk="$(dirname "$(dirname "$(readlink -f "$(command -v javac)")")")"
src="$here/build/tendril-host-src"
out="$here/build/tendril-host"
mkdir -p "$src" "$out"
find "$src" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
git -C "$dir" archive "$ref" | tar -x -C "$src"
(cd "$src" && cargo build -p tendril-ffi --lib -j 6)
gcc -shared -fPIC -Wall -Wextra -I"$jdk/include" -I"$jdk/include/linux" -I"$src/crates/tendril-ffi/include" \
  "$src/bindings/kotlin/jni/tendril_jni.c" -L"$src/target/debug" -ltendril_ffi -Wl,-rpath,"$out" -o "$out/libtendril_jni.so"
cp "$src/target/debug/libtendril_ffi.so" "$out/"
printf 'android/test-hub: built %s\n' "$out/libtendril_jni.so"
