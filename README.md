# Dust

[![CI](https://github.com/mashu/dust/actions/workflows/ci.yml/badge.svg)](https://github.com/mashu/dust/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/mashu/dust/graph/badge.svg)](https://codecov.io/gh/mashu/dust)
[![GitHub Pages](https://github.com/mashu/dust/actions/workflows/pages.yml/badge.svg)](https://mashu.github.io/dust/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

CW Morse **group trainer** in Rust. Runs as a desktop app (Linux, Windows, macOS) and as WebAssembly in the browser.

Hear a Morse group, type it from memory, then see alignment-based accuracy and a skill score. Domain logic lives in `crates/cw-core` (no DOM, no audio backend).

## Develop

```bash
# CLI once: curl -sSL https://dioxus.dev/install.sh | bash

# Linux desktop
dx serve --platform desktop

# Browser / WASM
dx serve --platform web --port 8080

# Android (needs JDK 17 + Android SDK/NDK, see below)
dx serve --platform android

# iOS simulator (needs a Mac with Xcode)
dx serve --platform ios
```

## Tests

```bash
cargo test -p cw-core                      # domain logic
cargo test -p dust --features desktop      # app, audio and UI
```

The app tests run the real thing rather than a mock of it. A session is driven
through the actual Dioxus runtime with a paused clock, so sends, gaps, timeouts
and auto-confirms happen on their true schedule in milliseconds of wall time,
and screens are tested by pressing their buttons and reading the HTML that comes
back. Audio goes through a recording player, so what the trainer asked to be
sent — and when it was cancelled, stalled or retried — is checked without a
sound card. `src/testing.rs` holds that harness.

The cpal backend is exercised against the machine's real default output. On a
machine with no sound card, an ALSA null device stands in:

```bash
printf 'pcm.!default { type null }\nctl.!default { type null }\n' > ~/.asoundrc
```

Without one those few tests report that they are skipping and pass.

## Linux desktop bundle

`dx bundle` on Linux targets the native desktop app. That needs GTK/WebKit and ALSA:

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev
```

Then:

```bash
dx bundle --platform desktop --release
```

The binary is named **dust**. Look under `target/dx/dust/release/linux/`.

## GitHub Pages

Pushes to `main` deploy the WASM web app to GitHub Pages:

https://mashu.github.io/dust/

One-time: in the GitHub repo go to **Settings → Pages → Build and deployment** and set **Source** to **GitHub Actions**.

## GitHub Releases

Push a git tag that matches the workspace version in `Cargo.toml` (currently `0.3.0`):

```bash
git tag v0.3.0
git push origin v0.3.0
```

Creating the tag only in the GitHub UI also works. To rebuild an existing tag, use **Actions → Release → Run workflow** and pass `v0.3.0`.

GitHub Actions then builds and attaches:

- Linux `.deb`
- Windows NSIS installer (`.exe`)
- macOS `.dmg` (Apple Silicon on `macos-latest`)
- Android `.apk` (arm64)
- iOS `.ipa` (arm64, unsigned — see below)

macOS builds are unsigned (right-click → Open the first time). Windows needs WebView2, which is already present on typical Windows 10/11 systems.

The Android APK is signed with Gradle's debug key: fine for sideloading, not for the Play
Store. For Play, build a signed AAB locally (see **Play Store** below). The iOS IPA carries no
signature at all — a sideloader adds one, see **iOS** below. Neither mobile job can hold back
the desktop bundles: if one fails, the release still publishes without it.

## Android

Dioxus's `mobile` feature is the same wry/tao webview stack as the desktop build, so the whole
UI, curriculum and scoring come across unchanged. `dx` enables that feature itself for
`--platform android`.

Audio has full parity with the desktop app. It is not a reduced mobile build: `src/audio/native.rs`
is compiled as-is for Android, with cpal routing it to AAudio through
[oboe](https://github.com/google/oboe) instead of ALSA. Same keying envelope, same random tone
and speed per group, same QSB fading, QRN static and receiver background. Building it compiles
oboe's C++ with the NDK toolchain, which is why an NDK is needed and not just the SDK.

One Android-specific fallback: the receiver background runs as a second, continuously open
output stream, and some devices refuse to open two at once. If that happens the Morse, its
envelope and QSB still play — only the background hiss drops out — rather than the session
refusing to start. Mixing the background into a single stream would remove even that caveat.

If a toolchain bring-up ever blocks on oboe, `--features mobile-silent` builds the same app
with a player that keeps a session's timing but makes no sound, and the practice screen says so.

Settings and history persist: `dirs` has no `HOME` to work from on Android, so the store
resolves `/data/data/<package>/files` from the package name in `/proc/self/cmdline`.

### Building one yourself

```bash
rustup target add aarch64-linux-android
export ANDROID_HOME=~/Android/Sdk          # or /usr/lib/android-sdk
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
dx bundle --platform android --target aarch64-linux-android --package-types apk --release
```

Pass `--target` explicitly: without it `dx` builds for the host architecture, which on an
x86_64 machine means an emulator APK that a phone will not run.

On Debian the `google-android-*-installer` packages in contrib fetch Google's SDK bits
(`apt-cache search google-android` shows which versions your release carries); the official
`commandlinetools` zip plus `sdkmanager` gives tighter control over NDK versions. Either way
you need JDK 17 and `ANDROID_HOME`/`ANDROID_NDK_HOME` exported.

### Getting an APK without cutting a release

**Actions → Android APK → Run workflow**, on any branch. The APK is attached to that run as an
artifact.

### Play Store

Play wants an **Android App Bundle** (`.aab`) signed with an upload key, targeting API 36.
The keystore lives outside the repo (`~/.android/dust-play-upload.jks`) so it is never
committed.

One-time, create the upload key (keep the env file private; losing the key means a Play
Console upload-key reset):

```bash
install -m 700 -d ~/.android
pw="$(openssl rand -base64 32)"
printf 'KEYSTORE_PASSWORD=%s\nKEY_PASSWORD=%s\n' "$pw" "$pw" > ~/.android/dust-play-upload.env
chmod 600 ~/.android/dust-play-upload.env
# shellcheck disable=SC1090
source ~/.android/dust-play-upload.env
keytool -genkeypair -v \
  -keystore ~/.android/dust-play-upload.jks \
  -storetype JKS \
  -alias upload \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -dname "CN=Dust Morse Trainer, O=Dust, C=US" \
  -storepass "$KEYSTORE_PASSWORD" \
  -keypass "$KEY_PASSWORD"
chmod 600 ~/.android/dust-play-upload.jks
unset pw KEYSTORE_PASSWORD KEY_PASSWORD
```

Then, with a JDK that includes `javac` (17 or 21) and a writable SDK (this machine uses
`~/Android/Sdk`, NDK `27.2.12479018`):

```bash
scripts/android-play-bundle.sh
```

That writes `dist/dust-<version>-android-arm64.aab` (`dev.dust.morse`, versionCode 1). In
Play Console create the app, let Google generate the app signing key, and upload that AAB.
Back up `~/.android/dust-play-upload.jks` and `~/.android/dust-play-upload.env`. The public
upload certificate is `dist/dust-upload-certificate.pem`.

## iOS

Same story as Android — `mobile` is the wry/tao webview stack, cpal reaches CoreAudio, and
`dirs` needs no special case because iOS sets `$HOME` to the app container. One thing is
iOS-only: an app that has not claimed an `AVAudioSession` is silent, follows the ringer switch
and gets interrupted by anything else on the device, and cpal does not claim one. The player
takes the `playback` category before opening its first stream.

What you can build, and what Apple lets you install, are different questions:

| | Signing | Runs on |
| --- | --- | --- |
| Simulator build | none | a Mac |
| **Unsigned IPA** (this repo) | none | your iPhone, after a sideloader re-signs it |
| Signed IPA / TestFlight | Apple Developer Program, $99/yr | any device, no re-signing |

Releases carry the unsigned `.ipa`, and **Actions → iOS IPA → Run workflow** builds one from
any branch between releases. Install it with
[AltStore](https://altstore.io), [SideStore](https://sidestore.io) or
[Sideloadly](https://sideloadly.io): they re-sign the app with your own Apple ID. A free Apple
ID gives a signature that lasts **7 days** and allows **three** sideloaded apps at a time —
the tools re-sign in place before it expires. A paid membership removes both limits and is the
only route to TestFlight or the App Store.

CI type-checks the simulator target on every PR, which needs no Apple account and covers the
audio-session code and cpal's CoreAudio backend.

## Where your progress lives

Nothing syncs. Each install keeps its own history, in the place that platform gives it:

| Build | Location |
| --- | --- |
| Web | `localStorage` for that exact origin (`https://mashu.github.io`, or `localhost:8080` while developing — the two are separate stores) |
| Linux desktop | `~/.local/share/dust/` (`$XDG_DATA_HOME`) |
| Windows / macOS | the platform data dir `dirs` reports |
| Android | `/data/data/<package>/files/dust/` |
| iOS | `Library/Application Support/dust/` inside the app container |

Updating the app does not clear any of it. The storage keys (`dust_settings`, `dust_sessions`,
`dust_auto_adjust_*`) and file names are not versioned, and settings files from older builds
still load — new fields fall back to their defaults.

Stats are computed across your whole history. Changing course — Koch to Mixed, a new sequence,
a custom alphabet — does not hide what you already did: accuracy over time, letter mastery,
mistakes, the calendar and the streak all count every session, because a letter is the same
letter whichever set it was sent under.

The one exception is the **Sampling** tab, which shows what the trainer will draw next and so
narrows to the current character set, the same way the sampler itself does.

## Layout

```
crates/cw-core   Morse, Koch pools, Farnsworth timing, sampling, score, session
src/             Dioxus app (Web Audio on WASM, cpal on desktop and Android)
assets/          CSS
```
