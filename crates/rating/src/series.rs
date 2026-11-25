//! Expanding a pairing into the games of a series: maps, engine seeds and A/B slots.

use crate::rng::{mix64, Rng};
use crate::{GameResult, Outcome, Pairing, TeamId};

/// Who won a finished game, by engine slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Winner {
    SlotA,
    SlotB,
    Draw,
}

impl Winner {
    /// From the `outcome` string of a finished match job (`docs/JOBS.md`): `"a"`, `"b"` or `"draw"`.
    /// `"unfinished"` (a `max_ticks` stop, which also reports `winner: null`) and anything unknown
    /// give `None`: such a game has no result and must not be rated.
    pub fn from_job_outcome(outcome: &str) -> Option<Winner> {
        match outcome {
            "a" => Some(Winner::SlotA),
            "b" => Some(Winner::SlotB),
            "draw" => Some(Winner::Draw),
            _ => None,
        }
    }
}

/// One game of a series, ready to become a match job (`bot_a` = `slot_a`'s bot, `bot_b` = `slot_b`'s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PlannedGame {
    pub index: u32,
    /// Index into the map pool passed to [`plan_series`].
    pub map: u32,
    /// Engine seed, from [`game_seed`].
    pub seed: u64,
    /// Team in the engine's team-A slot (winner `0`).
    pub slot_a: TeamId,
    /// Team in the engine's team-B slot (winner `1`).
    pub slot_b: TeamId,
}

/// All games of one series between `team_a` and `team_b`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SeriesPlan {
    pub series_id: u64,
    pub team_a: TeamId,
    pub team_b: TeamId,
    pub games: Vec<PlannedGame>,
}

impl SeriesPlan {
    /// Translate a finished game's [`Winner`] (by engine slot) into a [`GameResult`] seen from the
    /// series' team A. `None` if the plan has no game with that index.
    pub fn result(&self, game_index: u32, winner: Winner) -> Option<GameResult> {
        let game = self.games.iter().find(|g| g.index == game_index)?;
        let a_in_slot_a = game.slot_a == self.team_a;
        let outcome = match winner {
            Winner::Draw => Outcome::Draw,
            Winner::SlotA if a_in_slot_a => Outcome::AWins,
            Winner::SlotB if !a_in_slot_a => Outcome::AWins,
            Winner::SlotA | Winner::SlotB => Outcome::BWins,
        };
        Some(GameResult {
            series_id: self.series_id,
            game_index,
            team_a: self.team_a,
            team_b: self.team_b,
            outcome,
        })
    }
}

