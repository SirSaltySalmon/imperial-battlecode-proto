# Reference bots

The pool that balance is measured against (`docs/hillclimb.md`). Every bot here is registered in `ibc-sim`
(`crates/sim/src/lib.rs`), so `ibc-sim league` and `ibc-bench` can name it, and every Rust bot also builds
to wasm with `ibc build bots/<name>`.

| Bot | Attack | Defence | Spending | Kits and abilities |
|---|---|---|---|---|
| `idle` | Does nothing | Does nothing | Nothing | None |
| `rusher` | Everyone runs at the site nearest spawn | Runs at sounds and the attacker spawn, then sweeps the sites | Most expensive weapon it can afford | None |
| `holder` | Walks in silently, site agreed through team memory | Splits over the sites, holds angles, rotates to sightings | Most expensive weapon it can afford | None |
| `utility` | As `rusher` | As `rusher` | As `rusher`, plus ability charges | Drafts by slot, uses every ability on sight of an enemy |
| `sapper` | Runs at one site together, plants as soon as the carrier is covered | Holds the sites | Best affordable, a different weapon per slot | Yes |
| `retaker` | Walks in silently | One anchor per site, the rest stacked; gathers after a plant and retakes | Best affordable, rifles | Yes |
| `lurker` | Three hit the site, two walk round past another site | Holds the sites | Best affordable, a different weapon per slot | Yes |
| `saver` | As `sapper` | Holds the sites | Full buy or save | Yes |
| `forcer` | As `sapper` | Half the team holds the chokes in front of the sites and hunts sounds | SMG or shotgun and light armour every round | Yes |

`sapper`, `retaker`, `lurker`, `saver` and `forcer` are one implementation, `bots/squad`, with a different
`Style` each. A style names three decisions: how to attack, how to defend, how to spend. Everything else is
shared: shooting, the plant (the carrier plants under fire when a teammate is within five tiles), the
defuse (the defender nearest the charge defuses, the rest guard it), the draft and the abilities. To add a
bot with a new combination, copy one of the five crates (about twenty lines) and change its `STYLE`. To add
a new way to attack, defend or spend, add a variant in `bots/squad/src/lib.rs`.

What the squad bots do that the first three do not:

- **Vary with the seed.** The site to attack, the site to stack and the kit each slot drafts are a function
  of the match seed, the round and the team. Teammates agree without talking, and two seeds give two games.
  `rusher` still ignores the seed.
- **Play the charge.** Covered plants, post-plant positions with a line onto the charge, running at a
  defuse that is heard, grouped retakes that stop waiting when the fuse forces them.
- **Draft and use abilities for the objective.** Smoke on the line the enemy arrives on, or on the charge
  to defuse inside it; fire on a defuse in progress; a barrier in front of a plant or defuse; recon on the
  site before going in; flash, heal and dash as the fight asks. Kits are unique within a team.
- **Buy by what a weapon is,** not by its name: longest range, biggest magazine, most pellets, cheapest.
  The logic survives a change to `rules/weapons.toml`.
- **Stay inside the fuel budget.** The expensive questions (where does the enemy come into view of a site,
  how far is it) are asked one per buy tick, last in the turn, and kept for the match. Measured with the
  wasm runner: no out-of-fuel turns for the five squad bots on the pairings listed below. `holder` and
  `rusher` (so `utility`) do the same (measured below).

## Load bot: `burner`

Not in the pool and not registered in `ibc-sim` (natively there is no fuel meter, so it burns nothing). Every
unit spends its whole budget on every tick and yields about 250 fuel short of zero, so it is never paused.
`src/main.rs` does nothing else, like `idle`: the match then always plays all 18 rounds (2,305 ticks).
`examples/` holds `forcer` plus burning, `forcer` alone from the same sources (the control: same final state
hash), and kernels that spend the same fuel in costlier ways (`div`, `chase`, `fuel_calls`, `los`, `reads`,
`draws`, `mem_read`, `mem_write`, and `logs`, `prints`, `randoms` for `log`, WASI `fd_write` and
`random_get`). Build them with `cargo build --release --target wasm32-wasip1 --examples` in `bots/burner` and
pass the `.wasm` files to `ibc run`.

Measured on 2026-11-27, ruleset `5795cbc069627a93`, wasm runner, one match per process, i7-12700H:

