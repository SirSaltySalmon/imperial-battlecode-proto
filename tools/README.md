# tools

Thin wrappers around `ibc-bench` (`crates/bench`). They only fix the working directory and a few defaults;
everything real lives in the binary. The workflow they serve is in [`docs/hillclimb.md`](../docs/hillclimb.md).

| Script | What it does |
|---|---|
| `tools/bench.sh <args>` | `cargo run --release -p ibc-bench -- <args>` from the repo root |
| `tools/ab.sh <candidate> <baseline>` | paired A/B, up to 40 seeds, stops early by SPRT (`elo0=0 elo1=30`) |
| `tools/balance.sh run <label>` | balance report into `out/balance/<label>/` |
| `tools/balance.sh compare <old> <new>` | which metrics moved beyond their intervals between two labels |

## `ibc-bench` in one screen

```
ibc-bench ab <A> <B>        paired A/B: W/D/L, Wilson, paired interval, sign test, per map, optional SPRT
ibc-bench league            round robin, cross table, Bradley-Terry ratings with bootstrap intervals
ibc-bench balance           designer's report from a league run; --compare OLD.json NEW.json diffs two
ibc-bench noise <A> <B>     sd of the win rate as a function of N; games needed to detect an Elo gap
ibc-bench record <A> <B>    replay of one native game (built-in bots), for `why` and `dump`
ibc-bench dump <replay>     per-tick JSONL + per-round CSV
ibc-bench why <replay> --round R --unit U    what that unit saw, did and suffered
```

A bot is a built-in name (`rusher`), a bot directory (`bots/rusher`) or a `.wasm` file. Built-in names run
on the **native** backend (through `ibc-sim`, no wasm, fastest). Paths run on the **wasm** backend, which
shells out to `ibc run-batch --jsonl`; point it at the runner with `--ibc <path>` or `IBC_BIN`. With
`--backend wasm` a built-in name means `bots/<name>` built to wasm.

Output goes to `out/<command>/<utc stamp>/` (override with `--out`): `report.md` for people, `report.json`
for programs (each has a `schema` field), `games.jsonl` with one record per game. Wasm runs also keep
`games/g<id>/replay.ibcr`; pass `--discard-replays` to save disk. Reports contain no clock times or
absolute paths, so the same inputs give the same `report.json`.

Exit code: 0 ok, 1 usage or setup error, 2 the run finished but some games failed (they are listed on
stderr and excluded from the statistics).

## Files written by `dump`

`ticks.jsonl`, one object per tick:

| key | meaning |
|---|---|
| `t`, `r`, `ph`, `left` | tick, round, phase (`PHASE_*`), ticks left in the phase |
| `score`, `atk` | rounds won `[A, B]`, attacking team |
| `charge` | `[state, x, y]` |
| `u` | per unit `[id, alive, x, y, facing, hp, armor, weapon, ammo, credits, state bits, seen mask]` |
| `p` | per projectile `[owner, weapon, x, y]` |
| `ev` | per event `[kind, actor, target, x, y, value]` (`EVENT_*`) |

`rounds.csv`: `round, attackers, winner, reason, ticks, planted, first_contact, first_kill, shots, kills,
damage, moved_tiles, atk_credits, def_credits, atk_spent, def_spent`.
