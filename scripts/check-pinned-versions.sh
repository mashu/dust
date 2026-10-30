#!/usr/bin/env bash
# Fail unless every pin of the Rust toolchain and of dx agrees.
#
# Each workflow installs the Rust in its RUST_TOOLCHAIN, never whatever
# stable is newest, so a lint a new Rust adds cannot turn CI red under
# -D warnings with no change here; moving to a new Rust changes them all
# at once. dx bundles with the Dioxus it was released with, so every
# DIOXUS_CLI_VERSION, `dioxus-cli --version` and `dx X.Y.Z` in the
# workflows, docs and scripts is the dioxus in Cargo.lock.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

failed=0
# Each `file:line:version` on stdin must be `want`.
agree() {
  local what=$1 want=$2 count=0 line
  while IFS= read -r line; do
    count=$((count + 1))
    if [[ "${line##*:}" != "$want" ]]; then
      echo "${line%:*}: $what ${line##*:}, not $want" >&2
      failed=1
    fi
  done
  if ((count == 0)); then
    echo "No $what pin found" >&2
    failed=1
  fi
}

dioxus="$(awk '
  /^name = "dioxus"$/ { found = 1; next }
  found && /^version = / { gsub(/"/, "", $3); print $3; exit }
' Cargo.lock)"
agree dx "${dioxus:-missing}" < <(
  grep -rnoE 'DIOXUS_CLI_VERSION: "[0-9.]+"|dioxus-cli --version [0-9.]+|dx [0-9]+\.[0-9]+\.[0-9]+' \
    .github/workflows README.md docs scripts | sed -E 's/^([^:]+:[0-9]+):.*[^0-9.]([0-9.]+)"?$/\1:\2/'
)

pins="$(grep -rnoE 'RUST_TOOLCHAIN: "[^"]*"' .github/workflows | sed -E 's/RUST_TOOLCHAIN: "([^"]*)"$/\1/')"
rust="${pins%%$'\n'*}"
agree Rust "${rust##*:}" <<< "$pins"
# A step that names its toolchain any other way is not pinned.
# shellcheck disable=SC2016 # the workflows' own ${{ }}, not the shell's
if grep -rnE 'dtolnay/rust-toolchain@(stable|beta|nightly)|^ +toolchain: ' .github/workflows |
  grep -v 'toolchain: \${{ env.RUST_TOOLCHAIN }}$' >&2; then
  echo "Install the Rust in RUST_TOOLCHAIN there." >&2
  failed=1
fi

if ((failed)); then
  exit 1
fi
echo "Rust ${rust##*:} and dx ${dioxus} in every pin."
