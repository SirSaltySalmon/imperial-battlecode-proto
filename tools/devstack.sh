#!/usr/bin/env bash
# The whole online loop on this machine: a coordinator, two workers, eight reference bots as teams, and the
# ranked scheduler playing round after round. Proof that games run online repeatedly, and a local server to
# point `ibc` and the web front end at.
#
#   tools/devstack.sh [PORT] [--rounds N] [--exit] [--interval SECS]
#
#   PORT          coordinator port on 127.0.0.1 (default 18016)
#   --rounds N    wait until every team has played N ranked series, then print the ladder (default 3)
#   --exit        stop everything after that; without it the stack keeps running until Ctrl-C
#   --interval S  seconds between ranked scheduler rounds (default 30; the scheduler sizes each round
#                 to what the workers can play in one interval, at a 2 core-seconds-per-game prior until
#                 it has measured games, so a much shorter interval starts with one series a round)
#
# Builds natively in debug (honours CARGO_TARGET_DIR). Starts from an empty out/devstack/ every time; logs
# and the data directory stay there afterwards. Needs bash, curl and the wasm32-wasip1 Rust target.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT=18016
ROUNDS=3
EXIT=0
INTERVAL=30
while [ $# -gt 0 ]; do
  case "$1" in
    --rounds) ROUNDS="$2"; shift 2 ;;
    --exit) EXIT=1; shift ;;
    --interval) INTERVAL="$2"; shift 2 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    [0-9]*) PORT="$1"; shift ;;
    *) echo "devstack: unknown argument $1 (see --help)" >&2; exit 2 ;;
  esac
done

BOTS=(rusher holder utility sapper retaker lurker saver forcer)
GAMES=2 # games per ranked series
OUT=out/devstack
URL="http://127.0.0.1:$PORT"
ADMIN=devstack-admin
TRUSTED=devstack-trusted
EXE=""
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) EXE=".exe" ;; esac
TARGET_BIN="${CARGO_TARGET_DIR:-target}/debug"
BIN="$OUT/bin"
IBC="$BIN/ibc$EXE"

log() { printf '[devstack] %s\n' "$*" >&2; }

PIDS=()
stop_all() {
  for pid in "${PIDS[@]}"; do
    # Windows: kill the whole tree (workers keep `ibc run-batch` children); elsewhere SIGTERM drains.
    if [ -r "/proc/$pid/winpid" ] && command -v taskkill >/dev/null 2>&1; then
      taskkill //T //F //PID "$(cat "/proc/$pid/winpid")" >/dev/null 2>&1 || true
    else
      kill "$pid" 2>/dev/null || true
    fi
  done
  wait 2>/dev/null || true
}
trap stop_all EXIT
trap 'exit 130' INT TERM

