# Participant quickstart

Everything runs through one binary, `ibc`. A match is a pure function of (patch, map, bots, seed), so a game
you play locally is the game the servers would play.

## 1. Install

```sh
pip install imperial-battlecode            # puts `ibc` on PATH; or download a release binary
ibc version                                # the patch: engine, ruleset hash, wasmtime, ABI
```

Rust bots need Rust and the wasm target: `rustup target add wasm32-wasip1`.
C++ bots need wasi-sdk, which `ibc` fetches for you: `ibc toolchain install` (then `ibc toolchain status`).

## 2. Make a workspace

```sh
ibc init mybots          # or `ibc init` in an empty directory; --lang cpp for a C++ starter bot
cd mybots
```

| Path | What it is | Yours to edit? |
|---|---|---|
| `bots/` | Your bots, one directory each. `ibc init` adds `bots/starter` to an empty workspace | Yes |
| `maps/` | The map pool of this patch. Add your own `.ibcmap` files here | Yes |
| `ibc.toml` | The ibc version this workspace is pinned to, and `ibc run` defaults (map, opponent) | Yes |
| `docs/` | Rules, ABI, SDK reference, this page | Refreshed by `ibc init` |
| `rules/` | The ruleset of this patch, for reading (the engine uses its built-in copy) | Refreshed by `ibc init` |
| `.ibc/` | The SDK the bots build against (Rust crates, C++ headers), from the `ibc` binary itself | No: refreshed by `ibc init` |

After updating `ibc`, run `ibc init` again in the workspace: it refreshes `.ibc/` and `docs/`, adds new maps,
moves the version pin, and leaves your bots and custom maps alone. `ibc build` warns when the workspace and
the binary disagree. `.ibc/` is in `.gitignore`; a teammate who clones the workspace runs `ibc init` once.

## 3. Write a bot

```sh
ibc new mybot                              # Rust starter: bots/mybot/src/lib.rs
ibc new mybot2 --lang cpp                  # C++ starter: bots/mybot2/src/main.cpp
ibc new ref --template example             # a complete reference bot to read or start from
```

Every unit runs its own copy of your bot. Each tick it reads its fogged view and queues actions; read every
number from the rules tables instead of hard-coding it. Start with `docs/RULES.md` (the game), then
`docs/SDK-RUST.md` or `docs/SDK-CPP.md` (the API) and `docs/ABI.md` (the exact contract and fuel costs).

## 4. Build, run, watch

```sh
ibc build bots/mybot                       # -> bots/mybot/.ibc/bot.wasm
ibc run bots/mybot                         # against `opponent` on `map` from ibc.toml
ibc run bots/mybot bots/starter --map maps/tunnels.ibcmap --seed 7 -o out/t7
ibc view out/t7/replay.ibcr                # replay viewer on localhost
```

`ibc run` writes `replay.ibcr`, `summary.json` and each team's log under `-o` (default `out/run`). Games are
deterministic: rerunning the same seed proves nothing, so compare across seeds, maps and both sides.
`ibc replay verify FILE` re-simulates a replay and checks every state hash.

## 5. Submit

Once the platform commands are in your `ibc` (`ibc --help` lists them): sign in with `ibc login`, upload with
`ibc submit bots/mybot`, and pick which upload plays ranked games in `ibc submissions`. The server compiles
nothing: it runs the same `bot.wasm` you tested.

## Working with an AI agent

`ibc init` writes `AGENTS.md`, which points an agent at the docs and the SDK source in `.ibc/`. Give the agent
the whole `docs/` folder (rules, ABI, SDK reference); everything there matches your `ibc` version because it
came out of the same binary. Ask it to check its work with `ibc build` and a few `ibc run` seeds, and to read
the team logs in the run's output directory when a bot misbehaves.
