#!/bin/bash
# The module-graph matrix of one checkout: src and cw-core, depths 1, 2 and 99, with --methods,
# under the default, lineage and strict rules. Prints one summary line each.
# Usage: graphs.sh <checkout> [<edges-out-file>]
set -u
T=/tmp/claude-0/-home-user-dust/e07ea342-050f-502a-b473-6fa881d75cc0/scratchpad/tools/modgraph2.py
cd "$1" || exit 1
for crate in src crates/cw-core/src; do
  for depth in 1 2 99; do
    for rule in "" --lineage --strict; do
      line=$(python3 $T $crate --depth $depth --methods $rule 2>&1 | grep -E "^depth $depth:" | head -1)
      echo "$crate d$depth ${rule:-default}: $line"
    done
  done
done
if [ $# -ge 2 ]; then
  python3 $T src --depth 2 --methods --strict --edges 2>/dev/null > "$2"
fi
