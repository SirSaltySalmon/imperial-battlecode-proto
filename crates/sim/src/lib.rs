//! Native simulation harness. Bots are plain Rust values driven through `ibc::native::with_host`, so a
//! match runs at native speed with a normal debugger and no wasm runtime. The engine calls made are
//! exactly the ones the wasm runner makes (`ibc_abi::Host`), so results match hash for hash as long as
//! the bots stay within their fuel budget (fuel is not modelled here).

use ibc::Bot;
use ibc_engine::{Map, Match, Ruleset};

/// Names accepted by `make_bot`.
pub const BOTS: [&str; 9] =
    ["idle", "rusher", "holder", "utility", "sapper", "retaker", "lurker", "saver", "forcer"];

/// Default safety cap on ticks; the engine ends matches itself well before this.
pub const DEFAULT_MAX_TICKS: u32 = 20_000;

/// Construct a reference bot by name. Must be called inside `ibc::native::with_host`
/// (bots read rules and the map in `new`).
pub fn make_bot(name: &str) -> Option<Box<dyn Bot>> {
    Some(match name {
        "idle" => Box::new(idle::Idle::new()),
        "rusher" => Box::new(rusher::Rusher::new()),
        "holder" => Box::new(holder::Holder::new()),
        "utility" => Box::new(utility::Utility::new()),
        "sapper" => Box::new(sapper::Sapper::new()),
        "retaker" => Box::new(retaker::Retaker::new()),
        "lurker" => Box::new(lurker::Lurker::new()),
        "saver" => Box::new(saver::Saver::new()),
        "forcer" => Box::new(forcer::Forcer::new()),
        _ => return None,
    })
}

/// `act` calls made by one team's bots and how many returned an error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActStats {
    pub calls: u64,
    pub errors: u64,
    /// Errors by action kind (`A_*`).
    pub errors_by_kind: [u64; 16],
}

#[derive(Clone, Debug)]
pub struct SimResult {
    /// The engine reported the match as finished (false = `max_ticks` hit first).
    pub finished: bool,
    pub winner: Option<i32>,
    pub score: [i32; 2],
    pub ticks: u32,
    pub final_hash: u64,
    pub summary_json: String,
    /// Per team (A, B).
    pub acts: [ActStats; 2],
}

/// Play `bot_a` (team A) against `bot_b` (team B) on a map with the built-in ruleset.
pub fn run(map_text: &str, seed: u64, bot_a: &str, bot_b: &str, max_ticks: u32) -> Result<SimResult, String> {
    run_with(map_text, seed, bot_a, bot_b, max_ticks, |_| {})
}

/// Like `run`, calling `on_tick` with the match before the first tick and after every step.
pub fn run_with(
    map_text: &str,
    seed: u64,
    bot_a: &str,
    bot_b: &str,
    max_ticks: u32,
    on_tick: impl FnMut(&Match),
) -> Result<SimResult, String> {
    for name in [bot_a, bot_b] {
        if !BOTS.contains(&name) {
            return Err(format!("unknown bot '{name}' (known: {})", BOTS.join(", ")));
        }
    }
    let names = [bot_a, bot_b];
    run_custom(map_text, seed, max_ticks, |team| make_bot(names[team]).expect("name checked above"), on_tick)
}

/// The general form: `make(team)` builds the bot for each unit of team 0 (A) or 1 (B), so tests and
/// experiments can pit their own `ibc::Bot` types against the reference bots.
pub fn run_custom(
    map_text: &str,
    seed: u64,
    max_ticks: u32,
    mut make: impl FnMut(usize) -> Box<dyn Bot>,
    mut on_tick: impl FnMut(&Match),
) -> Result<SimResult, String> {
    let map = Map::parse(map_text)?;
    let mut m = Match::new(Ruleset::builtin(), map, seed);
    let mut bots: Vec<Option<Box<dyn Bot>>> = (0..m.unit_count()).map(|_| None).collect();
    let mut acts = [ActStats::default(); 2];

    on_tick(&m);
    while !m.finished() && m.tick() < max_ticks {
        for unit in m.turn_order() {
            let team = (m.unit_team(unit) & 1) as usize;
            let slot = &mut bots[unit as usize];
            ibc::native::reset_stats();
            ibc::native::with_host(&mut m, unit, || {
                // Like a wasm instance, a bot is created on the first tick its unit runs.
                slot.get_or_insert_with(|| make(team)).turn();
            });
            let s = ibc::native::stats();
            acts[team].calls += s.act_calls;
            acts[team].errors += s.act_errors;
            for (total, n) in acts[team].errors_by_kind.iter_mut().zip(s.errors_by_kind) {
                *total += n;
            }
        }
        m.take_inputs();
        m.step();
        on_tick(&m);
    }
    Ok(SimResult {
        finished: m.finished(),
        winner: m.winner(),
        score: m.score(),
        ticks: m.tick(),
        final_hash: m.state_hash(),
        summary_json: m.summary_json(),
        acts,
    })
}

/// Per team (A, B): rounds won while attacking and rounds played as attackers, read from a match
/// summary. Returns `None` if the summary has no per-round records.
pub fn attacker_rounds(summary_json: &str) -> Option<[(u32, u32); 2]> {
    let v: serde_json::Value = serde_json::from_str(summary_json).ok()?;
    let rounds = v.get("rounds")?.as_array()?;
    let mut out = [(0, 0); 2];
    for r in rounds {
        let (winner, attackers) = (r.get("winner")?.as_i64()?, r.get("attackers")?.as_i64()?);
        let team = out.get_mut(attackers as usize)?;
        team.0 += (winner == attackers) as u32;
        team.1 += 1;
    }
    Some(out)
}
