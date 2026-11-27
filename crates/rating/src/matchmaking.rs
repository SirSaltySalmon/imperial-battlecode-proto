//! Pairings for one periodic ranked batch.
//!
//! The policy follows the Halite III matchmaker: a *seed* team is drawn with a bias towards teams the
//! ladder knows least about, then its opponent is drawn from the nearest teams by rating, with a
//! heavy-tailed window so that most series are close and a few are not (which keeps the rating graph
//! connected and lets a mis-rated team escape its neighbourhood). Within the window, opponents met
//! recently are less likely but never ruled out, and a per-team cap on recent series bounds how fast an
//! idle fleet can run any one team.

use crate::rng::{mix64, Rng};
use crate::{RatingSystem, TeamId};
use std::collections::BTreeSet;

/// What the matchmaker needs to know about one team. The coordinator builds these from its database.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TeamState {
    pub id: TeamId,
    /// Current rating under the system passed to [`plan_batch`]; carries sigma and total games played.
    pub rating: crate::Rating,
    /// Rated games in the recent window (suggested: the last 24 hours).
    pub recent_games: u32,
    /// When the team last played a rated game, in any monotone unit (seconds, batch number), the same
    /// unit as `now`. `None` = never.
    pub last_played: Option<u64>,
    /// Opponents of this team's series in the last day, oldest first, one entry per series (so an
    /// opponent met twice appears twice). The last entry is the most recent opponent.
    pub recent_opponents: Vec<TeamId>,
    /// Ranked series of this team in the caller's cap window (suggested: the last 15 minutes), counted
    /// against [`MatchmakingConfig::series_cap`].
    pub recent_series: u32,
    /// Whether the team has a live (accepted, runnable) submission. Teams without one are never paired.
    pub live: bool,
}

/// One series to schedule. Which side is `team_a` is a fair coin flip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pairing {
    pub team_a: TeamId,
    pub team_b: TeamId,
}

/// Matchmaking policy. The defaults are Halite III's numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MatchmakingConfig {
    /// Probability that the seed is drawn by "under-played" weight (high uncertainty, few recent games).
    pub p_underplayed: f64,
    /// Probability that the seed is drawn in proportion to idle time. The remainder
    /// (`1 - p_underplayed - p_idle`) picks the team that played least recently.
    pub p_idle: f64,
    /// Opponent window `K = ceil(window_scale / (window_offset + U)^window_exponent)`, `U ~ U(0,1)`:
    /// the opponent is drawn among the K nearest candidates by mu. Median K is about 8, 1 % of draws
    /// exceed 60.
    pub window_scale: f64,
    pub window_offset: f64,
    pub window_exponent: f64,
    /// Within the window, a candidate met `m` times in `recent_opponents` is drawn with weight
    /// `rematch_weight^m`: fresh opponents first, a rematch whenever they run out.
    pub rematch_weight: f64,
    /// Do not pair a team with its most recent opponent unless nobody else is available.
    pub avoid_immediate_rematch: bool,
    /// Only a team with fewer than this many `recent_series` looks for an opponent.
    pub series_cap: u32,
    /// A team at its cap may still be drawn as an opponent, up to this many series past it, when every
    /// candidate in the seed's window is at its cap. Past `series_cap + cap_overflow` it is never paired.
    pub cap_overflow: u32,
    /// [`plan_placement`] draws among this many nearest opponents by mu.
    pub placement_window: u32,
}

impl Default for MatchmakingConfig {
    fn default() -> Self {
        MatchmakingConfig {
            p_underplayed: 0.65,
            p_idle: 0.25,
            window_scale: 5.0,
            window_offset: 0.01,
            window_exponent: 0.65,
            rematch_weight: 0.25,
            avoid_immediate_rematch: true,
            series_cap: 5,
            cap_overflow: 5,
            placement_window: 3,
        }
    }
}

impl MatchmakingConfig {
    fn under_cap(&self, t: &TeamState) -> bool {
        t.recent_series < self.series_cap
    }

