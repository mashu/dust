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

GitHub Actions then builds and attaches:

- Linux `.deb`
- Windows NSIS installer (`.exe`)
- macOS `.dmg` (Apple Silicon on `macos-latest`)

macOS builds are unsigned (right-click → Open the first time). Windows needs WebView2, which is already present on typical Windows 10/11 systems.

Native Android/iOS packages are not produced. The app has no mobile target, and store-ready IPA/AAB files need signing certificates that GitHub-hosted runners do not provide. Use the GitHub Pages build in a mobile browser instead.

## Layout

```
crates/cw-core   Morse, Koch pools, Farnsworth timing, sampling, score, session
src/             Dioxus app (Web Audio on WASM, cpal on desktop)
assets/          CSS
```