| Mirror | Games | Core-seconds per game, mean / max |
|---|---:|---:|
| `burner` (multiply chain), 14 maps | 112 (56 distinct) | 0.53 / 0.66 |
| `forcer` plus burning, 14 maps | 112 (56 distinct) | 0.47 / 0.66 |
| `forcer` alone, 14 maps | 56 | 0.32 / 0.41 |
| `div` / `fuel_calls` / `los` / `reads`, 4 maps | 4 each | 1.1 / 2.0 / 5.2 / 5.3 (max 5.6) |
| `chase` (16 MiB per unit), 4 maps | 4 | 20.3 / 22.3 |
| `draws`, 4 maps | 4 | 74.3 / 76.6, and a 14 MB replay |
| `mem_read`, first 20 ticks only | 1 | 13.5 for 20 ticks (about 750 for a whole match) |

The fuel budget bounds CPU time only as well as host calls are priced. The same fuel costs 0.18 ns of CPU
per unit as arithmetic, 0.37 as division, 2 as `los` or table reads, 8 as cache misses, 28 as `debug_draw`
calls (free then) and about 290 as 1 MiB `mem_read` calls (charged 20 fuel whatever the length). `mem_write` was
worse: the engine recorded every call in the replay, refused or not, so one refused 1 MiB call per unit-turn
grew the runner's memory by about 23 MB a tick. Both are fixed: the engine checks a memory call before
anything of the requested length exists, and the replay keeps only what accepted calls left
(docs/REPLAY.md, "Inputs").

Pricing `debug_draw` (140 fuel, and 2,048 kept drawings per team and tick instead of 4,096 for the world),
`fd_write` (5 per iovec) and `random_get` (2 per 8 bytes), measured later that day on `crossfire`, seed 1,
with the binaries before and after run alternately (the machine was slower than above: table reads at about
4.5 ns per fuel, start-up of about 0.3 s included):

| Mirror | Before | After |
|---|---:|---:|
| `draws`, whole match | 84.3 core-s, 566 MB peak, 14.3 MB replay | 32.8 core-s, 628 MB, 20.9 MB |
| `prints` (64 one-byte iovecs per `fd_write`), 300 ticks | 19.2 core-s (53 ns per fuel) | 1.4 (4.0) |
| `randoms` (4 KiB per `random_get`) | 306 ms a tick (242 ns per fuel) | 4.5 ms (3.8) |
| `logs` / `reads` / `burner`, 300 ticks | 1.5 / 1.7 / 0.36 core-s | 1.6 / 1.6 / 0.33 |

`log` and `fuel_remaining` cost no more per fuel than a table read and stay free. What is left of `draws` is
storing the 4,096 drawings a tick that the cap keeps, about 3 µs of CPU each; the replay is larger because
drawings from several units compress worse than one unit's run. Only a lower cap, or a cheaper replay writer,
reduces that.

## Measured

Taken on 2026-11-25, ruleset `47ebd4e1efcddb9b`, native backend. The first table is on the ten maps the pool
had before it grew, so that the only thing that changed between the two columns is the bots. Intervals are
95%, computed by `ibc-bench` over distinct games.

```
ibc-bench balance --bots rusher,holder --maps <ten maps> --seeds 3
ibc-bench balance --bots rusher,holder,utility,sapper,retaker,lurker,saver,forcer --maps <ten maps> --seeds 3
```

| | `rusher`, `holder` | All eight (no `idle`) |
|---|---:|---:|
| Games (distinct) | 120 (78) | 1,920 (1,800) |
| Rounds | 1,493 | 20,609 |
| Charge planted | 48.3% [39.7, 56.9] | 69.3% [68.1, 70.6] |
| Round ends by elimination | 68.0% [60.9, 75.1] | 59.4% [58.1, 60.8] |
| Round ends by detonation | 19.8% [14.3, 25.3] | 11.6% [10.8, 12.4] |
| Round ends by defuse | 10.8% [6.4, 15.1] | 28.0% [26.8, 29.2] |
| Round ends on the clock | 1.4% [0.3, 2.5] | 1.0% [0.8, 1.2] |
| Attackers win rounds | 56.9% [48.6, 65.2] | 55.2% [53.7, 56.7] |
| Matches decided by tie-break | 29.2% [18.9, 39.4] | 7.9% [6.7, 9.2] |
| Kills with the marksman rifle | 64.8% | 23.2% |
| Kills by an ability | 0% | 9.2% |
| Failed `act` calls, worst bot | 1.6% | 0.1% |

