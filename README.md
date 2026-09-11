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
```

```bash
cargo test -p cw-core
```

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

Push a git tag that matches the workspace version in `Cargo.toml` (currently `0.1.0`):

```bash
git tag v0.1.0
git push origin v0.1.0
```

Creating the tag only in the GitHub UI also works. To rebuild an existing tag, use **Actions → Release → Run workflow** and pass `v0.1.0`.

GitHub Actions then builds and attaches:

- Linux `.deb`
- Windows NSIS installer (`.exe`)
- macOS `.dmg` (Apple Silicon on `macos-latest`)
- Android `.apk` (arm64)

macOS builds are unsigned (right-click → Open the first time). Windows needs WebView2, which is already present on typical Windows 10/11 systems.

The Android APK is signed with Gradle's debug key: fine for sideloading, not for the Play
Store, which wants an AAB signed with an upload key. No iOS package is produced — a
store-ready IPA needs signing certificates that GitHub-hosted runners do not provide.

## Android

Dioxus's `mobile` feature is the same wry/tao webview stack as the desktop build, so the whole
UI, curriculum and scoring come across unchanged. `dx` enables that feature itself for
`--platform android`.

Audio is cpal's Android backend — AAudio through [oboe](https://github.com/google/oboe) — so
the player, band simulation and QSB are the desktop ones. Building it compiles oboe's C++ with
the NDK toolchain, which is why an NDK is required and not just the SDK. If a toolchain
bring-up ever blocks on that, `--features mobile-silent` builds the same app with a player
that keeps a session's timing but makes no sound, and the practice screen says so.

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

## Layout

```
crates/cw-core   Morse, Koch pools, Farnsworth timing, sampling, score, session
src/             Dioxus app (Web Audio on WASM, cpal on desktop and Android)
assets/          CSS
```
