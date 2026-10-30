# Dust

[![CI](https://github.com/mashu/dust/actions/workflows/ci.yml/badge.svg)](https://github.com/mashu/dust/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/mashu/dust/graph/badge.svg)](https://codecov.io/gh/mashu/dust)
[![Pages](https://github.com/mashu/dust/actions/workflows/pages.yml/badge.svg)](https://mashu.github.io/dust/)
[![License: GPL v3](https://img.shields.io/badge/license-GPLv3-blue.svg)](LICENSE)

A CW Morse group trainer: hear a group, type what you copied, see how you did.

**[Try it in the browser](https://mashu.github.io/dust/)**, or download it for
Linux, Windows, macOS, Android or iOS from
[Releases](https://github.com/mashu/dust/releases).

## Build

```bash
cargo install dioxus-cli --version 0.7.10 --locked   # once

dx serve --platform desktop
dx serve --platform web
```

`dx` must match the `dioxus` in `Cargo.lock`. CI builds with the Rust release
named by `RUST_TOOLCHAIN` in `.github/workflows`, so its lints are that
release's.

On Linux, install ALSA and WebKitGTK first:

```bash
sudo apt install libasound2-dev libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev
```

Android, iOS and desktop bundles: [docs/platforms.md](docs/platforms.md).

## Test

```bash
cargo test -p cw-core
cargo test -p dust --features desktop
```

## Release

Push a tag `v` + the version in `Cargo.toml`, e.g. `git tag v1.2.3 && git push
origin v1.2.3`, on a commit pushed to `main`. The release waits for that
commit's CI run and stops unless it passed; then it builds every bundle and
attaches it to the release.

## License

Copyright © 2026 Mateusz Kaduk SO5KM.

Dust is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version. Anything built from it and distributed must be released under the
same terms, with its source.

### App stores

As an additional permission under section 7 of the GNU General Public License
version 3, you may package Dust, or a work based on it, for iOS, Android or any
other platform, and convey it through an app store or a similar distribution
service, such as Apple's App Store or Google Play, even when the service's
terms or technical measures (usage rules, device limits, digital rights
management) add restrictions that the GNU General Public License would
otherwise not allow, provided that

- it stays free: you offer it free of charge, with nothing in it sold;
- the Corresponding Source of what you convey is also available under the GNU
  General Public License, with or without this permission, from a place free
  of those restrictions; and
- you add no restrictions of your own.

Anyone who conveys Dust may remove this permission from their copy, as
section 7 allows.

### No warranty

Dust is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR
A PARTICULAR PURPOSE. See [LICENSE](LICENSE) for the full text of the GNU
General Public License version 3.