League, same ten maps, three seeds, both slots (`ibc-sim league`, 420 games per bot):

| Bot | Match wins | Rounds won when attacking |
|---|---:|---:|
| `forcer` | 78.8% | 62.1% |
| `saver` | 60.5% | 63.5% |
| `rusher` | 58.3% | 46.1% |
| `retaker` | 56.9% | 63.6% |
| `sapper` | 50.5% | 62.5% |
| `lurker` | 45.0% | 57.4% |
| `utility` | 28.1% | 31.9% |
| `holder` | 21.9% | 53.0% |

What the numbers say, and what they do not:

- The objective now decides about four rounds in ten. Before, the two-bot pool mostly bought the marksman
  rifle and shot it out.
- `forcer` beats every other bot. It spends the least and defends the furthest forward. That is either a
  finding about cheap close-range weapons and forward defence, or a weakness of how the other bots hold a
  site. Read replays before changing a price.
- `holder` loses 84 to 92% of rounds when it defends against any bot that attacks as a group. It is the
  weak point of the pool, not evidence about defence in general.
- Mirror matches are still lopsided (attackers win 26.5% of `rusher` mirrors and 85.4% of `holder` mirrors),
  so a single bot against itself remains a poor balance test.
- None of this is a tuning result. It is the baseline the next rules change is measured against.

### After doubling plant and defuse (ruleset `5db90f2ad2ef9f28`)

`plant_ticks` 4 to 8, `defuse_ticks` 7 to 14, `defuse_kit_ticks` 4 to 8, nothing else. Same eight bots, same
ten maps, same seeds (`ibc-bench balance --compare`). 27 metrics moved beyond the noise; the main ones:

| | Before | After |
|---|---:|---:|
| Attackers win rounds | 55.2% [53.7, 56.7] | 68.2% [66.9, 69.5] |
| Round ends by defuse | 28.0% [26.8, 29.2] | 12.8% [12.1, 13.5] |
| Round ends by detonation | 11.6% [10.8, 12.4] | 14.5% [13.6, 15.4] |
| Round ends by elimination | 59.4% [58.1, 60.8] | 71.1% [70.0, 72.3] |
| Charge planted | 69.3% [68.1, 70.6] | 64.7% [63.3, 66.1] |
| Matches decided by tie-break | 7.9% [6.7, 9.2] | 9.8% [8.4, 11.2] |

Attackers gained on every map, most on `postern` (37% to 66%) and `agora` (45% to 69%). The fuse is still 40
ticks, so a 14-tick defuse leaves 26 ticks to reach the charge and clear it, where a 7-tick one left 33.
The bots were not retuned for the new numbers: they still gather and guard as if a defuse were quick, so
part of the swing is the bots and part is the rule. The fuse is the obvious next number to move.

### After moving to the vision's timings (ruleset `0304dfc448350cca`)

`plant_ticks` 12, `defuse_ticks` 21, `defuse_kit_ticks` 16, `fuse_ticks` 80, `buy_ticks` 6, and `max_ticks`
4000 so the longest match (3,745 ticks) still fits. Five numbers moved at once, at the designer's request, so
the table says what the set did and cannot say which number did it. Same eight bots, ten maps and seeds;
"before" is the doubled timings above.

| | Before | After |
|---|---:|---:|
| Attackers win rounds | 68.2% [66.9, 69.5] | 57.1% [55.5, 58.6] |
| Round ends by defuse | 12.8% [12.1, 13.5] | 19.6% [18.6, 20.5] |
| Round ends by detonation | 14.5% [13.6, 15.4] | 1.3% [1.1, 1.6] |
| Round ends by elimination | 71.1% [70.0, 72.3] | 77.1% [76.1, 78.1] |
| Round ends on the clock | 1.6% [1.3, 1.8] | 2.0% [1.7, 2.3] |
| Charge planted | 64.7% [63.3, 66.1] | 58.7% [57.3, 60.1] |
| Mean round length (ticks) | 84.4 | 96.0 |

The sides are back near even, but the charge almost never goes off: a planted round now ends by detonation
about one time in forty. Eighty ticks is long enough for the defenders to kill the attackers or defuse nearly
every time, and the post-plant attackers in `bots/squad` do not play for time (they hold a line onto the
charge and take every fight). The 12-tile blast that is meant to make a late defuse dangerous is not in the
engine yet. Before shortening the fuse, read some planted rounds in the viewer.

