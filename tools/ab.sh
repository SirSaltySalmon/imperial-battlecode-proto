#!/usr/bin/env bash
# Paired A/B of a candidate bot against a baseline, with early stopping.
#   tools/ab.sh <candidate> <baseline> [extra ibc-bench ab flags]
# Bots are built-in names (played as bots/<name> built to wasm), bot directories or .wasm files (paths
# relative to the repo root). Games are fuel metered; --backend native is an unmetered quick look only.
# Prints the verdict; exit 2 = some games failed.
set -euo pipefail
if [ $# -lt 2 ]; then
  echo "usage: tools/ab.sh <candidate> <baseline> [ibc-bench ab flags]" >&2
  exit 1
fi
exec "$(dirname "$0")/bench.sh" ab "$1" "$2" --seeds 40 --sprt elo0=0 elo1=30 alpha=0.05 beta=0.05 "${@:3}"
