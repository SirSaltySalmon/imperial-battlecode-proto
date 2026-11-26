#!/usr/bin/env bash
# Local platform for trying the website: builds everything, then runs a coordinator that serves the web app at
# http://127.0.0.1:18101/ and the viewer at /viewer/, plus one worker that plays the matches.
# Seeds the 14 maps and a sparring team "holder-ref" with a live bot. Data is wiped on every start.
#
#   tools/webstack.sh [--no-build]
#   --no-build    skip the build step on later runs (binaries, viewer and web app already built)
# Ctrl+C stops everything.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BUILD=1
for a in "$@"; do case "$a" in --no-build) BUILD=0 ;; *) REPO="$a" ;; esac; done
cd "$REPO"
PORT=18101; URL="http://127.0.0.1:$PORT"; D=out/stack; BIN=$D/bin; ADMIN=local-admin
T="${CARGO_TARGET_DIR:-target}"

if [ "$BUILD" = 1 ]; then
  echo "[stack] building ibc, coordinator and worker (release)"
  cargo build -q --release -p ibc-cli -p ibc-coordinator -p ibc-worker
  echo "[stack] building the engine wasm, the viewer and the web app"
  cargo build -q -p ibc-engine-wasm --release --target wasm32-unknown-unknown
  (cd viewer && npm ci --silent && npm run -s build >/dev/null)
  (cd web && npm ci --silent && npm run -s build >/dev/null)
fi
# Run copies so a later cargo build cannot replace a running binary.
rm -rf "$D"; mkdir -p "$BIN"
cp "$T/release/ibc.exe" "$T/release/ibc-coordinator.exe" "$T/release/ibc-worker.exe" "$BIN/"
IBC="$BIN/ibc.exe"
"$IBC" build bots/holder >/dev/null

PIDS=()
trap 'for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; taskkill //F //IM ibc-worker.exe >/dev/null 2>&1; taskkill //F //IM ibc-coordinator.exe >/dev/null 2>&1; echo "[stack] stopped"' EXIT

IBC_DATA=$D/data IBC_LISTEN=127.0.0.1:$PORT IBC_ADMIN_TOKEN=$ADMIN IBC_ADMIN_EMAILS=admin@example.test \
  IBC_REGISTRATION_TOKEN=local-untrusted IBC_TRUSTED_REGISTRATION_TOKEN=local-trusted \
  IBC_RANKED_INTERVAL_SECS=30 IBC_WEB_DIR=web/dist IBC_VIEWER_DIR=viewer/dist \
  "$BIN/ibc-coordinator.exe" > $D/coordinator.log 2>&1 &
PIDS+=($!)
for _ in $(seq 100); do curl -fsS $URL/healthz >/dev/null 2>&1 && break; sleep 0.2; done
curl -fsS $URL/healthz >/dev/null || { cat $D/coordinator.log; exit 1; }

api() { m=$1; p=$2; shift 2; curl -fsS -X "$m" -H "Authorization: Bearer $ADMIN" "$@" "$URL$p"; }
V="$("$IBC" version --json)"
g() { sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p" <<<"$V"; }
api POST /v1/admin/patch -H 'Content-Type: application/json' \
  -d "{\"engine\":\"$(g engine)\",\"ruleset_hash\":\"$(g ruleset_hash)\",\"wasmtime\":\"$(g wasmtime)\"}" >/dev/null
for m in maps/*.ibcmap; do api PUT "/v1/admin/maps/$(basename "$m" .ibcmap)" --data-binary "@$m" >/dev/null; done
TEAM="$(api POST /v1/admin/teams -H 'Content-Type: application/json' -d '{"name":"holder-ref"}')"
TOK="$(sed -n 's/.*"token":"\([^"]*\)".*/\1/p' <<<"$TEAM")"
curl -fsS -X POST -H "Authorization: Bearer $TOK" --data-binary @bots/holder/.ibc/bot.wasm \
  "$URL/v1/submissions?language=rust&live=true" >/dev/null

"$BIN/ibc-worker.exe" --coordinator $URL --registration-token local-trusted --name local-1 --runner "$IBC" \
  --cache $D/worker --concurrency 2 --poll-ms 1000 > $D/worker.log 2>&1 &
PIDS+=($!)

echo
echo "[stack] ready: open $URL/   (sign up as admin@example.test for the organisers' pages)"
echo "[stack] logs in $D/; Ctrl+C to stop"
wait
