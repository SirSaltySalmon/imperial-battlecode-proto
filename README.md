# imperial-battlecode-proto

Concept build for Imperial Battlecode: a Battlecode-style competition platform and its game, a deterministic 2D
attackers-vs-defenders tactical shooter in which every unit runs its own bot.

- Writing a bot with the `ibc` kit (`ibc init`, `ibc new`): [docs/PARTICIPANTS.md](docs/PARTICIPANTS.md)
- The game: [docs/RULES.md](docs/RULES.md)
- What bots see and do: [docs/ABI.md](docs/ABI.md)
- How matches are dispatched to workers: [docs/JOBS.md](docs/JOBS.md)
- Working conventions (people and agents): [CLAUDE.md](CLAUDE.md)
- What is left to do: [docs/TODO.md](docs/TODO.md)

## Quick start

```
rustup target add wasm32-wasip1
cargo build --release -p ibc-cli
target/release/ibc run bots/rusher bots/holder --map maps/crossfire.ibcmap --seed 1 -o out/demo
target/release/ibc replay verify out/demo/replay.ibcr
cargo run --release -p ibc-sim -- league --maps maps --seeds 3     # native, no wasm
```

## How it fits together

Bots are WebAssembly modules metered by Wasmtime fuel, so a match is a pure function of (engine version, ruleset,
map, bots, seed) and gives the same result on any machine. That makes match jobs idempotent, cacheable and
verifiable, which is what lets one coordinator hand work to any mix of owned and rented machines.
