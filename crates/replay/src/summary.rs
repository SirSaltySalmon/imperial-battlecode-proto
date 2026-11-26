//! `summary.json`: what a finished match produced, without re-simulating it.

use crate::MatchResult;
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
