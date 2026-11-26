//! Ladder simulation: synthetic teams with a hidden true skill play ranked batches produced by the real
//! matchmaker, series planner and budget governor, and are rated by each system.
//!
//! True skill is on the Elo scale: P(A beats B) = 1 / (1 + 10^((s_B - s_A) / 400)).
//! Run with `cargo test -p ibc-rating --release -- --nocapture` to see the tables.

use ibc_rating::rng::{mix64, Rng};
use ibc_rating::{
    apply_game, plan_batch, plan_placement, plan_series, series_per_batch, BudgetConfig, Elo,
    MatchmakingConfig, Pairing, Rating, RatingSystem, TeamState, WengLin, Winner,
};

const TEAMS: usize = 60;
const BATCHES: u64 = 200;
const GAMES_PER_SERIES: u32 = 3;
const MAPS: usize = 6;
/// Batches per "day": the window of `recent_opponents` and `recent_games`.
const DAY: u64 = 8;
const SKILL_SD: f64 = 250.0;
const DRAW_RATE: f64 = 0.02;
/// The coordinator's default `placement_series`.
const PLACEMENT_SERIES: u32 = 3;

/// A 4-core box at 70 % utilisation, 6 batches an hour, 28 core-seconds per game: 20 series a batch,
/// so 40 of the 60 teams play in each batch and the matchmaker has real choices to make.
fn budget() -> u32 {
    let cfg = BudgetConfig {
        games_per_series: GAMES_PER_SERIES,
        ..BudgetConfig::default()
    };
    series_per_batch(4.0 * 3600.0, Some(28.0), &cfg)
}

fn gaussian(rng: &mut Rng) -> f64 {
    let (u1, u2) = (1.0 - rng.next_f64(), rng.next_f64());
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

struct Ladder<'a> {
    system: &'a dyn RatingSystem,
    seed: u64,
    outcomes: Rng,
    skill: Vec<f64>,
    rating: Vec<Rating>,
    /// Per team: (batch, opponent) of every series played.
    series: Vec<Vec<(u64, u64)>>,
    map_usage: Vec<u64>,
    batch: u64,
    next_series_id: u64,
    games: u64,
    /// Team whose estimate error is recorded after each of its games (see `error_of`).
    watched: Option<usize>,
    trace: Vec<f64>,
}

impl<'a> Ladder<'a> {
    fn new(system: &'a dyn RatingSystem, seed: u64) -> Self {
        let mut skills = Rng::new(mix64(seed ^ 0x0051_C111));
        Ladder {
            system,
            seed,
            outcomes: Rng::new(mix64(seed ^ 0x00C0_FFEE)),
            skill: (0..TEAMS)
                .map(|_| 1500.0 + SKILL_SD * gaussian(&mut skills))
                .collect(),
            rating: vec![system.initial(); TEAMS],
            series: vec![Vec::new(); TEAMS],
            map_usage: vec![0; MAPS],
            batch: 0,
            next_series_id: 0,
            games: 0,
            watched: None,
            trace: Vec::new(),
        }
    }

    fn states(&self) -> Vec<TeamState> {
        (0..TEAMS)
            .map(|i| {
                let recent: Vec<u64> = self.series[i]
                    .iter()
                    .filter(|(at, _)| at + DAY > self.batch)
                    .map(|&(_, opponent)| opponent)
                    .collect();
                TeamState {
                    id: i as u64,
                    rating: self.rating[i],
                    recent_games: recent.len() as u32 * GAMES_PER_SERIES,
                    last_played: self.series[i].last().map(|&(at, _)| at),
                    recent_opponents: recent,
                    live: true,
                }
            })
            .collect()
    }

