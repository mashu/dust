# Dust

[![CI](https://github.com/mashu/dust/actions/workflows/ci.yml/badge.svg)](https://github.com/mashu/dust/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/mashu/dust/graph/badge.svg)](https://codecov.io/gh/mashu/dust)
[![Pages](https://github.com/mashu/dust/actions/workflows/pages.yml/badge.svg)](https://mashu.github.io/dust/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

CW Morse **group trainer** in Rust. Hear a group, type it from memory, and get
alignment-based accuracy, letter mastery and a skill score.

Two ways to practise: Koch-style **character groups**, unlocked one letter at a
time, or **callsigns** — real prefixes in realistic shapes, from `W1AW` up to
`DL1ABC/P`, the drill MorseRunner and Morse Walker are built around.

**[Try it in the browser →](https://mashu.github.io/dust/)**

## Platforms

One codebase, one feature per target. Audio is the same plan everywhere: same
keying envelope, same random tone and speed per group, same QSB, QRN and
receiver background.

| Platform | Audio | Get it | Build it |
| --- | --- | --- | --- |
| Web | Web Audio | [mashu.github.io/dust](https://mashu.github.io/dust/) | `dx serve --platform web` |
| Linux | ALSA (cpal) | `.deb` on [Releases](https://github.com/mashu/dust/releases) | `dx bundle --platform desktop --release` |
| Windows | WASAPI (cpal) | NSIS installer on Releases | `dx bundle --platform desktop --release` |
| macOS | CoreAudio (cpal) | `.dmg` on Releases (unsigned) | `dx bundle --platform desktop --release` |
| Android | AAudio via oboe (cpal) | `.apk` on Releases (debug-signed) | [docs/platforms.md](docs/platforms.md#android) |
| iOS | CoreAudio (cpal) | `.ipa` on Releases (unsigned) | [docs/platforms.md](docs/platforms.md#ios) |

Settings and history stay on the device and are never synced —
[where they live](docs/platforms.md#where-your-progress-lives).

## Develop

```bash
curl -sSL https://dioxus.dev/install.sh | bash   # once

dx serve --platform desktop
dx serve --platform web --port 8080
```

The Linux desktop build needs GTK, WebKit and ALSA:

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libasound2-dev
```

## Test

```bash
cargo test -p cw-core                    # domain logic
cargo test -p dust --features desktop    # app, audio and UI
```

Sessions run for real: a paused clock drives the true sends, gaps, timeouts and
auto-confirms in milliseconds, screens are tested by pressing their buttons, and
a recording player stands in for the sound card. The cpal backend opens the
machine's real output; where there is none, an ALSA null device stands in:

```bash
printf 'pcm.!default { type null }\nctl.!default { type null }\n' > ~/.asoundrc
```

## Release

Push a tag matching the workspace version in `Cargo.toml` (currently `0.8.0`):

```bash
git tag v0.8.0 && git push origin v0.8.0
```

CI then builds and attaches every bundle in the table above. Mobile jobs cannot
hold the desktop bundles back: if one fails, the release publishes without it.

## Layout

```
crates/cw-core   Morse, Koch pools, Farnsworth timing, sampling, score, session
src/             Dioxus app: screens, session runtime, audio backends, storage
assets/          CSS
docs/            Platform notes
```

MIT — see [LICENSE](LICENSE).
