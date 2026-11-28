# Hill-climbing bots and balance

How to improve a bot, or change the game's balance, and know that the change is real. Written to be
followed literally, by a person or by an AI agent. The tool is `ibc-bench` (`crates/bench`); `tools/`
holds short wrappers. Commands are run from the repo root.

```
cargo build --release -p ibc-bench -p ibc-cli      # tools/bench.sh does this for you, every time
export IBC_BIN=target/release/ibc                  # the runner the wasm backend plays through
```

Every game is played the way a real match is: the bots' wasm builds, under the fuel budget, through `ibc
run-batch`. A built-in name such as `rusher` means `bots/rusher` built to wasm. Rebuild `ibc` together
with `ibc-bench` (the script does): when their rules differ, every report says so and the run is void.

`--backend native` is the exception, for quick iteration and debugging only. It links the built-in bots as
Rust and runs them with **unlimited fuel** (a native build cannot count wasm instructions), so a bot that
overruns its budget plays better there than it does for real. It is faster (see "What metering costs" at
the end) and shows no fuel. Every report it writes says `UNMETERED`; such numbers never go in a PR.

## The rules that do not bend

1. **A number without its interval is not a result.** Every report prints `estimate [95 % interval]` and
   the sample behind it. If the interval of a win rate contains 50 %, or two intervals overlap, you have
   learned nothing yet. The report says `INSIDE THE NOISE` or `NOT ESTABLISHED` in that case: believe it.
2. **Matches are deterministic, so the sample is maps x seeds x sides, not "runs".** Re-running the same
   command gives the same numbers. Evidence comes from more maps, more seeds and both slot orders.
3. **Exact repeats are counted once.** If the bots ignore the seed, ten seeds are one game played ten
   times (same map, bots and slots, and the same `digest` in the match summary, docs/REPLAY.md). Reports
   show `distinct` next to `played` and compute intervals from the distinct ones. When `distinct` stops
   growing, more seeds are wasted compute: add maps or opponents.
4. **One change at a time.** Two changes in one run cannot be told apart afterwards.
5. **A loss might be a bug.** Before dropping an idea, read a lost game with `ibc-bench why`.

## 1. Improving a bot

### Step 0: know the noise (once per pairing, again after big changes)

```
ibc-bench noise <your-bot> <baseline> --seeds 40
```

Read the table: for each number of games `N` it gives the standard deviation of the score and the 95 %
half-width with the paired design. A measured gap smaller than the half-width at your `N` is noise. The
second table says how many games it takes to detect a gap of 10/20/50/100/200 Elo. If it says "every pair
had the same result", this pairing has no seed noise on these maps and the open question is other maps
and other opponents, not more seeds.

### Step 1: paired A/B against the previous version

Keep a copy of the bot before your change (`cp -r bots/mybot bots/mybot-base`, or a `.wasm` of the old
build). Then:

```
ibc-bench ab bots/mybot bots/mybot-base --seeds 40 --sprt elo0=0 elo1=30 alpha=0.05 beta=0.05
# same thing: tools/ab.sh bots/mybot bots/mybot-base
```

Every (map, seed) is played twice with the slots swapped, so the map and the side cancel inside a pair.
With `--sprt` the run stops as soon as the evidence is in (or as soon as a batch adds no new distinct
pair), which is what keeps the compute bill down. Without `--sprt` all seeds are played.

How to read the output, in this order:

| Line | Meaning | Act on it? |
|---|---|---|
| `REAL DIFFERENCE: X is better` | paired interval excludes 50 % on at least 6 distinct pairs | yes |
| `NOT ESTABLISHED: X is ahead` | ahead, but fewer than 6 distinct pairs | no: add maps |
| `INSIDE THE NOISE` | the interval contains 50 % | no: more maps/seeds, or drop the idea |
| `SPRT ... AcceptH1` | the gain is at least `elo1` at the chosen error rates | yes |
| `SPRT ... AcceptH0` | the gain is not above `elo0` | the change does not help |
| `SPRT ... Continue` | seeds ran out first | undecided; do not round it up |

Then the supporting lines:

- **paired** `better/level/worse` counts pairs, not games; the **sign test p** is the chance of a split
  that lopsided between two equal bots. Below 0.05 agrees with a real difference.
- **rounds differential per game** moves earlier than the win rate does (a 7-6 win and a 7-0 win are both
  one win). If the verdict is `INSIDE THE NOISE` but the report adds that the round differential favours
  one side, that is a reason to run more games, not a result.
- **slot effect**: the score in the team A slot (attacks first) versus the team B slot. A large gap means
  side bias, which is a balance question, not a bot question.
- **per map**: a bot that gains on one map and loses on another has not improved, it has specialised.
- **WARNING** lines are part of the result. In particular "ran out of fuel" means the bot was cut short
  in some turns: its results depend on the budget, and its unmetered native numbers are optimistic.
  "UNMETERED" means the run used `--backend native`: re-run it without that flag before reporting it.

### Step 2: check it against the whole pool

Beating your own previous version is necessary, not sufficient (see traps). Run the league:

```
ibc-bench league --bots rusher,holder,bots/mybot,bots/mybot-base --seeds 5
```

Ratings are Bradley-Terry on the Elo scale with bootstrap intervals over (map, seed) blocks. The new
version must not rate below the old one, and its interval should sit above the old one's before you call
it stronger overall. Overlapping intervals mean the order is not established.

### Step 3: read the losses

`report.md` of `ab` lists the games your bot lost, with the path of each replay (`--discard-replays`
drops them). After a `--backend native` run, or to look at a built-in bot without fuel limits, re-create
the game natively (unmetered: it matches the wasm game only if no bot ran out of fuel in it):

```
ibc-bench record rusher holder --map maps/crossfire.ibcmap --seed 3 --out out/record/loss.ibcr
ibc-bench why out/record/loss.ibcr --round 4 --unit 2
ibc-bench dump out/record/loss.ibcr          # ticks.jsonl + rounds.csv for your own analysis
```

`why` prints, tick by tick, what the unit saw (enemies in its vision mask, with distance and state), what
it heard, every action it asked for and whether the engine **rejected** it (`ERR_NOT_READY`, ...), and what
hit it. A unit that stood still while seeing an enemy, or fired while reloading, is a bug, and a bug is
cheaper to fix than a strategy is to replace. Units of team A are `0..n-1`, team B `n..2n-1`; team A
attacks in the first half.

### Step 4: the PR

Paste into the PR: the exact commands, the `ab` summary block (verdict, W/D/L, paired line, rounds line,
SPRT line), the league table before and after, and any WARNING lines. If the verdict was not `REAL
DIFFERENCE` say so in those words; do not describe a gap inside the noise as an improvement.

## 2. Changing game balance

Rules are data (`rules/*.toml`) compiled into the binaries. Changing one changes the ruleset hash, which
every report prints.

```
tools/balance.sh run before                 # = ibc-bench balance --out out/balance/before
# edit ONE value in rules/*.toml
tools/balance.sh run after                  # cargo rebuilds ibc-bench and ibc with the new rules
tools/balance.sh compare before after
```

`balance` plays a league of the reference pool (mirrors included, the do-nothing bot excluded; `--bots`
overrides) and prints a one-screen summary; `report.md` has the full tables and `report.json` every number
with its interval in one flat `metrics` map. Per-round rates are computed per match and combined with a
cluster interval, because rounds inside one match are not independent.

What to look at:

| Metric | Healthy sign | Smell |
|---|---|---|
| `attacker_round_win_rate` (overall, per map) | interval contains or is near 50 % | a map far off: map or objective timing problem |
| attacker wins by pairing (report.md) | similar across pairings | one bot wins only as attacker: a strategy problem, not a side problem |
| `end_reason.*.share` | a mix of elimination, detonation, defuse, timeout | one reason near 100 %: the objective does not matter |
| `plant_rate`, `first_contact_ticks`, `first_kill_ticks`, `round_ticks` | fights start well before the timer | no contact, or rounds always at the timer |
| `kills.weapon.*.share`, `shots_per_kill.weapon.*`, loadouts | several weapons bought and killing | one weapon takes every kill |
| `kills.distance.*.share` | several bands populated | everything in one band |
| `weapon.*.kill_index` and its splits (below) | intervals around 1, each weapon ahead in its own niche | one weapon above 1 everywhere, against every armour, for every bot |
| `econ.*`, credits-by-round table | the loser can still buy something | snowball: winners rich, losers on sidearms |
| `draw_rate`, `tiebreak_rate` | low | high tie-break rate: matches are decided by the tie-break rule |
| `degenerate.*`, "always times out", "nobody ever shoots" | zero | any entry: a bot or a rule is broken |
| `fuel.*` | `out_of_fuel_turn_rate` near 0 | bots hitting the budget: results depend on the budget |

### Reading the per-weapon section

Kill share is not strength: it is carriers times kills per carrier. The "Weapons per carrier-round"
section of `report.md` (`weapon.<name>.*` in `report.json`) measures what a weapon is worth to whoever
carries it:

- A **carrier-round** is one unit holding the weapon when the action phase starts (a unit without a
  primary carries its sidearm). Every enemy kill, all damage dealt to enemies and every kill reward count
  for the weapon the killer carried *then*, whatever finished the kill (a marksman carrier who switches to
  the pistol still scores for the marksman; ability kills too). Friendly kills and the blast do not count.
- **`kill_index`** = the carriers' kills per carrier-round over all kills per unit-round. 1 is average, and
  the indices average to exactly 1 over carrier-rounds. 1.12 means a carrier of it kills 12 % more often
  than the average unit. `kills_per_carrier_round`, `damage_per_carrier_round` and
  `kill_reward_per_carrier_round` are the raw rates behind it (reward is what the engine paid: the
  `kill_reward` of the weapon that made the kill).
- **Splits compare like with like.** `kill_index.side.attack|defend` is each side against its own
  baseline; `kill_index.bot.<bot>` is the weapon inside one bot's games against that bot's own kills per
  unit-round, which separates the weapon from the bot that buys it (the strong bots buy the cheap guns);
  `kill_index.victim_armor.<class>` counts only kills of victims in that armour at action start ("none",
  else the cheapest armour item covering their points) against all such kills.
- **Distance** is the other attribution: `weapon.<name>.kill_distance_tiles.mean` and
  `weapon.<name>.kills.distance.<band>.share` describe enemy kills made *with* the weapon.
- Every number is a ratio of per-match sums with an interval clustered by match (the index's interval is
  the delta method for a ratio of two ratios). `n` is the number of games in which somebody carried the
  weapon. Cells under 100 games are printed with `n=`: splits by bot, side and armour shrink fast, so read
  them as hints and do not act inside the interval.
- Not measured yet: round type (eco, force, full buy), unit slot, and duels by weapon pair and distance
  band. The pool's weapon choice is code, not choice: a weapon held mostly by weak bots looks weak.

These metrics need details recorded by this version of `ibc-bench` (`games.jsonl` details carry a
`version`). An older `games.jsonl` still loads; its games are left out of the section, and when none has the
attribution the section says "Unavailable" and the report carries a warning. `--compare` diffs the
`weapon.*` metrics like any other; a report older than schema 2 simply lists them as "only in NEW".

`--compare` prints three groups: **MOVED beyond the noise** (the difference exceeds both intervals
combined: these are the effects of your change), "changed, but no interval" (cannot be judged), and a
count of metrics that differ within the noise (ignore those). If nothing is listed as moved, the change did
nothing measurable at this sample size; say that, or run more maps/seeds on both sides. If the header
warns that the runs are "not like for like" (different bots, maps, seeds or backend), the comparison is
void: re-run both sides with the same flags.

The balance PR contains: the rule changed and why, the old and new ruleset hash, the `--compare` output,
the summary block of both runs, and the league table before and after (`ibc-bench league`). Remember that
a balance report describes the game *as the current bots play it*. A lopsided number can be the bots'
fault; check the pairing table and a replay before blaming a rule.

To use competitors' real bots, add `--bots bots/a,bots/b,...` (bot directories or `.wasm` files). After a
rules change rebuild `ibc` as well as `ibc-bench` (`tools/balance.sh` does both): if the runner's ruleset
differs from ibc-bench's, the report carries a warning and the run is not valid.

## Common traps

- **Mirror matches.** A bot against a copy of itself measures side and slot bias, nothing about strength.
  `ab` warns when both sides are the same bot.
- **Self-play overfitting.** Tuning version N+1 only against version N finds whatever beats version N,
  including its bugs. Always finish with the league against the whole pool, both slots.
- **Stacking changes.** Two rule edits, or a rule edit plus a bot edit, in one before/after cannot be
  attributed. One change, one comparison.
- **Reading wins when rounds are steadier.** With few games the win rate moves in steps of whole matches.
  Use the round differential to decide whether more games are worth it, and the paired interval to decide
  whether the result is real.
- **Counting seeds as evidence.** Check `distinct`. Reference bots barely use the seed, so most seeds
  repeat; only maps, opponents and sides add information there.
- **Unmetered numbers.** `--backend native` and `ibc-sim` do not meter fuel. A bot that runs out of fuel
  plays differently as wasm; when the two disagree for the same (map, seed, slots), look at
  `out_of_fuel_turns` first. Iterate natively if you like, but every number you report comes from the
  default wasm backend.
- **Stopping when it looks good.** Without `--sprt`, decide the seed count before running and do not
  stop early because the number is pleasing. `--sprt` exists precisely to make early stopping honest.
- **Comparing across rulesets or runner builds.** Check the ruleset hash at the top of both reports.
- **Tuning the game to the bots.** Do not "fix" a 60 % attacker rate that comes from one bot never
  defending a site. Fix or replace the bot, then measure again.

## What metering costs

Wall time on a 20-thread Windows desktop (`--jobs 20`, the default), 2026-11-27, while other builds and
leagues shared the machine, so read these as ratios, not benchmarks:

| Run | native (unmetered) | wasm (metered) |
|---|---:|---:|
| `league --seeds 3`, whole pool, 14 maps (3,024 games) | 6 to 12 s | 51 to 58 s with `.wasm` files, 69 to 75 s naming the bots |
| `balance --seeds 3` (2,688 games, per-tick details) | 7 to 15 s | 78 to 88 s |
| first `ibc build` of the nine reference bots | | 159 s, once |

Metered matches cost five to eight times as much as native ones. Naming a built-in bot runs `ibc build
bots/<name>` on every run (about 3.5 s per bot when nothing changed, all bots at once, longer while other
cargo commands hold cargo's locks); pass the `.wasm` files (`bots/<name>/.ibc/bot.wasm`) to skip that in
a loop. The wasm
backend runs one `ibc run-batch --jsonl --jobs N`: on this machine one process with 20 threads played
35.7 matches a second, against 31.9 for four processes of five threads and 14.8 for twenty processes of
one (each process compiles every module), so the in-process slowdown noted in `docs/TODO.md` did not show
here. `balance` re-simulates every replay for its details on the same N threads.