    /// Play one ranked batch and return its pairings. Checks the batch invariants on the way.
    fn step(&mut self) -> Vec<Pairing> {
        let states = self.states();
        let cfg = MatchmakingConfig::default();
        let batch_seed = mix64(self.seed ^ self.batch);
        let pairings = plan_batch(self.system, &states, self.batch, budget(), &cfg, batch_seed);

        // (e) nobody plays themselves or twice in a batch; the budget and the daily cap hold.
        assert!(pairings.len() <= budget() as usize);
        let mut busy = [false; TEAMS];
        for p in &pairings {
            assert_ne!(p.team_a, p.team_b, "batch {}: self-pairing", self.batch);
            for id in [p.team_a, p.team_b] {
                assert!(
                    !std::mem::replace(&mut busy[id as usize], true),
                    "batch {}: team {id} twice",
                    self.batch
                );
            }
            let met = states[p.team_a as usize]
                .recent_opponents
                .iter()
                .filter(|&&o| o == p.team_b)
                .count();
            assert!(met < cfg.max_series_per_opponent_per_day as usize);
        }

        for p in &pairings {
            self.play(p);
        }
        self.batch += 1;
        pairings
    }

    /// Play `series` placement series for `team` back to back before the next batch, each opponent
    /// picked by `plan_placement` from the ratings as they stand after the previous one.
    fn place(&mut self, team: usize, series: u32) {
        let cfg = MatchmakingConfig::default();
        for k in 0..series {
            let states = self.states();
            let seed = mix64(self.seed ^ 0x9_1ACE ^ (self.batch << 8) ^ u64::from(k));
            let Some(opponent) = plan_placement(self.system, &states[team], &states, &cfg, seed)
            else {
                return;
            };
            self.play(&Pairing {
                team_a: team as u64,
                team_b: opponent,
            });
        }
    }

    fn play(&mut self, p: &Pairing) {
        let plan = plan_series(self.next_series_id, *p, &self.map_usage, GAMES_PER_SERIES);
        self.next_series_id += 1;
        assert_eq!(plan.games.len(), GAMES_PER_SERIES as usize);
        let (a, b) = (p.team_a as usize, p.team_b as usize);
        for game in &plan.games {
            self.map_usage[game.map as usize] += 1;
            let (sa, sb) = (
                self.skill[game.slot_a as usize],
                self.skill[game.slot_b as usize],
            );
            let p_slot_a = 1.0 / (1.0 + 10f64.powf((sb - sa) / 400.0));
            let winner = if self.outcomes.next_f64() < DRAW_RATE {
                Winner::Draw
            } else if self.outcomes.next_f64() < p_slot_a {
                Winner::SlotA
            } else {
                Winner::SlotB
            };
            let result = plan.result(game.index, winner).expect("planned game");
            let (mut ra, mut rb) = (self.rating[a], self.rating[b]);
            apply_game(self.system, &mut ra, &mut rb, result.outcome);
            self.rating[a] = ra;
            self.rating[b] = rb;
            self.games += 1;
            if let Some(w) = self.watched.filter(|&w| w == a || w == b) {
                self.trace.push(self.error_of(w));
            }
        }
        self.series[a].push((self.batch, p.team_b));
        self.series[b].push((self.batch, p.team_a));
    }

    fn run(&mut self, batches: u64) {
        for _ in 0..batches {
            self.step();
        }
    }

    /// 1-based position of `team` when the ladder is sorted by `mu - z * sigma`, best first.
    fn shown_rank(&self, team: usize, z: f64) -> usize {
        let key = |r: &Rating| r.mu - z * r.sigma;
        let mine = key(&self.rating[team]);
        1 + self.rating.iter().filter(|r| key(r) > mine).count()
    }

    fn true_rank(&self, team: usize) -> usize {
        1 + self.skill.iter().filter(|&&s| s > self.skill[team]).count()
    }

    fn displays(&self) -> Vec<f64> {
        self.rating.iter().map(|r| self.system.display(r)).collect()
    }

    fn mus(&self) -> Vec<f64> {
        self.rating.iter().map(|r| r.mu).collect()
    }

    fn games_per_team(&self) -> f64 {
        2.0 * self.games as f64 / TEAMS as f64
    }