    fn under_overflow(&self, t: &TeamState) -> bool {
        t.recent_series < self.series_cap.saturating_add(self.cap_overflow)
    }
}

fn meetings(a: &TeamState, b: &TeamState) -> u32 {
    let in_a = a.recent_opponents.iter().filter(|&&o| o == b.id).count();
    let in_b = b.recent_opponents.iter().filter(|&&o| o == a.id).count();
    in_a.max(in_b).min(u32::MAX as usize) as u32
}

fn just_met(a: &TeamState, b: &TeamState) -> bool {
    a.recent_opponents.last() == Some(&b.id) || b.recent_opponents.last() == Some(&a.id)
}

/// Idle time used as a weight. A team that never played counts as twice the longest known idle time.
fn idle_weights(pool: &[&TeamState], now: u64) -> Vec<f64> {
    let idle = |t: &TeamState| t.last_played.map(|at| now.saturating_sub(at));
    let longest = pool.iter().filter_map(|t| idle(t)).max().unwrap_or(0);
    pool.iter()
        .map(|t| match idle(t) {
            Some(d) => d as f64 + 1.0,
            None => 2.0 * longest as f64 + 2.0,
        })
        .collect()
}

fn pick_seed(
    system: &dyn RatingSystem,
    pool: &[&TeamState],
    now: u64,
    cfg: &MatchmakingConfig,
    rng: &mut Rng,
) -> usize {
    let mode = rng.next_f64();
    if mode < cfg.p_underplayed {
        // Uncertain teams and teams with few recent games both need games; either is enough.
        let weights: Vec<f64> = pool
            .iter()
            .map(|t| 0.05 + system.uncertainty(&t.rating) + 1.0 / (1.0 + f64::from(t.recent_games)))
            .collect();
        rng.weighted(&weights)
    } else if mode < cfg.p_underplayed + cfg.p_idle {
        rng.weighted(&idle_weights(pool, now))
    } else {
        // Least recent: never-played first, then the oldest timestamp; `pool` is sorted by id, and
        // `min_by_key` keeps the first minimum, so ties go to the lowest id.
        let key = |t: &TeamState| t.last_played.map_or(0, |at| at.saturating_add(1));
        (0..pool.len()).min_by_key(|&i| key(pool[i])).unwrap_or(0)
    }
}

/// Indices into `pool` of the teams `seed` may play: never itself, never past its cap plus the overflow,
/// and not its most recent opponent unless nobody else is left.
fn legal_opponents(seed: &TeamState, pool: &[&TeamState], cfg: &MatchmakingConfig) -> Vec<usize> {
    let allowed = |t: &TeamState| t.id != seed.id && cfg.under_overflow(t);
    let candidates: Vec<usize> = (0..pool.len())
        .filter(|&i| allowed(pool[i]) && !(cfg.avoid_immediate_rematch && just_met(seed, pool[i])))
        .collect();
    if candidates.is_empty() {
        // Small ladders: a rematch beats no series.
        return (0..pool.len()).filter(|&i| allowed(pool[i])).collect();
    }
    candidates
}

/// Draw `seed`'s opponent from `window` (indices into `pool`, non-empty). Teams under their cap if the
/// window has any; otherwise the window is all at its cap and the ones with the fewest recent series go
/// past it. Among those, a team met `m` times recently has weight `rematch_weight^m`.
fn choose_in_window(
    seed: &TeamState,
    pool: &[&TeamState],
    window: &[usize],
    cfg: &MatchmakingConfig,
    rng: &mut Rng,
) -> usize {
    let mut eligible: Vec<usize> = window
        .iter()
        .copied()
        .filter(|&i| cfg.under_cap(pool[i]))
        .collect();
    if eligible.is_empty() {
        let fewest = window.iter().map(|&i| pool[i].recent_series).min();
        eligible = window
            .iter()
            .copied()
            .filter(|&i| Some(pool[i].recent_series) == fewest)
            .collect();
    }
    let weights: Vec<f64> = eligible
        .iter()
        .map(|&i| {
            let m = i32::try_from(meetings(seed, pool[i])).unwrap_or(i32::MAX);
            cfg.rematch_weight.powi(m)
        })
        .collect();
    eligible[rng.weighted(&weights)]
}

