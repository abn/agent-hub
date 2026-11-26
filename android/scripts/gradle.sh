#!/usr/bin/env bash
# Runs the Gradle wrapper with the SDK and a JDK found, and under the host-friendly resource cap.
# JAVA_HOME and ANDROID_HOME win when set. Never installs anything.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

has_javac() { [ -x "$1/bin/javac" ]; }

if [ -z "${JAVA_HOME:-}" ] || ! has_javac "$JAVA_HOME"; then
  JAVA_HOME=""
  for candidate in /usr/lib/jvm/*temurin*21* /usr/lib/jvm/*21* /usr/lib/jvm/*17* "$HOME"/.gradle/jdks/*17* "$HOME"/.gradle/jdks/*21*; do
    if [ -d "$candidate" ] && has_javac "$candidate"; then
      JAVA_HOME="$candidate"
      break
    fi
  done
fi
if [ -z "$JAVA_HOME" ]; then
  printf 'android: no JDK 17 or 21 with javac found. Set JAVA_HOME to one.\n' >&2
  exit 1
fi
export JAVA_HOME

if [ -z "${ANDROID_HOME:-}" ] && [ -d "$HOME/Android/Sdk" ]; then
  ANDROID_HOME="$HOME/Android/Sdk"
fi
if [ -z "${ANDROID_HOME:-}" ] || [ ! -d "$ANDROID_HOME/platforms" ]; then
  printf 'android: Android SDK not found. Set ANDROID_HOME (needs platforms;android-37.0, build-tools, and for the Tendril AAR ndk and cmake 3.22.1).\n' >&2
  exit 1
fi
export ANDROID_HOME

cd "$here"
if command -v systemd-run >/dev/null 2>&1 && [ -z "${AGENT_HUB_UNCAPPED:-}" ]; then
  exec systemd-run --user --scope -q -p CPUQuota=600% -p CPUWeight=20 -p IOWeight=20 nice -n 10 ./gradlew --max-workers=6 "$@"
fi
exec ./gradlew --max-workers=6 "$@"