    /// |estimate - truth| of one team in true-skill (Elo) points. The estimate is the team's mu
    /// mapped onto the true-skill scale by matching the mean and spread of the other teams (so
    /// neither system is charged for the unit or the compression of its own scale).
    fn error_of(&self, team: usize) -> f64 {
        let mean = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;
        let sd = |xs: &[f64]| {
            let m = mean(xs);
            (xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / xs.len() as f64).sqrt()
        };
        let others = |xs: Vec<f64>| -> Vec<f64> {
            let keep = |&(i, _): &(usize, f64)| i != team;
            xs.into_iter()
                .enumerate()
                .filter(keep)
                .map(|(_, x)| x)
                .collect()
        };
        let (mu, skill) = (others(self.mus()), others(self.skill.clone()));
        let points_per_mu = sd(&skill) / sd(&mu);
        let estimate = mean(&skill) + points_per_mu * (self.rating[team].mu - mean(&mu));
        (estimate - self.skill[team]).abs()
    }
}

/// Ranks from 0, with tied values sharing the mean of their positions.
fn ranks(xs: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..xs.len()).collect();
    order.sort_by(|&i, &j| xs[i].total_cmp(&xs[j]));
    let mut rank = vec![0.0; xs.len()];
    let mut start = 0;
    while start < order.len() {
        let mut end = start + 1;
        while end < order.len() && xs[order[end]] == xs[order[start]] {
            end += 1;
        }
        for &i in &order[start..end] {
            rank[i] = (start + end - 1) as f64 / 2.0;
        }
        start = end;
    }
    rank
}

/// Spearman rank correlation: the Pearson correlation of the tie-averaged ranks (teams that have not
/// played yet share a rating). 0 if either side is constant.
fn spearman(xs: &[f64], ys: &[f64]) -> f64 {
    let (rx, ry) = (ranks(xs), ranks(ys));
    let mean = (xs.len() as f64 - 1.0) / 2.0;
    let dot = |a: &[f64], b: &[f64]| -> f64 {
        a.iter().zip(b).map(|(x, y)| (x - mean) * (y - mean)).sum()
    };
    let denom = (dot(&rx, &rx) * dot(&ry, &ry)).sqrt();
    if denom > 0.0 {
        dot(&rx, &ry) / denom
    } else {
        0.0
    }
}

/// Games per team needed until the leaderboard order first reaches `target` rank correlation.
fn games_to_reach(system: &dyn RatingSystem, seed: u64, target: f64) -> f64 {
    let mut ladder = Ladder::new(system, seed);
    for _ in 0..BATCHES {
        ladder.step();
        if spearman(&ladder.displays(), &ladder.skill) >= target {
            return ladder.games_per_team();
        }
    }
    f64::INFINITY
}

#[test]
fn convergence_table_and_rank_correlation() {
    let (elo, wl) = (Elo::default(), WengLin::default());
    let (mut le, mut lw) = (Ladder::new(&elo, 2026), Ladder::new(&wl, 2026));
    println!(
        "\n{TEAMS} teams, skill sd {SKILL_SD}, {} series x {GAMES_PER_SERIES} games per batch",
        budget()
    );
    println!("batch  games/team  rho(Elo)  rho(WL)  rho(WL mu)  WL mean sigma  provisional Elo/WL");
    for batch in 1..=BATCHES {
        le.step();
        lw.step();
        if [1, 2, 3, 5, 8, 12, 20, 35, 60, 100, 150, 200].contains(&batch) {
            let sigma = lw.rating.iter().map(|r| r.sigma).sum::<f64>() / TEAMS as f64;
            let prov_e = le.rating.iter().filter(|r| elo.is_provisional(r)).count();
            let prov_w = lw.rating.iter().filter(|r| wl.is_provisional(r)).count();
            println!(
                "{batch:5}  {:10.1}  {:8.3}  {:7.3}  {:10.3}  {sigma:13.2}  {prov_e:9}/{prov_w}",
                lw.games_per_team(),
                spearman(&le.displays(), &le.skill),
                spearman(&lw.displays(), &lw.skill),
                spearman(&lw.mus(), &lw.skill),
            );
        }
        // (f) Elo conserves the rating sum.
        let sum: f64 = le.mus().iter().sum();
        assert!(
            (sum - 1500.0 * TEAMS as f64).abs() < 1e-6,
            "batch {batch}: Elo sum {sum}"
        );
    }
    // (a) both leaderboards end up in the true order, near enough.
    let (rho_e, rho_w) = (
        spearman(&le.displays(), &le.skill),
        spearman(&lw.displays(), &lw.skill),
    );
    assert!(rho_e > 0.93, "Elo rank correlation {rho_e}");
    assert!(rho_w > 0.95, "Weng-Lin rank correlation {rho_w}");
    // The map pool is used evenly.
    let (lo, hi) = (
        lw.map_usage.iter().min().unwrap(),
        lw.map_usage.iter().max().unwrap(),
    );
    assert!(hi - lo <= 1, "map usage {:?}", lw.map_usage);
    // Everybody got games: the matchmaker starves nobody.
    let fewest = lw.rating.iter().map(|r| r.games).min().unwrap();
    assert!(
        f64::from(fewest) > 0.5 * lw.games_per_team(),
        "fewest games {fewest}"
    );
}

