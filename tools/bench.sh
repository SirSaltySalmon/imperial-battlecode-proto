#!/usr/bin/env bash
# Run ibc-bench from the repo root, building it on first use. All arguments are passed through.
#   tools/bench.sh ab rusher holder --seeds 20
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --quiet --release -p ibc-bench -- "$@"
