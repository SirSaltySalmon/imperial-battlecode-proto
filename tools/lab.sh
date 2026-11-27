#!/usr/bin/env bash
# Combat lab: records viewer/public/lab.json with ibc-combatviz (engine frames, the rules of this tree), serves the
# viewer with the Vite dev server and opens http://127.0.0.1:PORT/lab.html. Unless --no-play, the lab's Play section
# picks an opponent, a team bot and a map and starts `ibc play` with them on PORT+1 (viewer/scripts/lab-play.ts).
#
#   tools/lab.sh [PORT] [--no-play] [-- IBC_PLAY_OPTIONS...]
#   PORT          lab page port (default 5173); ibc play listens on PORT+1
#   --no-play     lab page only
#   -- OPTIONS    extra `ibc play` options for every match, e.g. -- --mode realtime --tps 8 (see `ibc play --help`)
# Ctrl+C stops everything.
set -euo pipefail
cd "$(dirname "$0")/.."
PORT=5173; PLAY=1; EXTRA=()
while [ $# -gt 0 ]; do
  case "$1" in
    --no-play) PLAY=0; shift ;;
    --) shift; EXTRA=("$@"); break ;;
    *) PORT="$1"; shift ;;
  esac
done

cargo run -q --release -p ibc-combatviz -- --out viewer/public/lab.json
(cd viewer && { [ -d node_modules ] || npm ci --silent; })

if [ "$PLAY" = 1 ]; then
  echo "[lab] building ibc and the viewer (ibc play serves its page from viewer/dist)"
  T="${CARGO_TARGET_DIR:-target}/release"
  if ! cargo build -q --release -p ibc-cli; then
    # Windows will not replace an ibc.exe that is running (another `ibc play`, say).
    if [ -f "$T/ibc.exe" ] || [ -f "$T/ibc" ]; then echo "[lab] ibc did not rebuild (above); using the last build"; else exit 1; fi
  fi
  (cd viewer && npm run -s build >/dev/null)
  mkdir -p out/lab-play/bin
  # A copy under its own name: a later cargo build cannot replace it, and stopping it hits nothing else.
  # A lab left over from a run whose exit trap never fired holds the copy open; stop it first.
  taskkill //F //IM ibc-lab.exe >/dev/null 2>&1 || pkill -x ibc-lab 2>/dev/null || true
  if [ -f "$T/ibc.exe" ]; then IBC=out/lab-play/bin/ibc-lab.exe; cp "$T/ibc.exe" "$IBC"; else IBC=out/lab-play/bin/ibc-lab; cp "$T/ibc" "$IBC"; fi
  export IBC_LAB_IBC="$PWD/$IBC" IBC_LAB_PLAY_PORT=$((PORT + 1))
  IBC_LAB_PLAY_ARGS="$(printf '%s\n' ${EXTRA[@]+"${EXTRA[@]}"})"
  export IBC_LAB_PLAY_ARGS
  trap 'taskkill //F //IM ibc-lab.exe >/dev/null 2>&1 || pkill -x ibc-lab 2>/dev/null || true' EXIT
fi

cd viewer
# A full URL, because Git Bash rewrites a bare /lab.html into a Windows path.
npx vite --host 127.0.0.1 --port "$PORT" --strictPort --open "http://127.0.0.1:$PORT/lab.html"