/// A strong team (5th best of 60 by true skill) joins a ladder that has already settled, with a fresh
/// rating. Returns, over its first `CAP` games: its mean |error|, the number of games until the error
/// is first under 100 points, and the number of games after which it stays under 100.
fn newcomer(system: &dyn RatingSystem, seed: u64) -> [f64; 3] {
    const CAP: usize = 90;
    let mut ladder = Ladder::new(system, seed);
    ladder.run(100);
    let order = ranks(&ladder.skill);
    let team = order.iter().position(|&r| r as usize == TEAMS - 5).unwrap();
    ladder.rating[team] = system.initial();
    ladder.series[team].clear();
    ladder.watched = Some(team);
    while ladder.trace.len() < CAP {
        ladder.step();
    }
    let trace = &ladder.trace[..CAP];
    let first = trace.iter().position(|&e| e < 100.0).unwrap_or(CAP);
    let stays = trace.iter().rposition(|&e| e >= 100.0).map_or(0, |i| i + 1);
    [
        trace.iter().sum::<f64>() / CAP as f64,
        first as f64,
        stays as f64,
    ]
}

#[test]
fn weng_lin_converges_in_fewer_games_than_elo() {
    let (elo, wl) = (Elo::default(), WengLin::default());
    let mean_over_seeds = |f: &dyn Fn(u64) -> f64| (1..=10u64).map(f).sum::<f64>() / 10.0;
    let cap = 2.0 * f64::from(budget() * GAMES_PER_SERIES) * BATCHES as f64 / TEAMS as f64;
    println!("\ncold start: games per team until the leaderboard first reaches a rank correlation");
    println!("(mean of 10 ladders; a ladder that never gets there counts as {cap:.0})");
    println!("target   Elo    Weng-Lin");
    let mut reach = Vec::new();
    for target in [0.70, 0.80, 0.90, 0.95, 0.97, 0.98] {
        let e = mean_over_seeds(&|s| games_to_reach(&elo, s, target).min(cap));
        let w = mean_over_seeds(&|s| games_to_reach(&wl, s, target).min(cap));
        println!("{target:6.2}  {e:5.1}  {w:8.1}");
        reach.push((target, e, w));
    }
    let e: Vec<f64> = (0..3)
        .map(|k| mean_over_seeds(&|s| newcomer(&elo, s)[k]))
        .collect();
    let w: Vec<f64> = (0..3)
        .map(|k| mean_over_seeds(&|s| newcomer(&wl, s)[k]))
        .collect();
    println!("\nnewcomer (5th best of {TEAMS}) joining a settled ladder, its first 90 games (mean of 10 ladders)");
    println!(
        "           mean |error|   games until first within 100   games until it stays within 100"
    );
    println!("Elo        {:12.1}   {:28.1}   {:31.1}", e[0], e[1], e[2]);
    println!("Weng-Lin   {:12.1}   {:28.1}   {:31.1}", w[0], w[1], w[2]);
    // (b) What holds, and what does not. Up to a rank correlation of about 0.95 the two are level:
    // while everybody is new, Elo's doubled provisional K does the job of a wide prior. Weng-Lin
    // needs fewer games to reach a tight ordering, and it puts a newcomer near its true place in a
    // handful of games where Elo needs dozens. It is not steadier once there: its larger steps make
    // the estimate wander, so "stays within 100" is printed but not asserted.
    for &(target, e, w) in &reach {
        if target >= 0.97 {
            assert!(w < 0.9 * e, "target {target}: Weng-Lin {w}, Elo {e}");
        } else {
            assert!(w < 1.25 * e, "target {target}: Weng-Lin {w}, Elo {e}");
        }
    }
    assert!(
        w[0] < 0.85 * e[0],
        "newcomer error: Weng-Lin {}, Elo {}",
        w[0],
        e[0]
    );
    assert!(
        w[1] < 0.5 * e[1],
        "newcomer games: Weng-Lin {}, Elo {}",
        w[1],
        e[1]
    );
}

