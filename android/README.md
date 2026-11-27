# Agent Hub for Android

The phone is where asks are answered. The app pairs with a hub through Tendril, holds
a session open for alerts, and shows Now, Projects, Artifacts and the Hub. It is a
Kotlin and Jetpack Compose app. `DESIGN.md` has the Android rules it is held to.

| Target | What it does |
|---|---|
| `make android/tendril` | Builds Tendril's Kotlin binding into `android/build/tendril/` from a checkout (`TENDRIL_DIR=path`, `TENDRIL_REF` pins the commit) |
| `make android/build` | Debug APK |
| `make android/test` | Unit tests, Compose UI tests and a render of every screen |
| `make android/screenshots` | Writes every screen, light and dark, to `android/app/build/screenshots` |
| `make android/lint` | Android lint |
| `make android/devices` | Lists the devices adb sees |
| `make android/device DEVICE=serial` | Installs the debug build (package `dev.agync.agenthub.debug`, never colliding with a release install) and launches it. `HUB_TRANSPORT=tendril` tries a real hub |
| `make android/emu` | Boots the existing emulator and installs the debug build; refuses an x86_64 image, since the app is arm64 only |
| `make android/smoke DEVICE=serial` | Runs the instrumented test that loads Tendril's native library and creates an endpoint, on a device |
| `make android/test-hub HUB_BIN=path TENDRIL_DIR=path` | Runs the transport against a real hub binary and a real Tendril initiator: pair, confirm, `/devices/me`, `/home`, approve, unpair |
| `make android/release` | Signed release APK, with `ANDROID_KEYSTORE_PROPERTIES=path/to/keystore.properties` |
| `make android/apk VERSION=x.y.z` | Unsigned release APK with that version, as the release workflow builds it before signing |

Needs the Android SDK (`ANDROID_HOME`, default `~/Android/Sdk`, with platform 37 and build-tools),
a JDK 17 or 21, and for the Tendril binding an NDK, CMake 3.22.1 and the Rust target
`aarch64-linux-android`. `android/scripts/gradle.sh` finds them, runs Gradle under a CPU cap, and
installs nothing. The app ships `arm64-v8a` only, because Tendril does.

## Signing

The release key never lives in this repository. `keystore.properties` holds `storeFile`,
`storePassword`, `keyAlias` and `keyPassword`; a relative `storeFile` is read next to the
properties file. Point `ANDROID_KEYSTORE_PROPERTIES` (or the Gradle property
`agenthub.keystoreProperties`) at it. Without one, the release build is unsigned.

Every GitHub release carries a signed APK: the release workflow builds it unsigned and signs it in
a separate job with the key held in the `release-signing` environment (see `RELEASING.md`).

## Which hub the app talks to

`-PhubTransport=seeded` runs the whole app against built-in sample data, with a pairing that accepts
any code, so every screen can be used with no hub. `-PhubTransport=tendril` is the real transport:
pairing and the REST API go through a Tendril session (HTTP/2 over the session, OkHttp on a pipe
socket, no credential). Debug builds and the screenshots default to seeded, the release build to
tendril; the property overrides either.

## Layout

- `ui/theme` tokens, type, glyphs and controls; `ui/screens` one file per area; `ui/Root.kt` the shell
- `data` the hub client contract, the seeded hub, the repository (cache, offline queue, snooze)
- `viewmodel` state holders; `model` the words and counts the screens show
- `alerts` the refetch-and-notify logic, the notification poster, the held-session service, the tile
- `tendril` the only code that touches Tendril: the session pipe, the HTTP transport, the hub client and its JSON mapping; `pairing` the seam in front of it
- `widgets` the three launcher widgets (Glance): they read the cache and only open the app
