//! Replay container, match summary and verification.
//!
//! A replay holds everything needed to re-simulate a match without the bots: the map text, the rules, the seed
//! and every accepted input in order. `verify` re-runs it on `ibc-engine` and compares every state hash. Next
//! to the simulation inputs it carries per-team records that never feed a hash (logs, per-turn fuel, debug
//! drawings). A team's memory writes and records are private to that team: `Replay::redact` removes them and
//! the result still verifies, because the per-tick hash is split into a world hash and one memory hash per
//! team. The byte format is in `codec` and docs/REPLAY.md.
//!
//! Nothing here touches the filesystem, so the crate also builds for `wasm32-unknown-unknown` (the viewer).

mod codec;
mod language;
mod records;
mod redact;
mod summary;

pub use ibc_engine::{DebugDraw, StateHashes};
pub use language::{detect_languages, LANGUAGE_SECTION};
pub use records::records_json;
pub use redact::Keep;
pub use summary::{Fault, Summary, TeamFuel, TeamSummary};

use ibc_engine::{Input, Map, Match, RulesSource, Ruleset};
use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8; 4] = b"IBCR";
pub const FORMAT_VERSION: u32 = 2;

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 64-bit hashes are written as 16 hex digits so JSON consumers never lose precision.
pub fn hash_hex(h: u64) -> String {
    format!("{h:016x}")
}

pub fn parse_hash_hex(s: &str) -> Result<u64, String> {
    u64::from_str_radix(s, 16).map_err(|e| format!("bad hash {s:?}: {e}"))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotInfo {
    pub name: String,
    /// SHA-256 of the wasm module of this team, hex.
    pub sha256: String,
    /// Fuel multiplier applied to the per-tick budget of the engine: the language's row of the ruleset, unless
    /// the run overrode it.
    pub fuel_multiplier: f64,
    /// The bot's language, as the runner resolved it ("" in replays recorded before per-language rows).
    #[serde(default)]
    pub language: String,
    /// Multiplier applied to the ruleset's `memory_bytes` for this team's units (1.0 before per-language rows).
    #[serde(default = "one")]
    pub memory_multiplier: f64,
}

fn one() -> f64 {
    1.0
}

/// How the match ended. Stored in the header so a replay can be checked against it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchResult {
    /// 0 = team A, 1 = team B, null = draw or unfinished.
    pub winner: Option<i32>,
    pub score: [i32; 2],
    /// Ticks simulated.
    pub ticks: u32,
    /// False if the run stopped at a `max_ticks` cap before the engine ended the match.
    pub finished: bool,
}

/// The rules TOML texts (`rules/*.toml`) the match was played under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderRules {
    pub core: String,
    pub weapons: String,
    pub abilities: String,
    pub economy: String,
}

impl HeaderRules {
    /// The source of `rules`, if it still parses to `rules` (a ruleset edited after parsing does not).
    pub fn of(rules: &Ruleset) -> Option<HeaderRules> {
        let s = &rules.source;
        let same = s.parse().is_ok_and(|r| r.hash() == rules.hash());
        same.then(|| HeaderRules {
            core: s.core.clone(),
            weapons: s.weapons.clone(),
            abilities: s.abilities.clone(),
            economy: s.economy.clone(),
        })
    }

