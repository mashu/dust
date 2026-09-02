# Dust

CW Morse **group trainer** in Rust. Runs as a Linux desktop app and as WebAssembly in the browser.

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

## Web release

```bash
dx bundle --platform web --release
```

Static files go to `target/dx/dust/release/web/public` (and `dist/` if you copy them). Serve over HTTPS.

## Layout

```
crates/cw-core   Morse, Koch pools, Farnsworth timing, sampling, score, session
src/             Dioxus app (Web Audio on WASM, cpal on Linux)
assets/          CSS
```
