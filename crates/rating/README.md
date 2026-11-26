# ibc-rating

Ratings, ladder matchmaking, series plans and the match budget governor. A pure library: no I/O, no clocks,
no async, no dependencies (serde derives behind the `serde` feature). Every random choice comes from an
explicit seed and a built-in SplitMix64, so a batch can be replayed from the seed stored with it.

Floating point is used here (unlike the engine): ratings are not part of a match's determinism contract. The
results are bit-reproducible on one platform; `exp`/`powf` may differ in the last bit across platforms, so
ratings should be computed in one place (the coordinator) and stored, not recomputed elsewhere and compared.

## What the coordinator calls

```rust
use ibc_rating::*;

let system = WengLin::default();                 // or Elo::default(); both implement RatingSystem
let mm = MatchmakingConfig::default();
let budget = BudgetConfig::default();

// Every ranked batch (default: 6 per hour)
let n = series_per_batch(core_seconds_per_hour, measured_mean_cost /* Option<f64>, None until measured */, &budget);
let pairings: Vec<Pairing> = plan_batch(&system, &teams, now, n, &mm, batch_seed);
for p in pairings {
    let plan: SeriesPlan = plan_series(series_id, p, &map_usage, budget.games_per_series);
    for g in &plan.games {
        // queue a match job: bot_a = g.slot_a's bot, bot_b = g.slot_b's bot, map pool[g.map], seed g.seed
        map_usage[g.map as usize] += 1;
    }
}

// Every finished game. `outcome` is the job's "a" | "b" | "draw" | "unfinished" (docs/JOBS.md).
// Use it, not `winner`: an unfinished game also has `winner: null` and must not be rated.
if let Some(w) = Winner::from_job_outcome(outcome) {
    outcomes.push(plan.result(game_index, w).unwrap().outcome);
}
// When the series is over: one update from the pre-series ratings.
let (a, b) = rate_series(&system, &rating_of[plan.team_a], &rating_of[plan.team_b], &outcomes);

// Placement: a team with a new submission, between batches, one series at a time
if let Some(opponent) = plan_placement(&system, &teams[newcomer], &teams, &mm, seed) { /* plan_series as above */ }

// Events
rating = system.on_new_submission(&rating);      // team uploads a new bot
rating = system.on_balance_patch(&rating, 1.5);  // for every team, when the ruleset changes
rating = system.on_inactive(&rating, periods);   // optional, per idle rating period

// Leaderboard
let shown = system.display(&rating);             // 1500-centred
let provisional = system.is_provisional(&rating);
```

Types (all `Clone + Debug`, serde with the feature):

