# Desktop rendering

Desktop draws with [Blitz](https://github.com/DioxusLabs/blitz) through Vello
onto wgpu. The same markup as the web build, laid out and painted in-process.

```bash
dx serve --platform desktop          # wgpu (the `desktop` / `gpu` feature)
cargo run --release --features desktop
```

The system webview is still there if a machine cannot bring wgpu up:

```bash
cargo run --release --features webview
```

`desktop`/`gpu` and `webview` are alternatives. Enabling both is a compile
error rather than a race between two `main` branches.

## Why this is the desktop path

The webview build has no GPU-side compositor doing the diffing. Every change is
an IPC message plus DOM work on the webview's main thread — the same thread that
delivers key events. A handful of mutations is still a round trip in front of
your typing.

Blitz owns the whole pipeline. Layout and paint happen in-process and land on
the GPU, so there is no IPC hop and no borrowed main thread.

## CSS and theme

Theme is `data-theme` on `.app-root`, not a JS write to `<html>`. Custom
properties, the dark media query, and the pinned light/dark tokens all key off
that node so both renderers see the same toggle.

Blitz implements a subset of CSS. Things that may still look wrong:

- `position: sticky` on the header
- Layered `linear-gradient` / `repeating-linear-gradient` backgrounds
- `backdrop-filter` on the header
- Google Fonts over the network (system fallbacks are already in the sheet)
- Form extras: `autocapitalize`, `enterkeyhint`

Inline SVG is enabled in `dioxus-native` by default (icons, charts, envelope).

## The cost

The dependency tree is larger than the webview build, and a clean debug build
takes longer. That is the price of carrying a layout engine and a GPU stack
instead of borrowing the system's.