/// Mean error of a team in its first `GAMES` games after its skill jumps, and how many of those games
/// it needs to get within 100 points, with or without the resubmission sigma bump.
fn rejoin(bump: bool, seed: u64) -> (f64, f64) {
    const GAMES: usize = 90;
    const JUMP: f64 = 300.0;
    let wl = WengLin::default();
    let mut ladder = Ladder::new(&wl, seed);
    ladder.run(100);
    // A mid-table team uploads a much better bot.
    let order = ranks(&ladder.skill);
    let team = order.iter().position(|&r| r as usize == TEAMS / 2).unwrap();
    ladder.skill[team] += JUMP;
    if bump {
        ladder.rating[team] = wl.on_new_submission(&ladder.rating[team]);
    }
    ladder.watched = Some(team);
    while ladder.trace.len() < GAMES {
        ladder.step();
    }
    let trace = &ladder.trace[..GAMES];
    let settled = trace.iter().position(|&e| e < 100.0).unwrap_or(GAMES);
    (trace.iter().sum::<f64>() / GAMES as f64, settled as f64)
}

#[test]
fn sigma_bump_reconverges_faster_after_resubmission() {
    let n = 12u64;
    let mean = |bump: bool| {
        let runs: Vec<(f64, f64)> = (0..n).map(|s| rejoin(bump, 100 + s)).collect();
        (
            runs.iter().map(|r| r.0).sum::<f64>() / n as f64,
            runs.iter().map(|r| r.1).sum::<f64>() / n as f64,
        )
    };
    let (with, without) = (mean(true), mean(false));
    println!(
        "\nresubmission, true skill +300 (Weng-Lin, mean of {n} ladders, first 90 games after)"
    );
    println!("                 mean |error|   games until first within 100 (90 = never)");
    println!("sigma bump       {:12.1}   {:6.1}", with.0, with.1);
    println!("no bump          {:12.1}   {:6.1}", without.0, without.1);
    // (c)
    assert!(
        with.0 < 0.7 * without.0,
        "mean error {} vs {}",
        with.0,
        without.0
    );
    assert!(
        with.1 < without.1,
        "games to settle {} vs {}",
        with.1,
        without.1
    );
}

/// A newcomer at true rank 5, 30 or 55 joins a settled ladder and plays `placement` series before the
/// next batch. Returns, averaged over seeds and ranks: its |error| right after placement, its mean
/// |error| over that moment and the next 10 batches, and how many places from its true rank it is shown
/// right after placement when the ladder sorts by `mu - 3 sigma` and by `mu`.
fn arrival(placement: u32) -> [f64; 4] {
    let wl = WengLin::default();
    let mut acc = [0.0; 4];
    let mut n = 0.0;
    for seed in 1..=10u64 {
        for target in [5, 30, 55] {
            let mut ladder = Ladder::new(&wl, seed);
            ladder.run(100);
            let team = (0..TEAMS).find(|&t| ladder.true_rank(t) == target).unwrap();
            ladder.rating[team] = wl.initial();
            ladder.series[team].clear();
            ladder.place(team, placement);
            let gap = |l: &Ladder, z| (l.shown_rank(team, z) as f64 - target as f64).abs();
            let (shown_z3, shown_mu) = (gap(&ladder, 3.0), gap(&ladder, 0.0));
            let mut errors = vec![ladder.error_of(team)];
            for _ in 0..10 {
                ladder.step();
                errors.push(ladder.error_of(team));
            }
            let mean = errors.iter().sum::<f64>() / errors.len() as f64;
            for (k, v) in [errors[0], mean, shown_z3, shown_mu]
                .into_iter()
                .enumerate()
            {
                acc[k] += v;
            }
            n += 1.0;
        }
    }
    acc.map(|x| x / n)
}