/// Engine seed of game `game_index` of series `series_id`: a fixed mix of the two, so a series can be
/// re-queued or audited without storing its seeds.
pub fn game_seed(series_id: u64, game_index: u32) -> u64 {
    mix64(mix64(series_id) ^ (u64::from(game_index) + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// Plan the games of a series.
///
/// `map_usage[m]` is how many ranked games map `m` of the pool has hosted so far (all zeros is fine);
/// its length is the pool size. Maps are taken least-used first with ties broken at random from
/// `series_id`, so usage stays level across the pool over time, and no map repeats within a series
/// unless `games_per_series` exceeds the pool (then the same order is cycled). The caller adds the
/// chosen maps to its usage counts.
///
/// One game is already side-balanced (the engine swaps attack and defence at half-time). The A/B slot
/// still alternates game by game, starting from a coin flip on `series_id`, to cancel any residual
/// slot bias (spawn order, tie-breaks).
///
/// Returns a plan with no games if the pool is empty.
pub fn plan_series(
    series_id: u64,
    pairing: Pairing,
    map_usage: &[u64],
    games_per_series: u32,
) -> SeriesPlan {
    let mut rng = Rng::new(mix64(series_id ^ 0x5E71_E5A1_B0A7_C0DE));
    let mut order: Vec<(u64, u64, u32)> = map_usage
        .iter()
        .zip(0u32..)
        .map(|(&used, m)| (used, rng.next_u64(), m))
        .collect();
    order.sort_unstable();
    let a_first = rng.next_u64() & 1 == 0;
    let games = if order.is_empty() {
        0
    } else {
        games_per_series
    };
    let games = (0..games)
        .map(|index| {
            let a_in_slot_a = a_first == (index % 2 == 0);
            let (slot_a, slot_b) = if a_in_slot_a {
                (pairing.team_a, pairing.team_b)
            } else {
                (pairing.team_b, pairing.team_a)
            };
            PlannedGame {
                index,
                map: order[index as usize % order.len()].2,
                seed: game_seed(series_id, index),
                slot_a,
                slot_b,
            }
        })
        .collect();
    SeriesPlan {
        series_id,
        team_a: pairing.team_a,
        team_b: pairing.team_b,
        games,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAIR: Pairing = Pairing {
        team_a: 10,
        team_b: 20,
    };

    #[test]
    fn maps_do_not_repeat_and_slots_alternate() {
        for series_id in 0..200 {
            let plan = plan_series(series_id, PAIR, &[0; 7], 5);
            assert_eq!(plan, plan_series(series_id, PAIR, &[0; 7], 5));
            let mut maps: Vec<u32> = plan.games.iter().map(|g| g.map).collect();
            maps.sort_unstable();
            maps.dedup();
            assert_eq!(maps.len(), 5);
            for pair in plan.games.windows(2) {
                assert_eq!(pair[0].slot_a, pair[1].slot_b);
                assert_ne!(pair[0].seed, pair[1].seed);
            }
            for g in &plan.games {
                assert!(g.map < 7);
                assert_eq!(g.seed, game_seed(series_id, g.index));
                assert!([(10, 20), (20, 10)].contains(&(g.slot_a, g.slot_b)));
            }
        }
    }

    #[test]
    fn usage_stays_level_and_first_slot_is_fair() {
        let mut usage = [0u64; 5];
        let mut a_first = 0;
        for series_id in 0..1000 {
            let plan = plan_series(series_id, PAIR, &usage, 3);
            a_first += u32::from(plan.games[0].slot_a == 10);
            for g in &plan.games {
                usage[g.map as usize] += 1;
            }
            let (lo, hi) = (usage.iter().min().unwrap(), usage.iter().max().unwrap());
            assert!(hi - lo <= 1, "{usage:?}");
        }
        assert!((400..600).contains(&a_first), "{a_first}");
    }

    #[test]
    fn small_pools_cycle_and_empty_pools_plan_nothing() {
        let plan = plan_series(3, PAIR, &[0, 0], 5);
        let maps: Vec<u32> = plan.games.iter().map(|g| g.map).collect();
        assert_eq!(maps, [maps[0], maps[1], maps[0], maps[1], maps[0]]);
        assert_ne!(maps[0], maps[1]);
        assert!(plan_series(3, PAIR, &[], 5).games.is_empty());
    }

    #[test]
    fn results_are_seen_from_team_a() {
        let plan = plan_series(42, PAIR, &[0; 3], 2);
        for g in &plan.games {
            let (a_slot, b_slot) = if g.slot_a == 10 {
                (Winner::SlotA, Winner::SlotB)
            } else {
                (Winner::SlotB, Winner::SlotA)
            };
            assert_eq!(
                plan.result(g.index, a_slot).unwrap().outcome,
                Outcome::AWins
            );
            assert_eq!(
                plan.result(g.index, b_slot).unwrap().outcome,
                Outcome::BWins
            );
            assert_eq!(
                plan.result(g.index, Winner::Draw).unwrap().outcome,
                Outcome::Draw
            );
        }
        assert_eq!(Winner::from_job_outcome("a"), Some(Winner::SlotA));
        assert_eq!(Winner::from_job_outcome("b"), Some(Winner::SlotB));
        assert_eq!(Winner::from_job_outcome("draw"), Some(Winner::Draw));
        assert_eq!(Winner::from_job_outcome("unfinished"), None);
        assert!(plan.result(2, Winner::SlotA).is_none());
    }
}
