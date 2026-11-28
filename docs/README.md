# Docs

Start with the glossary: every page uses its words in exactly its sense.

## Hand-written

| Page | What it is |
|---|---|
| [GLOSSARY.md](GLOSSARY.md) | Exact definitions: tick, turn, round, game, series, slot, side, patch, kit, fog, ... |
| [RULES.md](RULES.md) | The game: match flow, a tick, vision, shooting, objective, economy, fuel |
| [rules/combat.md](rules/combat.md), [rules/economy.md](rules/economy.md), [rules/abilities.md](rules/abilities.md) | Mechanics in detail, with formulas and worked examples |
| [ABI.md](ABI.md) | What bots see and do: the conventions of the bot ABI |
| [JOBS.md](JOBS.md) | How matches are dispatched to workers |
| [release.md](release.md) | What a patch is and how one ships |
| [maps.md](maps.md) | The map pool and the map format |
| [hillclimb.md](hillclimb.md) | Measuring balance with `ibc-bench` |
| [TODO.md](TODO.md) | What is left to do |
| [DEPRECATED.md](DEPRECATED.md) | Features we dropped, one line each |
| [informal/](informal/) | The vision notes and the review of them, the weapons vision, and the design vision for the site and viewer |

## Generated

Written by `cargo xtask docs` from the sources of truth; never edit them by hand. CI fails a PR whose change to
a source leaves them stale (`cargo xtask docs --check`).

| Page | Generated from |
|---|---|
| [generated/rules.md](generated/rules.md) | `rules/*.toml` through the engine's ruleset loader: every number, with the ruleset hash |
| [generated/abi.md](generated/abi.md) | `crates/abi/src/lib.rs`: every id and constant, every record with offsets and sizes |
| [generated/llm.md](generated/llm.md) | One file for AI agents: the glossary, the rules, the ABI, both generated pages and a Rust SDK quick reference |