#[test]
fn placement_places_a_newcomer_before_its_first_batch() {
    let (none, placed) = (arrival(0), arrival(PLACEMENT_SERIES));
    println!("\nnewcomer at true rank 5, 30 or 55 of {TEAMS} joins a settled ladder (Weng-Lin, 30 arrivals)");
    println!("                    |error| after   mean |error|        places off when shown by");
    println!("placement series    placement       next 10 batches     mu - 3 sigma   mu");
    for (label, r) in [("none", none), ("3 (9 games)", placed)] {
        println!(
            "{label:<18}  {:13.1}   {:17.1}   {:12.1}   {:4.1}",
            r[0], r[1], r[2], r[3]
        );
    }
    assert!(
        placed[0] < 0.6 * none[0],
        "after placement {placed:?} vs {none:?}"
    );
    assert!(
        placed[1] < 0.85 * none[1],
        "next batches {placed:?} vs {none:?}"
    );
    // Shown by mu, the newcomer is near its place; the conservative estimate keeps it far from it.
    assert!(placed[3] < 0.7 * placed[2], "shown {placed:?}");
}

/// Mean |error| of a mid-table team over the 60 games after its skill jumps by `jump` and it places
/// again, under `system`'s resubmission rule.
fn rebuilt(system: &WengLin, jump: f64, seed: u64) -> f64 {
    const GAMES: usize = 60;
    let mut ladder = Ladder::new(system, seed);
    ladder.run(100);
    let team = (0..TEAMS)
        .find(|&t| ladder.true_rank(t) == TEAMS / 2)
        .unwrap();
    ladder.skill[team] += jump;
    ladder.rating[team] = system.on_new_submission(&ladder.rating[team]);
    ladder.watched = Some(team);
    ladder.place(team, PLACEMENT_SERIES);
    while ladder.trace.len() < GAMES {
        ladder.step();
    }
    ladder.trace[..GAMES].iter().sum::<f64>() / GAMES as f64
}

#[test]
fn a_new_bot_is_treated_as_mostly_unknown() {
    let jumps = [-300.0, -150.0, 0.0, 150.0, 300.0, 500.0];
    let seeds = 12u64;
    let row = |system: &WengLin| -> Vec<f64> {
        jumps
            .iter()
            .map(|&j| (0..seeds).map(|s| rebuilt(system, j, 100 + s)).sum::<f64>() / seeds as f64)
            .collect()
    };
    let old = WengLin {
        resubmission_sigma_fraction: 0.5,
        ..WengLin::default()
    };
    let (now, before) = (row(&WengLin::default()), row(&old));
    println!("\nresubmission: a mid-table team's skill jumps and it places again; mean |error| over 60 games ({seeds} ladders)");
    print!("sigma after upload ");
    for j in jumps {
        print!("{j:>7}");
    }
    println!("     mean");
    let mean = |r: &[f64]| r.iter().sum::<f64>() / r.len() as f64;
    for (label, r) in [("0.85 sigma0", &now), ("0.5 sigma0", &before)] {
        print!("{label:<18} ");
        for e in r.iter() {
            print!("{e:7.1}");
        }
        println!("  {:7.1}", mean(r));
    }
    assert!(mean(&now) < 0.9 * mean(&before), "{now:?} vs {before:?}");
    // The price: a bot no better than the last one wanders more before it settles again.
    assert!(now[2] > before[2]);
}

#[test]
fn fixed_seed_is_deterministic() {
    let wl = WengLin::default();
    let elo = Elo::default();
    for system in [&wl as &dyn RatingSystem, &elo] {
        let run = |seed: u64| {
            let mut ladder = Ladder::new(system, seed);
            let pairings: Vec<Vec<Pairing>> = (0..40).map(|_| ladder.step()).collect();
            let bits: Vec<(u64, u64, u32)> = ladder
                .rating
                .iter()
                .map(|r| (r.mu.to_bits(), r.sigma.to_bits(), r.games))
                .collect();
            (pairings, bits)
        };
        // (d)
        assert_eq!(run(7), run(7), "{}", system.name());
        assert_ne!(run(7).0, run(8).0, "{}", system.name());
    }
}
