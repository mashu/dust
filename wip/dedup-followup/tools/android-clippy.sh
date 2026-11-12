#!/bin/bash
# Android type-check of one checkout, as CI's "Android type-check" job, with a stand-in C++ compiler.
# Usage: android-clippy.sh <checkout> <target-dir>
set -u
cd "$1" || exit 1
export RUSTUP_TOOLCHAIN=1.99.1 CARGO_TARGET_DIR=$2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CC_aarch64_linux_android=/home/user/wt/bin/fakecc CXX_aarch64_linux_android=/home/user/wt/bin/fakecc AR_aarch64_linux_android=ar
cargo clippy -p dust --no-default-features --features mobile --target aarch64-linux-android --locked -- -D warnings
