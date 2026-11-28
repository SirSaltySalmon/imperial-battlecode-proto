# tools

Thin wrappers around `ibc-bench` (`crates/bench`). They only fix the working directory and a few defaults;
everything real lives in the binary. The workflow they serve is in [`docs/hillclimb.md`](../docs/hillclimb.md).

| Script | What it does |
|---|---|
| `tools/bench.sh <args>` | `cargo run --release -p ibc-bench -- <args>` from the repo root |
| `tools/ab.sh <candidate> <baseline>` | paired A/B, up to 40 seeds, stops early by SPRT (`elo0=0 elo1=30`) |
| `tools/loadout.sh <base> <slot> <arm>` | a squad bot with one slot's weapon forced, under `out/loadout/`, for a loadout A/B (`bots/README.md`) |
| `tools/balance.sh run <label>` | balance report into `out/balance/<label>/` |
| `tools/balance.sh compare <old> <new>` | which metrics moved beyond their intervals between two labels |
| `tools/devstack.sh [PORT] [--rounds N] [--exit]` | the online loop locally: coordinator + two workers + the eight reference bots as teams, ranked scheduler on; prints the ladder after N rounds (`docs/DEPLOY.md`) |
| `tools/lab.sh [PORT] [--no-play] [-- OPTIONS]` | combat lab: records `viewer/public/lab.json` (`ibc-combatviz`), serves the viewer with Vite on `PORT` (default 5173) and opens `/lab.html`. Its Play section picks the opponent, the team bot, the map (`tools/lab/range.ibcmap` is 1v1) and your kit and starts `ibc play` with them on `PORT+1`; `OPTIONS` go to every `ibc play`. Rerun after a rule or combat change |

## `ibc-bench` in one screen

```
ibc-bench ab <A> <B>        paired A/B: W/D/L, Wilson, paired interval, sign test, per map, optional SPRT
ibc-bench league            round robin, cross table, Bradley-Terry ratings with bootstrap intervals
ibc-bench balance           designer's report from a league run; --compare OLD.json NEW.json diffs two
ibc-bench noise <A> <B>     sd of the win rate as a function of N; games needed to detect an Elo gap
ibc-bench record <A> <B>    replay of one native game (built-in bots, unmetered), for `why` and `dump`
ibc-bench dump <replay>     per-tick JSONL + per-round CSV
ibc-bench why <replay> --round R --unit U    what that unit saw, did and suffered
```

A bot is a built-in name (`rusher`), a bot directory (`bots/rusher`) or a `.wasm` file. Games run on the
**wasm** backend by default: `ibc run-batch --jsonl` under the fuel budget, as a real match is played, and a
built-in name means `bots/<name>` built to wasm. Point it at the runner with `--ibc <path>` or `IBC_BIN`;
`tools/bench.sh` builds the runner and sets it. `--backend native` links the built-in bots as Rust through
`ibc-sim`: faster, but UNMETERED (unlimited fuel), and every report it writes says so. Report wasm results.

Output goes to `out/<command>/<utc stamp>/` (override with `--out`): `report.md` for people, `report.json`
for programs (each has a `schema` field), `games.jsonl` with one record per game. Wasm runs also keep
`games/g<id>/replay.ibcr`; pass `--discard-replays` to save disk. Reports contain no clock times or
absolute paths, so the same inputs give the same `report.json`.

`balance` (report schema `ibc-bench/balance/2`) also reports each weapon per **carrier-round** (a unit
holding it when the action phase starts): the **kill index** (its carriers' kills per carrier-round over
all kills per unit-round; 1 is average), split by side, by bot and by the victim's armour; kills, damage
and kill-reward income per carrier-round; and the distance of kills made with it. Keys are
`weapon.<name>.*`; how to read them is in [`docs/hillclimb.md`](../docs/hillclimb.md#reading-the-per-weapon-section).
`balance --from` an older `games.jsonl` still works; its per-weapon section reads "Unavailable".

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