# --- JSON without jq: the coordinator's answers are flat enough for sed ---
str() { sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p" <<<"$2" | head -n1; }
num() { sed -n "s/.*\"$1\":\(-\{0,1\}[0-9][0-9.]*\).*/\1/p" <<<"$2" | head -n1; }
api() { # METHOD PATH [curl args...]: as the organisers
  local method="$1" path="$2"
  shift 2
  curl -fsS -X "$method" -H "Authorization: Bearer $ADMIN" "$URL$path" "$@"
}

rm -rf "$OUT"
mkdir -p "$OUT/bin"
# Run copies, not the target directory's binaries: another build sharing CARGO_TARGET_DIR may replace
# those while the stack is up.
log "building ibc, ibc-worker, ibc-coordinator (debug) into $OUT/bin"
cargo build -q -p ibc-cli -p ibc-worker -p ibc-coordinator &&
  cp "$TARGET_BIN/ibc$EXE" "$TARGET_BIN/ibc-worker$EXE" "$TARGET_BIN/ibc-coordinator$EXE" "$OUT/bin/"

for bot in "${BOTS[@]}"; do
  log "ibc build bots/$bot"
  "$IBC" build "bots/$bot" >"$OUT/build-$bot.log" 2>&1 || { cat "$OUT/build-$bot.log" >&2; exit 1; }
done

log "coordinator on $URL (data $OUT/coordinator, log $OUT/coordinator.log)"
IBC_DATA="$OUT/coordinator" IBC_LISTEN="127.0.0.1:$PORT" IBC_ADMIN_TOKEN="$ADMIN" \
  IBC_REGISTRATION_TOKEN=devstack-untrusted IBC_TRUSTED_REGISTRATION_TOKEN="$TRUSTED" \
  IBC_RANKED_INTERVAL_SECS="$INTERVAL" IBC_RANKED_GAMES="$GAMES" \
  "$BIN/ibc-coordinator$EXE" >"$OUT/coordinator.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 100); do
  curl -fsS "$URL/healthz" >/dev/null 2>&1 && break
  sleep 0.2
done
curl -fsS "$URL/healthz" >/dev/null || { cat "$OUT/coordinator.log" >&2; exit 1; }

VERSION="$("$IBC" version --json)"
api POST /v1/admin/patch -H 'Content-Type: application/json' \
  -d "{\"engine\":\"$(str engine "$VERSION")\",\"ruleset_hash\":\"$(str ruleset_hash "$VERSION")\",\"wasmtime\":\"$(str wasmtime "$VERSION")\"}" >/dev/null
MAPS=0
for map in maps/*.ibcmap; do
  api PUT "/v1/admin/maps/$(basename "$map" .ibcmap)" --data-binary "@$map" >/dev/null
  MAPS=$((MAPS + 1))
done
log "patch $(str engine "$VERSION")/$(str ruleset_hash "$VERSION"), $MAPS maps"

for bot in "${BOTS[@]}"; do
  team="$(api POST /v1/admin/teams -H 'Content-Type: application/json' -d "{\"name\":\"$bot\"}")"
  curl -fsS -X POST -H "Authorization: Bearer $(str token "$team")" --data-binary "@bots/$bot/.ibc/bot.wasm" \
    "$URL/v1/submissions?language=rust&live=true" >/dev/null
done
log "${#BOTS[@]} teams with live submissions: ${BOTS[*]}"

for i in 1 2; do
  "$BIN/ibc-worker$EXE" --coordinator "$URL" --registration-token "$TRUSTED" --name "devstack-$i" \
    --runner "$IBC" --cache "$OUT/worker-$i" --concurrency 2 --poll-ms 2000 \
    >"$OUT/worker-$i.log" 2>&1 &
  PIDS+=($!)
done
log "2 workers (logs $OUT/worker-*.log); ranked scheduler every ${INTERVAL}s, $GAMES games per series"

ladder() { curl -fsS "$URL/v1/ladder"; }
# Ranked games of the team that has played least (0 until every team is on the ladder).
fewest_games() {
  local body counts
  body="$(ladder)"
  counts="$(grep -o '"games":[0-9]*' <<<"$body" | cut -d: -f2 || true)"
  if [ "$(grep -c . <<<"$counts" || true)" -lt "${#BOTS[@]}" ]; then echo 0; return; fi
  sort -n <<<"$counts" | head -n1
}

print_ladder() {
  printf '\n%-5s %-12s %7s %-11s %6s %5s %7s %6s\n' rank team rating tier games wins losses draws
  ladder | tr '{' '\n' | { grep '"rank"' || true; } | while IFS= read -r row; do
    name="$(str name "$row")"
    [ -n "$name" ] || name="$(str team_name "$row")"
    rating="$(num rating "$row")"
    tier="$(str tier "$row")"
    # A provisional team is still placing its live submission: a rating but no tier yet.
    grep -q '"provisional":true' <<<"$row" && tier="Provisional"
    printf '%-5s %-12s %7s %-11s %6s %5s %7s %6s\n' "$(num rank "$row")" "$name" "${rating:--}" "${tier:--}" \
      "$(num games "$row")" "$(num wins "$row")" "$(num losses "$row")" "$(num draws "$row")"
  done
}

print_matches() {
  printf '\n%-9s %-10s %-10s %-10s %-6s %s\n' kind map "side A" "side B" score winner
  # One match per line: array elements are the only objects that follow "},".
  curl -fsS "$URL/v1/matches?limit=12" | awk '{ gsub(/\},\{/, "}\n{"); print }' |
    { grep '"match_id"' || true; } | while IFS= read -r row; do
      names="$(grep -o '"name":"[^"]*"' <<<"$row" | cut -d'"' -f4 || true)"
      a="$(sed -n 1p <<<"$names")"
      b="$(sed -n 2p <<<"$names")"
      score="$(sed -n 's/.*"score":\[\([0-9]*\),\([0-9]*\)\].*/\1-\2/p' <<<"$row")"
      case "$(str outcome "$row")" in a) winner="$a" ;; b) winner="$b" ;; *) winner="$(str outcome "$row")" ;; esac
      printf '%-9s %-10s %-10s %-10s %-6s %s\n' "$(str kind "$row")" "$(str map "$row")" "$a" "$b" \
        "${score:--}" "${winner:--}"
    done
}

WANT=$((ROUNDS * GAMES))
START=$SECONDS
LAST=-1
while :; do
  have="$(fewest_games)"
  [ "$have" -ge "$WANT" ] && break
  if [ $((SECONDS - START)) -gt 900 ]; then
    log "timed out after 900 s (fewest games $have of $WANT); see $OUT/*.log"
    exit 1
  fi
  if [ "$have" != "$LAST" ]; then
    log "every team has played at least $have of $WANT ranked games ($((SECONDS - START)) s)"
    LAST="$have"
  fi
  sleep 2
done
log "$ROUNDS ranked series per team played in $((SECONDS - START)) s"
print_ladder
print_matches
echo

if [ "$EXIT" = 1 ]; then
  log "stopping (--exit)"
  exit 0
fi
log "still running at $URL (admin token $ADMIN); Ctrl-C stops everything"
wait
