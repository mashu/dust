#!/bin/bash
# Broken intra-doc links of one crate, one per line (sorted), for comparing before and after a move.
# Usage: doclinks.sh <checkout> <target-dir> core|app
# Baselines at a50e8eb: /home/user/wt/logs/doclinks-base-core.txt and doclinks-base-app.txt.
set -u
cd "$1" || exit 1
export RUSTUP_TOOLCHAIN=1.99.1 CARGO_TARGET_DIR=$2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
if [ "$3" = core ]; then
  cargo doc -p cw-core --no-deps --document-private-items --locked > "$2/doclinks.log" 2>&1
else
  cargo doc -p dust --no-deps --document-private-items --features desktop --locked > "$2/doclinks.log" 2>&1
fi
echo "cargo doc exit $?" >&2
grep -E "warning: (unresolved link to|.* is both )|ambiguous link" "$2/doclinks.log" | sed -E 's/^\s+//' | sort | uniq -c