    pub fn parse(&self) -> Result<Ruleset, String> {
        RulesSource {
            core: self.core.clone(),
            weapons: self.weapons.clone(),
            abilities: self.abilities.clone(),
            economy: self.economy.clone(),
        }
        .parse()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub engine_version: String,
    /// Hex, see `hash_hex`.
    pub ruleset_hash: String,
    pub map_hash: String,
    pub map_text: String,
    pub seed: u64,
    /// Wasmtime version that executed the bots ("" if none, e.g. a native simulation).
    pub wasmtime_version: String,
    pub abi_version: u32,
    /// Team A, team B.
    pub bots: [BotInfo; 2],
    pub result: MatchResult,
    /// Units per team: unit ids `0..team_size` are team A, the next `team_size` team B. Lets a replay be
    /// redacted without the engine that recorded it.
    pub team_size: u32,
    /// Teams whose memory writes, memory hashes and records were removed (`Replay::redact`).
    #[serde(default)]
    pub redacted: [bool; 2],
    /// The rules of the match. `None` if the recorder ran a ruleset it had no source for; readers then use
    /// their built-in rules, which must hash to `ruleset_hash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<HeaderRules>,
}

impl Header {
    /// Team (0 or 1) of a unit id, by the engine's numbering.
    pub fn unit_team(&self, unit: u32) -> usize {
        (unit >= self.team_size) as usize
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Input(Input),
    /// The bot of this unit crashed; the engine was told with `Match::fault`.
    Fault(u32),
}

/// What a team's bots produced that is not a simulation input. Never part of any hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    /// Bytes the unit wrote to its log (`ibc.log`, stdout, stderr) during its turn this tick.
    Log { unit: u32, text: Vec<u8> },
    /// Fuel the unit used in its turn this tick; `out_of_fuel` if the budget ran out before `end_turn`.
    Fuel { unit: u32, used: u64, out_of_fuel: bool },
    /// A `debug_draw` call made this tick.
    Draw(DebugDraw),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TickRecord {
    pub events: Vec<Event>,
    /// `Match::state_hashes()` after the step. The memory hash of a redacted team is 0.
    pub hashes: StateHashes,
    /// Records of team A and team B, in the order they happened. Empty for a redacted team.
    pub private: [Vec<Record>; 2],
}

impl TickRecord {
    /// `Match::state_hash()` after the step. Meaningless on a redacted replay.
    pub fn state_hash(&self) -> u64 {
        self.hashes.combined()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Replay {
    pub header: Header,
    pub ticks: Vec<TickRecord>,
}

/// Builds a replay tick by tick while a match runs.
pub struct ReplayWriter {
    header: Header,
    ticks: Vec<TickRecord>,
    current: TickRecord,
}

impl ReplayWriter {
    /// `header.result` is a placeholder until `finish`.
    pub fn new(header: Header) -> ReplayWriter {
        ReplayWriter { header, ticks: Vec::new(), current: TickRecord::default() }
    }

    pub fn input(&mut self, input: Input) {
        self.current.events.push(Event::Input(input));
    }

    pub fn inputs(&mut self, inputs: Vec<Input>) {
        self.current.events.extend(inputs.into_iter().map(Event::Input));
    }

    pub fn fault(&mut self, unit: u32) {
        self.current.events.push(Event::Fault(unit));
    }

    /// A non-simulation record of the current tick for `team`.
    pub fn record(&mut self, team: usize, record: Record) {
        self.current.private[team & 1].push(record);
    }

    /// Debug drawings of the current tick (`Match::take_debug`), filed under each unit's team.
    pub fn draws(&mut self, draws: Vec<DebugDraw>) {
        for d in draws {
            let team = self.header.unit_team(d.0);
            self.record(team, Record::Draw(d));
        }
    }

    /// Close the current tick with the post-step state hashes.
    pub fn end_tick(&mut self, hashes: StateHashes) {
        let mut tick = std::mem::take(&mut self.current);
        tick.hashes = hashes;
        self.ticks.push(tick);
    }

    pub fn ticks(&self) -> u32 {
        self.ticks.len() as u32
    }

    pub fn finish(mut self, result: MatchResult) -> Replay {
        self.header.result = result;
        Replay { header: self.header, ticks: self.ticks }
    }
}

impl Replay {
    /// The ruleset to re-simulate with: the header's own rules if present, else this build's built-in rules.
    /// Either must hash to the header's `ruleset_hash`.
    pub fn rules(&self) -> Result<Ruleset, String> {
        let want = &self.header.ruleset_hash;
        let (rules, whose) = match &self.header.rules {
            Some(r) => (r.parse().map_err(|e| format!("replay header rules do not parse: {e}"))?, "header rules"),
            None => (Ruleset::builtin(), "this build"),
        };
        let got = hash_hex(rules.hash());
        if &got != want {
            return Err(format!("ruleset mismatch: replay has {want}, {whose} hash to {got}"));
        }
        Ok(rules)
    }

    /// A fresh match in the initial state of the replay. Fails if the engine, rules or map parser of this
    /// build do not reproduce the recorded identity.
    pub fn new_match(&self) -> Result<Match, String> {
        let h = &self.header;
        if h.engine_version != ibc_engine::ENGINE_VERSION {
            return Err(format!(
                "replay was recorded with engine {} but this build is {}",
                h.engine_version,
                ibc_engine::ENGINE_VERSION
            ));
        }
        let rules = self.rules()?;
        let map = Map::parse(&h.map_text).map_err(|e| format!("replay map does not parse: {e}"))?;
        if hash_hex(map.hash()) != h.map_hash {
            return Err(format!("map hash mismatch: header {}, parsed {}", h.map_hash, hash_hex(map.hash())));
        }
        Ok(Match::new(rules, map, h.seed))
    }

    /// Re-simulate, calling `on_frame` with the match before the first tick and after every step.
    /// Stops with an error at the first hash that differs: the world hash on every tick, and the memory hash
    /// of each team that is not redacted.
    pub fn simulate(&self, mut on_frame: impl FnMut(&Match)) -> Result<Match, String> {
        let mut m = self.new_match()?;
        on_frame(&m);
        for (i, t) in self.ticks.iter().enumerate() {
            for e in &t.events {
                match e {
                    Event::Input(input) => m.apply_input(input),
                    Event::Fault(unit) => m.fault(*unit),
                }
            }
            m.step();
            self.check_tick(i, t, &m.state_hashes())?;
            on_frame(&m);
        }
        Ok(m)
    }

    /// Compare re-simulated hashes after tick `i` with the record: the world hash, and the memory hash of each
    /// team that is not redacted.
    pub fn check_tick(&self, i: usize, t: &TickRecord, got: &StateHashes) -> Result<(), String> {
        if got.world != t.hashes.world {
            return Err(format!(
                "state hash mismatch after tick {i}: replay {}, re-simulated {}",
                hash_hex(t.hashes.world),
                hash_hex(got.world)
            ));
        }
        for team in 0..2 {
            if !self.header.redacted[team] && got.memory[team] != t.hashes.memory[team] {
                return Err(format!(
                    "team {} memory hash mismatch after tick {i}: replay {}, re-simulated {}",
                    ["A", "B"][team],
                    hash_hex(t.hashes.memory[team]),
                    hash_hex(got.memory[team])
                ));
            }
        }
        Ok(())
    }
}

/// Re-simulate the replay and compare every per-tick hash and the final result.
pub fn verify(replay: &Replay) -> Result<(), String> {
    let m = replay.simulate(|_| {})?;
    let r = &replay.header.result;
    if r.ticks as usize != replay.ticks.len() {
        return Err(format!("header says {} ticks but the body has {}", r.ticks, replay.ticks.len()));
    }
    if m.score() != r.score {
        return Err(format!("final score mismatch: replay {:?}, re-simulated {:?}", r.score, m.score()));
    }
    if m.winner() != r.winner {
        return Err(format!("winner mismatch: replay {:?}, re-simulated {:?}", r.winner, m.winner()));
    }
    if m.finished() != r.finished {
        return Err(format!("finished flag mismatch: replay {}, re-simulated {}", r.finished, m.finished()));
    }
    Ok(())
}
