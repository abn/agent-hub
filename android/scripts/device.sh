#!/usr/bin/env bash
# Runs the app on hardware. Only this app's debug package is ever installed or launched; nothing else
# on a device is touched, and nothing is installed on the host.
#
#   device.sh devices
#   device.sh install <serial>     build the debug APK, install it, launch it
#   device.sh emu                  boot the existing emulator, install, launch (arm64 images only)
#   device.sh smoke <serial>       run the instrumented smoke test on the device
#
# HUB_TRANSPORT (default seeded) picks the hub the debug build talks to; tendril tries a real hub.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk="${ANDROID_HOME:-$HOME/Android/Sdk}"
adb="$sdk/platform-tools/adb"
app="dev.agync.agenthub.debug"
apk="$here/app/build/outputs/apk/debug/app-debug.apk"
transport="${HUB_TRANSPORT:-seeded}"

need_serial() {
  if [ -z "${1:-}" ]; then
    printf 'device.sh: set DEVICE=<serial>. Devices:\n' >&2
    "$adb" devices -l >&2
    exit 1
  fi
}

build() { "$here/scripts/gradle.sh" :app:assembleDebug -PhubTransport="$transport"; }

launch() { "$adb" "$@" shell am start -n "$app/dev.agync.agenthub.MainActivity"; }

case "${1:-}" in
  devices)
    "$adb" devices -l
    ;;
  install)
    need_serial "${2:-}"
    build
    "$adb" -s "$2" install -r "$apk"
    launch -s "$2"
    ;;
  emu)
    avd="$("$sdk/emulator/emulator" -list-avds | head -n 1)"
    [ -n "$avd" ] || { printf 'device.sh: no AVD exists\n' >&2; exit 1; }
    if ! "$adb" devices | grep -q '^emulator-'; then
      nohup "$sdk/emulator/emulator" -avd "$avd" -no-snapshot-save >/dev/null 2>&1 &
    fi
    "$adb" wait-for-device
    abis="$("$adb" -e shell getprop ro.product.cpu.abilist)"
    case "$abis" in
      *arm64-v8a*) ;;
      *) printf 'device.sh: the emulator runs %s but the app is arm64 only (Tendril). Use a device: make android/device\n' "$abis" >&2; exit 1 ;;
    esac
    build
    "$adb" -e install -r "$apk"
    launch -e
    ;;
  smoke)
    need_serial "${2:-}"
    ANDROID_SERIAL="$2" "$here/scripts/gradle.sh" :app:connectedDebugAndroidTest
    ;;
  *)
    printf 'usage: device.sh devices | install <serial> | emu | smoke <serial>\n' >&2
    exit 2
    ;;
esac
