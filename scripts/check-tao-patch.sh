#!/usr/bin/env bash
# Fail unless every build's tao is the patched copy in vendor/tao, with
# its Android fix in.
#
# tao 0.34's Android event loop ran the app's queued events only when the
# looper returned a wake. With a lifecycle event ready at the same moment,
# the looper takes the wake itself and returns the event: the wake was
# lost, and the app's tasks stopped until something else woke the loop.
# Cargo.toml patches in vendor/tao, tao 0.34.8 with tao 0.37.0's fix,
# because dioxus 0.7 asks for tao ^0.34. Every build here is --locked, so
# Cargo.lock says which tao they build: once dioxus moves to another tao,
# the copy goes unused and Cargo.lock says that too. Then the patch and
# vendor/tao can go (docs/platforms.md, Android).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

failed=0
event_loop=vendor/tao/src/platform_impl/android/mod.rs

# The queue is drained once on every return from the looper, after the
# match on what it returned, not in the arm for a wake alone.
drains="$(grep -cF 'while let Ok(event) = self.receiver.try_recv()' "$event_loop" || true)"
if ! grep -qF 'Some(EventSource::User) | None => {}' "$event_loop" || [[ "$drains" != 1 ]]; then
  echo "$event_loop: the event loop drains its queue only on a wake; put tao 0.37.0's fix back." >&2
  failed=1
fi

if ! grep -qxF 'tao = { path = "vendor/tao" }' Cargo.toml; then
  echo "Cargo.toml: tao is not patched to vendor/tao." >&2
  failed=1
fi

# Each tao in Cargo.lock, by where it comes from: none for the copy.
sources="$(awk '
  /^\[\[package\]\]$/ { tao = 0; next }
  /^name = "tao"$/ { tao = 1; source[++n] = "vendor/tao"; next }
  tao && /^source = / { gsub(/"/, "", $3); source[n] = $3 }
  END { for (i = 1; i <= n; i++) print source[i] }
' Cargo.lock)"
if [[ -z "$sources" ]]; then
  echo "Cargo.lock: no tao at all." >&2
  failed=1
elif grep -vxF vendor/tao <<< "$sources" >&2; then
  echo "Cargo.lock: a tao is built from there, not from vendor/tao." >&2
  failed=1
fi
if grep -qxF '[[patch.unused]]' Cargo.lock; then
  echo "Cargo.lock: a patch is not used. With no tao 0.34 left to patch, remove the patch and vendor/tao (docs/platforms.md, Android)." >&2
  failed=1
fi

if ((failed)); then
  exit 1
fi
echo "tao is vendor/tao, with its Android fix."
