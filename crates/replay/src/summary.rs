//! `summary.json`: what a finished match produced, without re-simulating it.

use crate::{MatchResult, Record, Replay};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamFuel {
    /// Fuel consumed by all units of the team over the match (wasm instructions + host call charges).
    pub total: u64,
    /// Fuel the units of the team were entitled to (sum of per-turn budgets).
    pub granted: u64,
    /// Most fuel the team used in one tick (all units together).
    pub max_per_tick: u64,
    /// Most fuel one unit used in one turn.
    pub max_unit_turn: u64,
    /// Bot turns taken.
    pub turns: u64,
    /// Turns that ended because the budget ran out instead of by `end_turn`.
    pub out_of_fuel_turns: u64,
    /// Fuel one unit used in one turn, over every turn of the team: nearest-rank percentiles (the maximum is
    /// `max_unit_turn`). 0 without turns.
    #[serde(default)]
    pub turn_p50: u64,
    #[serde(default)]
    pub turn_p90: u64,
    #[serde(default)]
    pub turn_p99: u64,
}

impl TeamFuel {
    /// Set the per-turn percentiles from `team`'s `Fuel` records in `replay` (none if that team is redacted).
    pub fn set_turn_percentiles(&mut self, replay: &Replay, team: usize) {
        let mut used: Vec<u64> = replay
            .ticks
            .iter()
            .flat_map(|t| &t.private[team & 1])
            .filter_map(|r| match r {
                Record::Fuel { used, .. } => Some(*used),
                _ => None,
            })
            .collect();
        used.sort_unstable();
        let at = |p| nearest_rank(&used, p);
        (self.turn_p50, self.turn_p90, self.turn_p99) = (at(50), at(90), at(99));
    }
}

/// The `p`-th percentile of `sorted` by nearest rank: the smallest value with at least `p`% of the values at
/// or below it. 0 for an empty list.
fn nearest_rank(sorted: &[u64], p: usize) -> u64 {
    match sorted.len() {
        0 => 0,
        n => sorted[(p * n).div_ceil(100).max(1) - 1],
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fault {
    pub unit: u32,
    pub team: i32,
    pub tick: u32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeamSummary {
    pub name: String,
    pub wasm_sha256: String,
    pub fuel_multiplier: f64,
    pub fuel: TeamFuel,
    pub faults: u32,
}

/// What `summary.json` contains. Deterministic: no wall-clock times, no paths.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// "a", "b", "draw" or "unfinished".
    pub outcome: String,
    pub winner: Option<i32>,
    pub score: [i32; 2],
    pub ticks: u32,
    pub teams: [TeamSummary; 2],
    pub faults: Vec<Fault>,
    pub seed: u64,
    pub engine_version: String,
    pub wasmtime_version: String,
    pub abi_version: u32,
    pub ruleset_hash: String,
    pub map_hash: String,
    /// State hash after the last tick.
    pub final_state_hash: String,
    /// SHA-256 of the encoded `.ibcr` bytes.
    pub replay_sha256: String,
    /// `Match::summary_json()` of the engine (game statistics).
    pub game: serde_json::Value,
}

impl Summary {
    pub fn outcome_of(result: &MatchResult) -> &'static str {
        match (result.finished, result.winner) {
            (false, _) => "unfinished",
            (true, Some(0)) => "a",
            (true, Some(_)) => "b",
            (true, None) => "draw",
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("summary serializes");
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::nearest_rank;

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<u64> = (1..=200).collect();
        assert_eq!([50, 90, 99, 100].map(|p| nearest_rank(&v, p)), [100, 180, 198, 200]);
        assert_eq!([50, 90, 99].map(|p| nearest_rank(&[7, 9], p)), [7, 9, 9]);
        assert_eq!(nearest_rank(&[5], 50), 5);
        assert_eq!(nearest_rank(&[], 99), 0);
    }
}
