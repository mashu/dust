#!/bin/bash
# The Windows probe of one checkout's working tree: in a scratch copy, every unix-only test
# (and its worker twin) is compiled out, as a Windows build leaves it out, and the desktop
# clippy runs over all targets, so a helper only unix tests use is found unused.
# Usage: winprobe.sh <checkout> <scratch-dir> <log>
set -u
SRC=$1; SCRATCH=$2; LOG=$3
PROBE=/tmp/claude-0/-home-user-dust/e07ea342-050f-502a-b473-6fa881d75cc0/scratchpad/s5winprobe-tools/probe.py
rm -rf "$SCRATCH"; mkdir -p "$SCRATCH"
(cd "$SRC" && git ls-files -co --exclude-standard -z | tar --null -T - -cf -) | (cd "$SCRATCH" && tar -xf -)
cd "$SCRATCH" || exit 1
files=$(grep -rl '#\[cfg(unix)\]\|#\[cfg(all(unix, feature = "native-runtime"))\]' src crates --include=*.rs \
  | while read -r f; do python3 - "$f" <<'PY' && echo "$f"
import re, sys
lines = open(sys.argv[1]).read().split("\n")
ok = any(lines[i].strip() == "#[cfg(test)]" and re.match(r'^\s*(pub(\([a-z]+\))? )?mod tests', lines[i + 1]) for i in range(len(lines) - 1))
sys.exit(0 if ok else 1)
PY
  done)
python3 -I "$PROBE" "$SCRATCH" all-test-code $files > "$LOG.flipped"
echo "flipped $(wc -l < "$LOG.flipped") unix-only items of the test code in: $(echo $files)"
export RUSTUP_TOOLCHAIN=1.99.1 CARGO_TARGET_DIR=/home/user/wt/targets/dr CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo clippy --workspace --features desktop --all-targets --locked -- -D warnings > "$LOG" 2>&1
echo "clippy exit $?"; tail -2 "$LOG"