/// Sort `candidates` nearest to `seed` by mu first.
fn sort_by_gap(seed: &TeamState, pool: &[&TeamState], candidates: &mut [usize], rng: &mut Rng) {
    let gap = |i: usize| (pool[i].rating.mu - seed.rating.mu).abs();
    // Equal gaps (a fresh ladder has everyone on the same mu) are ordered by a per-draw hash of the
    // id, not by the id itself, so low ids are not favoured.
    let salt = rng.next_u64();
    let tie = |i: usize| mix64(salt ^ pool[i].id);
    candidates.sort_by(|&x, &y| {
        gap(x)
            .total_cmp(&gap(y))
            .then(tie(x).cmp(&tie(y)))
            .then(pool[x].id.cmp(&pool[y].id))
    });
}

fn pick_opponent(
    seed: &TeamState,
    pool: &[&TeamState],
    cfg: &MatchmakingConfig,
    rng: &mut Rng,
) -> Option<usize> {
    let mut candidates = legal_opponents(seed, pool, cfg);
    if candidates.is_empty() {
        return None;
    }
    sort_by_gap(seed, pool, &mut candidates, rng);
    let u = rng.next_f64();
    let k = (cfg.window_scale / (cfg.window_offset + u).powf(cfg.window_exponent)).ceil();
    // `as usize` saturates, so an infinite window means "anyone"; NaN and values below 1 mean the
    // nearest candidate only.
    let k = (k as usize).clamp(1, candidates.len());
    Some(choose_in_window(seed, pool, &candidates[..k], cfg, rng))
}

/// Pairings for one ranked batch: at most `budget_series` series, at most one per team, only between
/// distinct live teams, each started by a team under its cap. The result depends only on the arguments,
/// and not on the order of `teams` as long as ids are unique (of several entries with one id, the first
/// is used). Fewer pairings than the budget are returned when teams run out, every remaining team is at
/// its cap, or a seed has nobody under the overflow limit to play.
///
/// `now` is the current time in the unit of [`TeamState::last_played`]. Store `seed` with the batch to
/// be able to replay the decision.
pub fn plan_batch(
    system: &dyn RatingSystem,
    teams: &[TeamState],
    now: u64,
    budget_series: u32,
    cfg: &MatchmakingConfig,
    seed: u64,
) -> Vec<Pairing> {
    let mut seen = BTreeSet::new();
    let mut pool: Vec<&TeamState> = teams
        .iter()
        .filter(|t| t.live && seen.insert(t.id))
        .collect();
    pool.sort_by_key(|t| t.id);
    let mut rng = Rng::new(seed);
    let mut pairings = Vec::new();
    while pool.len() >= 2 && (pairings.len() as u64) < u64::from(budget_series) {
        let seeds: Vec<&TeamState> = pool.iter().copied().filter(|t| cfg.under_cap(t)).collect();
        if seeds.is_empty() {
            break;
        }
        let s = seeds[pick_seed(system, &seeds, now, cfg, &mut rng)].id;
        let s = pool
            .iter()
            .position(|t| t.id == s)
            .expect("seeds come from the pool");
        let seed_team = pool.remove(s);
        // A seed with no legal opponent sits this batch out; the others can still be paired.
        let Some(o) = pick_opponent(seed_team, &pool, cfg, &mut rng) else {
            continue;
        };
        let opponent = pool.remove(o);
        let (team_a, team_b) = if rng.next_u64() & 1 == 0 {
            (seed_team.id, opponent.id)
        } else {
            (opponent.id, seed_team.id)
        };
        pairings.push(Pairing { team_a, team_b });
    }
    pairings
}

