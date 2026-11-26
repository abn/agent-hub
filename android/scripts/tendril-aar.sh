#!/usr/bin/env bash
# Builds Tendril's Kotlin binding (the JNI shim and the Rust engine, arm64 only) into
# android/build/tendril/tendril-android.aar, from a pinned ref of a local Tendril checkout.
#
#   TENDRIL_DIR  a Tendril git checkout (required)
#   TENDRIL_REF  the commit or tag to build (default: the one this app is built against)
#
# Build output lands under android/build, which is git-ignored. Nothing is installed.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ref="${TENDRIL_REF:-68c215b}"
dir="${TENDRIL_DIR:-}"
if [ -z "$dir" ] || [ ! -d "$dir/.git" ]; then
  printf 'android/tendril: set TENDRIL_DIR to a Tendril git checkout (git clone https://github.com/agync/tendril).\n' >&2
  exit 1
fi
sdk="${ANDROID_HOME:-$HOME/Android/Sdk}"
ndk="${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* 2>/dev/null | sort -V | tail -n 1)}"
bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
clang="$(ls "$bin"/aarch64-linux-android*-clang 2>/dev/null | sort -V | awk -F'aarch64-linux-android|-clang' '$2 >= 26 {print $0; exit}')"
if [ -z "$clang" ] || [ ! -x "$clang" ]; then
  printf 'android/tendril: no Android NDK with an aarch64 clang for API 26 or later under %s. Install an NDK (sdkmanager "ndk;27.1.12297006").\n' "$sdk" >&2
  exit 1
fi
if ! rustup target list --installed 2>/dev/null | grep -qx aarch64-linux-android; then
  printf 'android/tendril: the Rust target aarch64-linux-android is missing. Run: rustup target add aarch64-linux-android\n' >&2
  exit 1
fi

src="$here/build/tendril-src"
rm -rf "$src"
mkdir -p "$src" "$here/build/tendril"
git -C "$dir" archive "$ref" | tar -x -C "$src"

(
  cd "$src"
  export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$clang"
  export CC_aarch64_linux_android="$clang"
  export AR_aarch64_linux_android="$bin/llvm-ar"
  cargo build --release --target aarch64-linux-android -p tendril-ffi --lib -j 6
)

"$here/scripts/gradle.sh" -p "$src/bindings/kotlin" assembleRelease -PtendrilVersion="${TENDRIL_VERSION:-0.1.0}"
cp "$src"/bindings/kotlin/build/outputs/aar/*-release.aar "$here/build/tendril/tendril-android.aar"
printf 'android/tendril: built %s from %s\n' "$here/build/tendril/tendril-android.aar" "$ref"
