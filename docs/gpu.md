# The GPU renderer

An experiment, on a branch. `--features gpu` draws the same app with
[Blitz](https://github.com/DioxusLabs/blitz) — its own layout engine painting
through Vello onto wgpu — instead of handing markup to the system webview.

```bash
cargo run --no-default-features --features gpu
```

`gpu` and `desktop` are alternatives, not a stack: enabling both is a compile
error rather than a race between two `main` branches.

## Why it might be worth it

The webview build has no GPU-side compositor doing the diffing. Every change is
an IPC message plus DOM work on the webview's main thread — the same thread that
delivers key events. That is why a keystroke costing twenty mutations felt like
wading and one does not: there is nowhere for the work to go but in front of
your typing.

Blitz owns the whole pipeline. Layout and paint happen in-process and land on
the GPU, so there is no IPC hop and no borrowed main thread.

## What is actually verified

Honestly, not much yet — a headless container has no display.

- It resolves: `dioxus-native 0.7.10`, matching the app's Dioxus version. The
  0.8 alpha that `cargo info` shows is for a Dioxus this app is not on.
- It compiles and links, with `wgpu 26` under `vello 0.6`.
- It starts, and gets as far as asking for a window before dying on
  `neither WAYLAND_DISPLAY nor WAYLAND_SOCKET nor DISPLAY is set` — the one
  step this environment cannot do.

Nothing below the window has been seen.

## What to look at first

Blitz implements a subset of CSS, and this app's stylesheet is not a simple
one. In rough order of how likely each is to be missing or wrong:

- `position: sticky`, which holds the receiver panel above the group list
- `contain: paint` on the scope's face
- Custom properties, which the entire theme is built from — including the
  `prefers-color-scheme` and `[data-theme]` blocks that switch it
- Layered `linear-gradient` / `repeating-linear-gradient` backgrounds
- Inline SVG: the scope trace, the accuracy chart, the keying envelope, every
  icon. `dioxus-native` does enable its `svg` feature by default.
- Form behaviour: focus, caret, `autocapitalize`, `enterkeyhint`. The training
  screen is a column of text inputs, so this is the one that decides whether
  the experiment is usable at all.

## The cost

The dependency tree goes from 890 crates to 1422, and a clean debug build takes
a couple of minutes rather than seconds. That is the price of carrying a layout
engine and a GPU stack instead of borrowing the system's.

## Where this stands

The typing lag that prompted this is already fixed on the webview build:
measured through the renderer, a keystroke costs one mutation, a scope frame
one, and an idle screen zero. So this is no longer a rescue — it is a question
about whether a different stack is better, which wants someone to look at it on
a machine with a screen.