/// The opponent of one placement series for `newcomer`, a team whose current submission the ladder knows
/// little about. Placement is a sequence of such series played back to back, each pairing the newcomer
/// with one of the `placement_window` live teams nearest its current mu (the closest games carry the most
/// information), settled teams first: a game against a team whose rating is itself provisional places
/// neither. The rematch and cap rules of [`plan_batch`] apply. `None` if the newcomer is at its cap or
/// nobody is a legal opponent.
pub fn plan_placement(
    system: &dyn RatingSystem,
    newcomer: &TeamState,
    teams: &[TeamState],
    cfg: &MatchmakingConfig,
    seed: u64,
) -> Option<TeamId> {
    if !cfg.under_cap(newcomer) {
        return None;
    }
    let mut seen = BTreeSet::new();
    let mut pool: Vec<&TeamState> = teams
        .iter()
        .filter(|t| t.live && t.id != newcomer.id && seen.insert(t.id))
        .collect();
    pool.sort_by_key(|t| t.id);
    let legal = legal_opponents(newcomer, &pool, cfg);
    let settled: Vec<usize> = legal
        .iter()
        .copied()
        .filter(|&i| !system.is_provisional(&pool[i].rating))
        .collect();
    let mut candidates = if settled.is_empty() { legal } else { settled };
    if candidates.is_empty() {
        return None;
    }
    let mut rng = Rng::new(seed);
    sort_by_gap(newcomer, &pool, &mut candidates, &mut rng);
    let k = (cfg.placement_window.max(1) as usize).min(candidates.len());
    Some(pool[choose_in_window(newcomer, &pool, &candidates[..k], cfg, &mut rng)].id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Elo, Rating, WengLin};

    fn team(id: TeamId, mu: f64) -> TeamState {
        TeamState {
            id,
            rating: Rating {
                mu,
                sigma: 1.0,
                games: 100,
            },
            recent_games: 10,
            last_played: Some(0),
            recent_opponents: Vec::new(),
            recent_series: 0,
            live: true,
        }
    }

    #[test]
    fn respects_budget_liveness_and_uniqueness() {
        let wl = WengLin::default();
        let mut teams: Vec<TeamState> = (0..11).map(|i| team(i, 20.0 + i as f64)).collect();
        teams[3].live = false;
        teams.push(team(5, 99.0)); // duplicate id: ignored
        let cfg = MatchmakingConfig::default();
        let all = plan_batch(&wl, &teams, 10, 100, &cfg, 1);
        assert_eq!(all.len(), 5);
        let mut ids: Vec<TeamId> = all.iter().flat_map(|p| [p.team_a, p.team_b]).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 10);
        assert!(!ids.contains(&3));
        assert_eq!(plan_batch(&wl, &teams, 10, 2, &cfg, 1).len(), 2);
        assert!(plan_batch(&wl, &teams, 10, 0, &cfg, 1).is_empty());
        assert!(plan_batch(&wl, &teams[..1], 10, 5, &cfg, 1).is_empty());
        let mut shuffled = teams.clone();
        shuffled.reverse();
        shuffled.rotate_left(1); // keep the first entry of id 5 ahead of its duplicate
        assert_eq!(
            plan_batch(&wl, &teams, 10, 100, &cfg, 9),
            plan_batch(&wl, &shuffled, 10, 100, &cfg, 9)
        );
    }

    #[test]
    fn opponents_are_mostly_near() {
        let wl = WengLin::default();
        let teams: Vec<TeamState> = (0..200).map(|i| team(i, i as f64)).collect();
        let cfg = MatchmakingConfig::default();
        let mut gaps = Vec::new();
        for seed in 0..50 {
            for p in plan_batch(&wl, &teams, 10, 20, &cfg, seed) {
                gaps.push((p.team_a as f64 - p.team_b as f64).abs());
            }
        }
        gaps.sort_by(f64::total_cmp);
        let median = gaps[gaps.len() / 2];
        assert!(median <= 6.0, "median gap {median}");
        assert!(
            *gaps.last().unwrap() >= 20.0,
            "the tail should reach far sometimes"
        );
    }

    #[test]
    fn rematch_rules() {
        let elo = Elo::default();
        let cfg = MatchmakingConfig::default();
        // Three teams; 0 and 1 just met, so whoever is the seed, that pair is avoided.
        let mut teams: Vec<TeamState> = (0..3).map(|i| team(i, 1500.0)).collect();
        teams[0].recent_opponents = vec![1];
        teams[1].recent_opponents = vec![0];
        for seed in 0..40 {
            let p = plan_batch(&elo, &teams, 10, 1, &cfg, seed)[0];
            assert!(p.team_a == 2 || p.team_b == 2, "seed {seed}: {p:?}");
        }
        // Two teams that just met: the rematch is allowed as a last resort, however often they met.
        assert_eq!(plan_batch(&elo, &teams[..2], 10, 1, &cfg, 0).len(), 1);
        teams[0].recent_opponents = vec![1; 40];
        assert_eq!(plan_batch(&elo, &teams[..2], 10, 1, &cfg, 0).len(), 1);
    }

    #[test]
    fn fresh_opponents_are_preferred_but_rematches_still_happen() {
        let wl = WengLin::default();
        let cfg = MatchmakingConfig::default();
        // 1 was met three times (weight 1/64), 2 never (weight 1), 3 is the last opponent (skipped).
        let mut newcomer = team(9, 25.0);
        newcomer.recent_opponents = vec![1, 1, 1, 3];
        let teams = [team(1, 25.0), team(2, 25.0), team(3, 25.0)];
        let mut picks = [0u32; 4];
        for seed in 0..2000 {
            picks[plan_placement(&wl, &newcomer, &teams, &cfg, seed).unwrap() as usize] += 1;
        }
        assert_eq!(picks[3], 0);
        assert!(picks[2] > 1900, "{picks:?}");
        assert!(picks[1] > 0, "{picks:?}");
    }

    #[test]
    fn only_teams_under_their_cap_start_a_series() {
        let elo = Elo::default();
        let cfg = MatchmakingConfig::default();
        let mut teams: Vec<TeamState> = (0..5).map(|i| team(i, 1500.0)).collect();
        for t in &mut teams {
            t.recent_series = cfg.series_cap;
        }
        assert!(plan_batch(&elo, &teams, 10, 10, &cfg, 0).is_empty());
        // One team under its cap, one under it too: they pair with each other, not with a capped team.
        teams[0].recent_series = 0;
        teams[1].recent_series = 2;
        for seed in 0..100 {
            let p = plan_batch(&elo, &teams, 10, 10, &cfg, seed);
            assert_eq!(p.len(), 1, "seed {seed}: {p:?}");
            let mut pair = [p[0].team_a, p[0].team_b];
            pair.sort_unstable();
            assert_eq!(pair, [0, 1], "seed {seed}");
        }
    }

    #[test]
    fn a_capped_window_overflows_to_the_least_played_team() {
        let elo = Elo::default();
        let cfg = MatchmakingConfig::default();
        let limit = cfg.series_cap + cfg.cap_overflow;
        let mut teams: Vec<TeamState> = (0..5).map(|i| team(i, 1500.0)).collect();
        teams[1].recent_series = cfg.series_cap + 2;
        teams[2].recent_series = cfg.series_cap + 1;
        teams[3].recent_series = limit; // past the overflow: never paired
        teams[4].recent_series = cfg.series_cap + 1;
        let mut opponents = BTreeSet::new();
        for seed in 0..200 {
            let p = plan_batch(&elo, &teams, 10, 10, &cfg, seed);
            assert_eq!(p.len(), 1, "only team 0 may seed: {p:?}");
            assert!(p[0].team_a == 0 || p[0].team_b == 0);
            opponents.insert(p[0].team_a + p[0].team_b);
        }
        assert_eq!(opponents, BTreeSet::from([2, 4]));
        teams[2].recent_series = limit;
        teams[4].recent_series = limit;
        teams[1].recent_series = limit;
        assert!(plan_batch(&elo, &teams, 10, 10, &cfg, 0).is_empty());
    }

    #[test]
    fn seeds_favour_uncertain_and_idle_teams() {
        let wl = WengLin::default();
        let cfg = MatchmakingConfig::default();
        let mut teams: Vec<TeamState> = (0..40).map(|i| team(i, 25.0)).collect();
        for t in &mut teams {
            t.last_played = Some(99);
        }
        teams[7].rating = wl.initial();
        teams[7].recent_games = 0;
        teams[7].last_played = None;
        teams[21].last_played = Some(1);
        let (mut newcomer, mut idle) = (0, 0);
        for seed in 0..2000 {
            let p = plan_batch(&wl, &teams, 100, 1, &cfg, seed)[0];
            newcomer += u32::from(p.team_a == 7 || p.team_b == 7);
            idle += u32::from(p.team_a == 21 || p.team_b == 21);
        }
        // A uniform pick would include a given team in 2/40 = 5 % of single-series batches (100).
        assert!(newcomer > 500, "newcomer in {newcomer}/2000");
        assert!(idle > 150, "idle team in {idle}/2000");
    }

    #[test]
    fn placement_picks_a_near_settled_live_opponent() {
        let wl = WengLin::default();
        let cfg = MatchmakingConfig::default();
        let mut teams: Vec<TeamState> = (0..30).map(|i| team(i, i as f64)).collect();
        teams[16].live = false;
        teams[14].rating.sigma = wl.sigma0; // provisional: skipped while settled teams remain
        let mut newcomer = team(99, 15.0);
        newcomer.rating.sigma = wl.sigma0;
        let mut picked = BTreeSet::new();
        for seed in 0..200 {
            let o = plan_placement(&wl, &newcomer, &teams, &cfg, seed).unwrap();
            picked.insert(o);
        }
        // The three nearest legal, settled teams to mu 15: 15 itself, then 13 and 17 (14 provisional, 16 not live).
        assert_eq!(picked, BTreeSet::from([13, 15, 17]));
        let mut shuffled = teams.clone();
        shuffled.reverse();
        assert_eq!(
            plan_placement(&wl, &newcomer, &teams, &cfg, 5),
            plan_placement(&wl, &newcomer, &shuffled, &cfg, 5)
        );
    }

    #[test]
    fn placement_respects_rematch_rules_and_falls_back_to_provisional_teams() {
        let wl = WengLin::default();
        let cfg = MatchmakingConfig::default();
        let mut newcomer = team(9, 25.0);
        newcomer.recent_opponents = vec![0, 1];
        let mut teams = vec![team(0, 25.0), team(1, 25.0), team(2, 40.0)];
        teams[0].recent_series = cfg.series_cap + cfg.cap_overflow;
        // 0 is past its overflow, 1 was the last opponent: 2 is the only choice, however far.
        assert_eq!(plan_placement(&wl, &newcomer, &teams, &cfg, 1), Some(2));
        let mut capped = newcomer.clone();
        capped.recent_series = cfg.series_cap;
        assert_eq!(plan_placement(&wl, &capped, &teams, &cfg, 1), None);
        teams.truncate(2);
        // Only the last opponent is left: a rematch beats no placement.
        assert_eq!(plan_placement(&wl, &newcomer, &teams, &cfg, 1), Some(1));
        teams.truncate(1);
        assert_eq!(plan_placement(&wl, &newcomer, &teams, &cfg, 1), None);
        // A fresh ladder: everyone is provisional, so anyone near will do.
        let fresh: Vec<TeamState> = (0..4)
            .map(|i| TeamState {
                rating: wl.initial(),
                ..team(i, 0.0)
            })
            .collect();
        let alone = TeamState {
            rating: wl.initial(),
            ..team(9, 0.0)
        };
        assert!(plan_placement(&wl, &alone, &fresh, &cfg, 3).is_some());
        // The newcomer itself in the list is never its own opponent.
        assert_eq!(plan_placement(&wl, &fresh[0], &fresh[..1], &cfg, 3), None);
    }
}