On all fourteen maps (2,688 games, 29,616 rounds) attackers win 56.1% [54.8, 57.4] of rounds; per map in
`docs/maps.md`. League there, two seeds: `forcer` 79.3% of matches, `saver` 59.4%, `rusher` 57.9%, `lurker`
49.5%, `sapper` 45.4%, `retaker` 45.2%, `utility` 36.5%, `holder` 26.8%.

Fuel on the new maps, wasm runner: 6 matches of the squad bots on `bazaar`, `delta`, `fulcrum` and `steppe`
gave one out-of-fuel turn in total, on `steppe`, the largest map, where one planning step uses the whole buy
tick. A forward defender used to start a path search for every sound it heard; it now keeps one place to go.

Fuel, wasm runner, 2026-11-27, ruleset `5795cbc069627a93`: `holder` used to plan the whole round in its first
buy tick and to recompute a post mid-round when it rotated. In a league of the whole pool on all fourteen
maps, both slots, seeds 1 to 3 (672 game sides) it ran out of fuel in 4,947 turns over 560 sides (seed 1:
883 of them in the buy phase, 756 in the action phase). It now spends one buy tick per question (its own
post, the other posts, then distances) and keeps the answers: same league, no out-of-fuel turn. Largest
single turn (seed 1): 353,027 of the 400,000 a buy tick allows (`steppe`), 98,397 of 100,000 in the action
phase, where one `Pathfinder::next_waypoint` call towards a new goal alone costs up to about 95,000. Same
league, Elo before and after: metered -157 [-210, -117] and -136 [-181, -96]; native (unmetered) -140
[-189, -98] and -134 [-180, -97], 28.7% and 29.2% of 672 matches won: inside the noise.

`rusher` searched the path from its spawn to every site, and from the chosen site to the enemy spawn, in
its first buy tick of every round, and a defender started a new path search for every sound it heard. Same
league: 9,046 out-of-fuel turns over 588 sides, and 11,057 over 602 for `utility`, which plays as `rusher`.
The site to rush is the same every round of a side, so the searches now run once per side, one per buy
tick, and a defender keeps one sound to chase until it is reached or stale. After: `rusher` 2 turns over
2 sides, `utility` 201 over 87, all in the action phase and all from the `next_waypoint` cost above
(`utility` adds its abilities to the same tick; `holder`, now facing the new `rusher`, has 2 turns over 1
side from the same cause). Largest buy-phase turn 387,912 of 400,000 (`steppe`).
Elo before and after, metered: `rusher` 77 [22, 129] and 88 [41, 131], `utility` -81 [-132, -33] and -75
[-127, -19]; native: `rusher` 77 [22, 130] and 89 [43, 133]. Inside the noise.

With the SDK's cheaper pathfinding as well (same day, ruleset `76bad4d5dc0354b6`, the 10-tile pistol): the same
league, 3,024 games, had no out-of-fuel turn, no fault and no failed game for any of the nine bots. Largest
single unit turn 164,154, so in a buy or draft tick (an action tick allows 100,000).

Fuel, wasm runner (measured before the timing change): 40 matches (every pair of the five squad bots on `agora`, `longwatch`, `tunnels` and
`warrens`, seed 11) had no out-of-fuel turn and no fault; the largest single turn was 297,188 of the
400,000 a buy tick allows. Native and wasm gave the same final state hash for `sapper` against `retaker`
on `crossfire`, seed 1, and that replay was produced twice byte-identical and verified.

### `holder` writes its site in the action phase (2026-11-27, ruleset `b884be2eabbb7c1e`)

`holder` used to write the attack site to team memory while planning on the first buy tick, where team
memory is read-only, so every attacker's write was refused every round. The first attacker to act on the
first action tick now writes it, and the others read it there. Every attacker computes the same site
from the seed and the round, so the games did not change. The native league (14 maps, 3 seeds, 3,024 games,
672 per bot) is identical before and after: `holder` wins 28.7% [25.4, 32.3] of matches and 43.7% of its
attacking rounds. On the wasm runner, where the extra memory calls cost fuel, `ibc replay dump` prints the
same state on every tick (team memory aside) for `holder` against `rusher` on `tunnels`, seed 1, both ways
round, and the goldens kept every score and length. Only the team memory part of the state hash moved. The
`holder`-first replay now holds 6 accepted site writes where it held 18 refused ones.
