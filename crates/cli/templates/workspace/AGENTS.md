# Notes for coding agents

This is an Imperial Battlecode workspace: bots for a deterministic 2D tactical shooter. Each bot is a
directory under `bots/`, compiled to WebAssembly by `ibc build`. Read these before writing code:

- `docs/PARTICIPANTS.md`: the workflow (build, run, view, submit).
- `docs/RULES.md` and `docs/rules/`: the game.
- `docs/ABI.md`: what a bot sees and can do each tick, and the fuel each call costs.
- `docs/SDK-RUST.md` / `docs/SDK-CPP.md`: the SDK API. The SDK source is in `.ibc/sdk/rust` and
  `.ibc/include` (written by `ibc init`; do not edit, it is overwritten).

Check your work with `ibc build bots/NAME` and `ibc run bots/NAME [OPPONENT] --seed N`; the result line
and `out/run/` show what happened. Matches are deterministic: vary seeds and maps, not reruns.