| Type | Fields |
|---|---|
| `TeamId` | `u64` |
| `Rating` | `mu: f64, sigma: f64, games: u32` |
| `TeamState` | `id, rating: Rating, recent_games: u32, last_played: Option<u64>, recent_opponents: Vec<TeamId>, live: bool` |
| `Winner` | `SlotA \| SlotB \| Draw` (by engine slot; `Winner::from_job_outcome(&str)`) |
| `Outcome` | `AWins \| BWins \| Draw` (seen from the series' team A, not the engine slot) |
| `GameResult` | `series_id: u64, game_index: u32, team_a, team_b, outcome` |
| `Pairing` | `team_a, team_b` |
| `SeriesPlan` | `series_id, team_a, team_b, games: Vec<PlannedGame>` |
| `PlannedGame` | `index: u32, map: u32, seed: u64, slot_a: TeamId, slot_b: TeamId` |

`TeamState::last_played` and `now` share any monotone unit (seconds or batch number). `recent_opponents` lists
the opponents of the team's series in the last day, oldest first, one entry per series. `recent_games` is the
number of rated games in the same window. Ratings are updated **per series** (`rate_series`): every map's
update is computed from the pre-series ratings and the mean shifts are summed (a drawn map is a half), so
map order does not matter and a 3-0 moves more than a 2-1. Uncertainty shrinks as after that many games
(its step does not depend on who won). `apply_game` remains for per-game simulations.

Store with each rating the `RatingSystem::name()` that produced it. `mu`/`sigma` of Elo and Weng-Lin are not
interchangeable.

## Rating systems

### Weng-Lin (recommended)

Weng & Lin, *A Bayesian Approximation Method for Online Ranking*, JMLR 12 (2011), Algorithm 1
(Bradley-Terry, full pair), for two teams of one rated entity. Each team's skill is a belief `N(mu, sigma^2)`.
For team `i` against `q`, with `s` = 1, 1/2, 0 for a win, draw, loss:

```
sigma_i^2 <- sigma_i^2 + tau^2                      (OpenSkill's additive dynamics, not in the paper)
c     = sqrt(sigma_i^2 + sigma_q^2 + 2 beta^2)                               eq. (49)
p_iq  = 1 / (1 + exp((mu_q - mu_i) / c))                                     eq. (49)
delta = sigma_i^2 / c * (s - p_iq)                                           eq. (50)
eta   = gamma * (sigma_i / c)^2 * p_iq * (1 - p_iq),   gamma = sigma_i / c   eq. (50), section 6.1
mu_i      <- mu_i + delta                                                    step 3.2
sigma_i^2 <- sigma_i^2 * max(1 - eta, kappa)                                 step 3.2, eq. (28)
```

Both sides are computed from the pre-game values. Draws use `s = 1/2`, the tie model of eq. (51).

| Default | Value | Why |
|---|---|---|
| `mu0`, `sigma0` | 25, 25/3 | paper section 6.1 (TrueSkill's convention) |
| `beta` | 25/6 | paper section 6.1 |
| `kappa` | 0.0001 | paper section 6.1 |
| `tau` | 25/300 per game | OpenSkill default; keeps sigma from collapsing so a rating can follow a field that changes. Bots only change by upload (handled below), but lowering `tau` to 25/600, 25/1200 or 0 moved the mean rank correlation over batches 100..200 by less than 0.003, with or without a 5-point-per-batch drift of every team's skill, so it stays |
| `inactivity_tau` | 25/300 per idle period | `sigma^2 += periods * tau^2`, capped at `sigma0`; only if the coordinator calls `on_inactive` |
| resubmission | `sigma = max(sigma, 0.85 sigma0)`, mu kept | a new bot can be a different bot: the old estimate is a starting point, not much more (measured below) |
| balance patch | `sigma = min(sigma * factor, sigma0)` | everyone's bot is now playing a slightly different game |
| provisional | `sigma > 0.6 sigma0` (= 5.0) | about 25 games in the simulation below; a resubmission makes a team provisional again until it has played. The coordinator's ladder shows "placing" by series count instead; this flag orders tournament seeds |

**Display rating.** `display = 1500 + k * ((mu - z sigma) - mu0)` with `k = 400 / (sqrt(2) * beta * ln 10) = 29.48`
and `z = display_z`, 0 by default: the ladder shows the estimate itself. `k` is chosen so that, between two
settled teams, 400 display points mean 10:1 odds, as in Elo (the model's logistic has scale `c -> sqrt(2) beta`
as sigma -> 0; with real sigmas of 1.5 to 2.5 the scale is 5 to 15 % wider, so 400 points are slightly less than
10:1). A new team shows 1500 and moves with its results; an upload leaves the number where it is. TrueSkill's
conservative `z = 3` would show a new team at 763 and make every team climb for as long as its sigma shrinks,
long after its estimate is right; after three placement series it showed a newcomer 13 places from its true
rank, against 6 for `mu` (table below). Uncertainty is shown by the coordinator as "placing" instead.

### Elo

`E_A = 1 / (1 + 10^((R_B - R_A)/400))`, `R_A <- R_A + K (S_A - E_A)`, `R_B <- R_B - K (S_A - E_A)`.

| Default | Value | Why |
|---|---|---|
| start | 1500 | Cambridge Battlecode |
| `k` | 24 | MIT Battlecode |
| `k_provisional`, `provisional_games` | 48 for the first 10 games | Cambridge Battlecode's 10 provisional matches |

One K per game, the mean of the two teams' K-factors, applied to both: the rating sum is conserved exactly, so
newcomers cannot inflate the ladder. `Rating::sigma` holds the fraction of the provisional window still open
(1 for a new team, 0 after 10 games); a resubmission reopens half of it, a balance patch of factor `f` reopens
`f - 1` of it (nothing if that is less than half a game), inactivity does nothing. `display` is the rating
itself. Two side effects of the shared K to be aware of: a settled team moves with K = 36 against a provisional
opponent, and a team that resubmits every few games stays provisional and keeps its neighbours at K = 36.

## Matchmaking (`plan_batch`)

Halite III's scheme. Repeat until the budget is used or fewer than two teams are left:

1. **Seed team.** 65 %: weighted draw, weight `0.05 + uncertainty + 1 / (1 + recent_games)` (under-played:
   high sigma or few recent games). 25 %: weighted draw in proportion to idle time (a team that never played
   counts as twice the longest idle time). 10 %: the team that played least recently.
2. **Opponent.** Candidates are the remaining live teams, sorted by `|mu - mu_seed|`. Draw `U ~ U(0,1)` and
   pick uniformly among the nearest `K = ceil(5 / (0.01 + U)^0.65)`: the median K is 8, 10 % of draws exceed
   21, 1 % exceed 63. Close games are informative; the tail keeps the rating graph connected.
3. **Rematch rules.** A pair that already met `max_series_per_opponent_per_day` (2) times in
   `recent_opponents` is never paired. A team's most recent opponent is skipped unless nobody else is left.
4. Both teams leave the pool: a team gets at most one series per batch. Team A/B is a coin flip.

Teams with `live = false` are never paired. A seed with no legal opponent sits the batch out, so the result
may be shorter than the budget.

## Placement (`plan_placement`)

Placement matches, as in most ranked games: a team with a new submission plays a few series back to back,
outside the batches, so the ladder knows roughly where it belongs within minutes instead of after several
batches. Each series is planned after the previous one is rated: the opponent is drawn uniformly among the
`placement_window` (3) live teams nearest the newcomer's current mu, close games being the most informative.
Settled opponents come first, since a game against a provisional team places neither; on a fresh ladder,
where everyone is provisional, anyone near will do. The rematch rules of `plan_batch` apply. How many series
and how often is the coordinator's business (3 series of 3 maps by default, at most 9 placement series per
team per day).

## Series plan (`plan_series`)

- **Maps**: least-used first according to `map_usage` (the caller's running count per pool index), ties broken
  at random from the series id. No repeats within a series unless the series is longer than the pool. Usage
  across the pool never differs by more than one game when one planner feeds the counts back.
- **Slots**: the engine already swaps attack and defence at half-time, so one game is side-balanced. Team A/B
  slots still alternate game by game from a coin flip on the series id, against residual slot bias.
- **Seeds**: `game_seed(series_id, game_index)`, a fixed 64-bit mix, so seeds need not be stored.

## Budget governor (`series_per_batch`)

```
series = floor( core_seconds_per_hour * utilisation / batches_per_hour
                / (games_per_series * cost_per_game) )                   capped at max_series_per_batch
```

`cost_per_game` is the measured mean cost of recent finished games (core-seconds, from `wall_ms`), passed as
`Some(mean)`. With `None` (a new season, nothing measured) or a mean that is zero, negative or not finite, the
prior `prior_core_seconds_per_game` (10) is used, so a fresh ladder can start; the cost is never taken below
`min_core_seconds_per_game` (0.25), so a mean polluted by instantly-failing jobs cannot flood the queue. Both
numbers are placeholders until `crates/bench` has measured real games: set the prior on the high side.

Other defaults: `utilisation` 0.7 (headroom for retries, spot verification and the other job classes),
`batches_per_hour` 6, `games_per_series` 5, `max_series_per_batch` 500. A capacity or configuration that is
zero, negative or not finite gives 0: the ladder pauses instead of queueing work the fleet cannot do.

## Simulation (`tests/simulation.rs`)

60 teams, true skill `N(1500, 250)` on the Elo scale, 2 % draws, 20 series of 3 games per batch (the governor's
answer for a 4-core box at 28 core-seconds per game), 6 maps, 200 batches. `rho` is the Spearman correlation
between the displayed leaderboard and true skill (ties share a rank). The numbers are from one machine
(x86-64 Windows); another libm may shift them slightly, and the asserted margins allow for that.

```
batch  games/team  rho(Elo)  rho(WL)  rho(WL mu)  WL mean sigma  provisional Elo/WL
    1         2.0     0.513    0.513       0.513           7.85         60/60
    2         4.0     0.542    0.694       0.694           7.40         60/60
    3         6.0     0.672    0.721       0.721           7.00         60/60
    5        10.0     0.774    0.809       0.809           6.31         38/60
    8        16.0     0.872    0.834       0.834           5.50          0/56
   12        24.0     0.931    0.886       0.886           4.73          0/14
   20        40.0     0.926    0.926       0.926           3.77          0/0
   35        70.0     0.955    0.970       0.970           2.88          0/0
   60       120.0     0.969    0.985       0.985           2.28          0/0
  100       200.0     0.978    0.988       0.988           1.88          0/0
  150       300.0     0.975    0.984       0.984           1.69          0/0
  200       400.0     0.974    0.994       0.994           1.60          0/0

cold start: games per team until the leaderboard first reaches a rank correlation
(mean of 10 ladders; a ladder that never gets there counts as 400)
target   Elo    Weng-Lin
  0.70    6.2       5.8
  0.80   10.4       9.4
  0.90   20.8      20.6
  0.95   36.8      39.0
  0.97   87.4      59.8
  0.98  174.0     101.8

newcomer (5th best of 60) joining a settled ladder, its first 90 games (mean of 10 ladders)
           mean |error|   games until first within 100   games until it stays within 100
Elo               109.6                           33.2                              45.1
Weng-Lin           64.2                            6.6                              43.0

newcomer at true rank 5, 30 or 55 of 60 joins a settled ladder (Weng-Lin, 30 arrivals)
                    |error| after   mean |error|        places off when shown by
placement series    placement       next 10 batches     mu - 3 sigma   mu
none                        240.4                98.1           27.4   16.6
3 (9 games)                  99.8                69.3           13.1    6.0

resubmission, true skill +300 (Weng-Lin, mean of 12 ladders, first 90 games after)
                 mean |error|   games until first within 100 (90 = never)
sigma bump               63.8      9.9
no bump                 167.9     71.0

resubmission: a mid-table team's skill jumps and it places again; mean |error| over 60 games (12 ladders)
sigma after upload    -300   -150      0    150    300    500     mean
0.85 sigma0           91.1   67.2   56.7   65.5   82.5  119.7     80.5
0.5 sigma0           124.7   65.3   43.4   57.3  106.7  212.0    101.6
```

What this shows, including what it does not:

- Both systems order the ladder well; Weng-Lin ends tighter (0.99 against 0.975) because its steps shrink
  while Elo keeps moving 12 points a game.
- **On a cold start the two are level up to rho = 0.95.** Elo's doubled provisional K does the job of a wide
  prior while everyone is new. Weng-Lin is ahead only for tight orderings (0.97: 60 against 87 games per team;
  0.98: 102 against 174). Showing `mu` rather than `mu - 3 sigma` orders the ladder as well or better at every
  row (the old `rho(WL)` column was 0.446 at batch 1 and 0.820 at batch 8).
- A strong newcomer in a settled ladder gets near its place in about 7 games with Weng-Lin and 33 with Elo.
  It is not steadier afterwards: the estimate still wanders across the 100-point line for about as long as
  Elo takes to arrive.
- **Placement** (3 series of 3 games before the next batch) cuts a newcomer's error before its first batch
  from 240 to 100 points and its mean error over the next 10 batches from 98 to 69. Here the newcomer plays
  in most batches anyway (20 series for 60 teams, and the matchmaker favours it); on a ladder where a batch
  reaches a smaller share of the teams, waiting for batches costs more and placement saves more. Six series
  brought the error after placement to 53 in a trial run, at twice the cost.
- **Resubmission**: averaged over jumps from -300 to +500, widening sigma to 0.85 sigma0 instead of 0.5
  lowers the error from 102 to 81. The price is a bot no better than the last one: 57 against 43. 0.7 and
  1.0 gave 87 and 81 in a trial run, within the noise of 0.85.
- Errors are measured after matching each system's mean and spread to the true scale, so neither is charged
  for its units.
