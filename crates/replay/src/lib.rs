//! Replay container, match summary and verification.
//!
//! A `.ibcr` file is:
//!
//! ```text
//! "IBCR"                      4 bytes magic
//! format version              u32 little-endian (currently 1)
//! DEFLATE(payload)            raw deflate stream (no zlib header)
//!
//! payload = header JSON (one line, serde_json of `Header`) '\n' body
//! body    = varint tick_count, then per tick:
//!             varint event_count, then per event one of
//!               0x00 Act      varint unit, varint kind, zigzag a, zigzag b, zigzag c
//!               0x01 MemWrite varint unit, varint scope, varint off, varint len, bytes
//!               0x02 Fault    varint unit          (the bot of this unit crashed at this point of the tick)
//!             state hash after the step, u64 little-endian
//! ```
//!
//! A replay holds everything needed to re-simulate a match without the bots: the map text, the seed and
//! every accepted input in order. `verify` re-runs it on `ibc-engine` and compares every state hash.
//!
//! Nothing here touches the filesystem, so the crate also builds for `wasm32-unknown-unknown` (the viewer).

use ibc_engine::{Input, Map, Match, Ruleset};
use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8; 4] = b"IBCR";
pub const FORMAT_VERSION: u32 = 1;

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
    /// Language fuel multiplier applied to the per-tick budget of the engine.
    pub fuel_multiplier: f64,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Input(Input),
    /// The bot of this unit crashed; the engine was told with `Match::fault`.
    Fault(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TickRecord {
    pub events: Vec<Event>,
    /// `Match::state_hash()` after the step.
    pub hash: u64,
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
    current: Vec<Event>,
}

impl ReplayWriter {
    /// `header.result` is a placeholder until `finish`.
    pub fn new(header: Header) -> ReplayWriter {
        ReplayWriter { header, ticks: Vec::new(), current: Vec::new() }
    }

    pub fn input(&mut self, input: Input) {
        self.current.push(Event::Input(input));
    }

    pub fn inputs(&mut self, inputs: Vec<Input>) {
        self.current.extend(inputs.into_iter().map(Event::Input));
    }

    pub fn fault(&mut self, unit: u32) {
        self.current.push(Event::Fault(unit));
    }

    /// Close the current tick with the post-step state hash.
    pub fn end_tick(&mut self, hash: u64) {
        let events = std::mem::take(&mut self.current);
        self.ticks.push(TickRecord { events, hash });
    }

    pub fn ticks(&self) -> u32 {
        self.ticks.len() as u32
    }

    pub fn finish(mut self, result: MatchResult) -> Replay {
        self.header.result = result;
        Replay { header: self.header, ticks: self.ticks }
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_zigzag(out: &mut Vec<u8>, v: i32) {
    put_varint(out, ((v << 1) ^ (v >> 31)) as u32 as u64);
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.pos).ok_or("replay body truncated")?;
        self.pos += 1;
        Ok(v)
    }
    fn varint(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            if shift >= 64 {
                return Err("replay body: varint too long".into());
            }
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }
    fn u32(&mut self) -> Result<u32, String> {
        u32::try_from(self.varint()?).map_err(|_| "replay body: value out of range".to_string())
    }
    fn zigzag(&mut self) -> Result<i32, String> {
        let v = self.u32()?;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len()).ok_or("replay body truncated")?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
}

impl Replay {
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = serde_json::to_vec(&self.header).expect("header serializes");
        payload.push(b'\n');
        put_varint(&mut payload, self.ticks.len() as u64);
        for t in &self.ticks {
            put_varint(&mut payload, t.events.len() as u64);
            for e in &t.events {
                match e {
                    Event::Input(Input::Act { unit, kind, a, b, c }) => {
                        payload.push(0);
                        put_varint(&mut payload, *unit as u64);
                        put_varint(&mut payload, *kind as u64);
                        put_zigzag(&mut payload, *a);
                        put_zigzag(&mut payload, *b);
                        put_zigzag(&mut payload, *c);
                    }
                    Event::Input(Input::MemWrite { unit, scope, off, data }) => {
                        payload.push(1);
                        put_varint(&mut payload, *unit as u64);
                        put_varint(&mut payload, *scope as u64);
                        put_varint(&mut payload, *off as u64);
                        put_varint(&mut payload, data.len() as u64);
                        payload.extend_from_slice(data);
                    }
                    Event::Fault(unit) => {
                        payload.push(2);
                        put_varint(&mut payload, *unit as u64);
                    }
                }
            }
            payload.extend_from_slice(&t.hash.to_le_bytes());
        }
        let mut out = Vec::with_capacity(payload.len() / 3 + 16);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(&payload, 8));
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Replay, String> {
        if bytes.len() < 8 || &bytes[..4] != MAGIC {
            return Err("not an .ibcr replay (bad magic)".into());
        }
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(format!("unsupported replay format version {version} (this build reads {FORMAT_VERSION})"));
        }
        let payload = miniz_oxide::inflate::decompress_to_vec_with_limit(&bytes[8..], 1 << 30)
            .map_err(|e| format!("replay is corrupt (inflate: {e})"))?;
        let nl = payload.iter().position(|&b| b == b'\n').ok_or("replay is corrupt (no header line)")?;
        let header: Header =
            serde_json::from_slice(&payload[..nl]).map_err(|e| format!("replay header is invalid: {e}"))?;
        let mut r = Reader { b: &payload[nl + 1..], pos: 0 };
        let n = r.varint()? as usize;
        let mut ticks = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            let ne = r.varint()? as usize;
            let mut events = Vec::with_capacity(ne.min(1 << 16));
            for _ in 0..ne {
                events.push(match r.byte()? {
                    0 => Event::Input(Input::Act {
                        unit: r.u32()?,
                        kind: r.u32()?,
                        a: r.zigzag()?,
                        b: r.zigzag()?,
                        c: r.zigzag()?,
                    }),
                    1 => {
                        let unit = r.u32()?;
                        let scope = r.u32()?;
                        let off = r.u32()?;
                        let len = r.varint()? as usize;
                        Event::Input(Input::MemWrite { unit, scope, off, data: r.take(len)?.to_vec() })
                    }
                    2 => Event::Fault(r.u32()?),
                    t => return Err(format!("replay body: unknown event tag {t}")),
                });
            }
            let hash = u64::from_le_bytes(r.take(8)?.try_into().unwrap());
            ticks.push(TickRecord { events, hash });
        }
        if r.pos != r.b.len() {
            return Err("replay body: trailing bytes".into());
        }
        Ok(Replay { header, ticks })
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
        let rules = Ruleset::builtin();
        if hash_hex(rules.hash()) != h.ruleset_hash {
            return Err(format!(
                "ruleset mismatch: replay has {}, this build has {}",
                h.ruleset_hash,
                hash_hex(rules.hash())
            ));
        }
        let map = Map::parse(&h.map_text).map_err(|e| format!("replay map does not parse: {e}"))?;
        if hash_hex(map.hash()) != h.map_hash {
            return Err(format!("map hash mismatch: header {}, parsed {}", h.map_hash, hash_hex(map.hash())));
        }
        Ok(Match::new(rules, map, h.seed))
    }

    /// Re-simulate, calling `on_frame` with the match before the first tick and after every step.
    /// Stops with an error at the first state hash that differs.
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
            let got = m.state_hash();
            if got != t.hash {
                return Err(format!(
                    "state hash mismatch after tick {i}: replay {}, re-simulated {}",
                    hash_hex(t.hash),
                    hash_hex(got)
                ));
            }
            on_frame(&m);
        }
        Ok(m)
    }
}

/// Re-simulate the replay and compare every per-tick state hash and the final result.
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

// ---- summary ----

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
