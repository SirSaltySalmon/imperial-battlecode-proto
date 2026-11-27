#!/usr/bin/env bash
# Run ibc-bench from the repo root, building it and the `ibc` runner first. All arguments are passed through.
#   tools/bench.sh ab rusher holder --seeds 20
# Games are fuel metered (wasm, through `ibc run-batch`) unless you pass --backend native (unmetered).
# The runner is rebuilt from this tree every time so its rules match ibc-bench's; set IBC_BIN to use another.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --quiet --release -p ibc-bench -p ibc-cli
bin="${CARGO_TARGET_DIR:-target}/release"
ibc="$bin/ibc"
[ -f "$ibc.exe" ] && ibc="$ibc.exe"
export IBC_BIN="${IBC_BIN:-$ibc}"
exec "$bin/ibc-bench" "$@"
