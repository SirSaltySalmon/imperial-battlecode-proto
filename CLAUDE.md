# Working in this repo

Imperial Battlecode: a Battlecode-style competition platform and its game, a deterministic 2D tactical shooter
played by bots. Read `docs/RULES.md` (the game), `docs/ABI.md` (what bots see) and `docs/JOBS.md` (how matches
are dispatched) before changing anything in those areas.
Use words as `docs/GLOSSARY.md` defines them; `docs/generated/` is written by `cargo xtask docs` (never by hand).

## Layout

| Path | What it is |
|---|---|
| `crates/abi` | The frozen bot ABI. Additive changes only, and only on purpose |
| `crates/engine` | The game. `world.rs`, `host.rs`, `lib.rs`, `fixed.rs`, `map.rs`, `rules.rs` are core |
| `crates/engine/src/{combat,abilities,economy}` | Mechanics behind fixed seam functions (marked `SEAM`) |
| `crates/replay` | Replay and summary formats, replay verification |
| `crates/runner` | Wasmtime host: fuel, suspend/resume, WASI shim |
| `crates/sim` | Native harness: bots linked as Rust, no wasmtime. Use it for fast iteration |
| `crates/cli` | The `ibc` binary |
| `sdk/`, `bots/`, `maps/`, `rules/` | SDKs, reference bots, map pool, the ruleset |

## Hard rules

- **Determinism.** In `crates/engine`: integers and fixed-point only, no `f32`/`f64`, no `HashMap` iteration, no
  `usize` in state or output, no clocks, no I/O, no randomness outside the seeded state. A match is a pure
  function of (engine version, ruleset, map, bots, seed). If two runs differ, that is a bug, not noise.
- **Real geometry.** Distances are Euclidean, aim is a free angle, hits are swept. Do not introduce tile-distance
  or 8-direction shortcuts into mechanics.
- **Rules are data.** A tunable number goes in `rules/*.toml`, not in code. Changing a rule changes the ruleset
  hash, which is expected; say so in the PR.
- **Every state field is hashed.** If you add state that affects simulation, add it to the relevant `hash`
  function, or replays will verify when they should not.
- **Stay in your lane.** Change mechanics inside their module and through the seams. If a seam is missing, say so
  rather than reaching into core files from a feature branch.
- **Running cost matters.** Bot fuel dominates match cost, then storage. Do not add per-tick work that scales
  with map area, do not store what can be recomputed from a replay, and keep always-on services to one binary.

## How to check your work

```
cargo test -p ibc-engine                                   # mechanics
cargo run -p ibc-sim --release -- league --maps maps --seeds 3   # native league, seconds
ibc run bots/rusher bots/holder --map maps/crossfire.ibcmap --seed 1 -o out/e2e
ibc replay verify out/e2e/replay.ibcr                      # re-simulates and compares state hashes
```

Run the same match twice and compare the replay files: they must be byte-identical.

## Changing the game (lessons carried over from our CAMBC post-mortem)

- **Look up the known method first.** Most problems here have names (line of sight, swept collision, rating
  systems, task allocation). Implement the published approach rather than inventing one.
- **Build general systems, not patches.** One movement path, one damage path, one vision test. A special case for
  one map or one weapon is a smell.
- **One change at a time.** Stack two balance changes and you cannot tell which one moved the numbers.
- **Know the noise.** Games are deterministic, so vary seeds, maps and sides. Report a win rate with its sample
  size and interval; do not act on differences inside it.
- **Do not tune against a mirror.** A change that beats a copy of the same bot proves little. Use the whole
  reference pool and both sides.
- **A loss might be a bug.** Read the replay and the logs before dropping an idea.
- **Delete what lost.** No dead switches and no commented-out code; git is the archive.
- **Write down what you measured.** A balance change comes with the league table before and after.

## Conventions

- Rust stable, `cargo fmt`, no new `unsafe` outside `crates/abi`, `crates/runner` and the SDK's import layer.
- Keep files under about 800 lines and functions under about 80; split by the decision the code makes.
- Text files are LF. Maps and rules are hashed from their parsed content, not their bytes.
- Bench and match output goes under `out/` (ignored), never the repo root.
