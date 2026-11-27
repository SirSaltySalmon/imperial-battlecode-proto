#!/usr/bin/env bash
# The balance loop. Save a labelled report, change ONE thing in rules/*.toml, save another, compare.
#   tools/balance.sh run before            # writes out/balance/before/report.{md,json}
#   ... edit rules/*.toml ...
#   tools/balance.sh run after
#   tools/balance.sh compare before after  # lists the metrics that moved beyond their intervals
# Extra flags after the label go to `ibc-bench balance` (e.g. --seeds 10). Games are fuel metered (wasm);
# both ibc-bench and the ibc runner are rebuilt each run, so the new rules reach both.
set -euo pipefail
bench="$(dirname "$0")/bench.sh"
case "${1:-}" in
  run)
    [ $# -ge 2 ] || { echo "usage: tools/balance.sh run <label> [flags]" >&2; exit 1; }
    exec "$bench" balance --out "out/balance/$2" "${@:3}"
    ;;
  compare)
    [ $# -eq 3 ] || { echo "usage: tools/balance.sh compare <old-label> <new-label>" >&2; exit 1; }
    exec "$bench" balance --compare "out/balance/$2/report.json" "out/balance/$3/report.json"
    ;;
  *)
    echo "usage: tools/balance.sh run <label> [flags] | compare <old-label> <new-label>" >&2
    exit 1
    ;;
esac
