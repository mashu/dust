#!/bin/bash
# Full verification of one checkout in its own target directory.
# Usage: verify.sh <checkout> <target-dir> <log-prefix>
set -u
DIR=$1
TARGET=$2
LOG=$3
cd "$DIR" || exit 1
export RUSTUP_TOOLCHAIN=1.99.1 CARGO_TARGET_DIR=$TARGET CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
echo "### HEAD $(git rev-parse --short HEAD) dirty=$(git status --porcelain | wc -l)"
echo "### fmt"
cargo fmt --all --check >/dev/null 2>&1 && echo FMT-OK || echo FMT-FAIL
echo "### cw-core"
cargo test -p cw-core --locked > "$LOG.cwcore" 2>&1; echo "exit $?"
grep -E "^test result|FAILED|panicked" "$LOG.cwcore" | head -20
echo "### desktop"
cargo test -p dust --features desktop --locked > "$LOG.desktop" 2>&1; echo "exit $?"
grep -E "^test result|FAILED|panicked" "$LOG.desktop" | head -20
echo "### clippy desktop"
cargo clippy --workspace --features desktop --all-targets --locked -- -D warnings > "$LOG.clippy-desktop" 2>&1; echo "exit $?"
tail -2 "$LOG.clippy-desktop"
echo "### clippy web"
cargo clippy -p dust --no-default-features --features web --target wasm32-unknown-unknown --locked -- -D warnings > "$LOG.clippy-web" 2>&1; echo "exit $?"
tail -2 "$LOG.clippy-web"
echo "### clippy silent"
cargo clippy -p dust --no-default-features --features mobile-silent --all-targets --locked -- -D warnings > "$LOG.clippy-silent" 2>&1; echo "exit $?"
tail -2 "$LOG.clippy-silent"
echo "### silent tests"
cargo test -p dust --no-default-features --features mobile-silent --locked > "$LOG.silent" 2>&1; echo "exit $?"
grep -E "^test result|FAILED|panicked" "$LOG.silent" | head -20
echo "### DONE"
