//! Imperial Battlecode engine: a deterministic 2D attackers-vs-defenders tactical shooter.
//!
//! Rules of this crate (they keep matches bit-identical on every machine, natively and in wasm):
//! integer and fixed-point math only (no f32/f64 in simulation), no HashMap iteration, no usize in state or
//! output, no clocks, no I/O. See docs/RULES.md for the game and docs/ABI.md for what bots see.

pub mod abilities;
pub mod combat;
pub mod economy;
pub mod fixed;
mod host;
pub mod map;
pub mod rules;
pub mod world;

pub use ibc_abi as abi;
pub use map::Map;
pub use rules::Ruleset;
pub use world::World;

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// FNV-1a 64. Used for state, map and ruleset hashes.
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Fnv {
        Fnv(0xcbf29ce484222325)
    }
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ x as u64).wrapping_mul(0x100000001b3);
        }
        // length suffix so adjacent fields cannot run together
        self.0 = (self.0 ^ b.len() as u64).wrapping_mul(0x100000001b3);
    }
    pub fn i32(&mut self, v: i32) {
        for x in v.to_le_bytes() {
            self.0 = (self.0 ^ x as u64).wrapping_mul(0x100000001b3);
        }
    }
    pub fn u64(&mut self, v: u64) {
        for x in v.to_le_bytes() {
            self.0 = (self.0 ^ x as u64).wrapping_mul(0x100000001b3);
        }
    }
    pub fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Fnv {
    fn default() -> Self {
        Fnv::new()
    }
}

/// Everything a bot can do to the world. A replay is the list of these per tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Act { unit: u32, kind: u32, a: i32, b: i32, c: i32 },
    MemWrite { unit: u32, scope: u32, off: u32, data: Vec<u8> },
}

/// One match. Drive it with: for each unit in `turn_order()` let its bot call the `Host` methods, then
/// `take_inputs()` (for the replay) and `step()`.
pub struct Match {
    pub world: World,
    inputs: Vec<Input>,
}

impl Match {
    pub fn new(rules: Ruleset, map: Map, seed: u64) -> Match {
        let mut world = World::new(rules, map, seed);
        // Tick 0 is the draft tick; nothing to see yet.
        world.events.clear();
        Match { world, inputs: Vec::new() }
    }

    pub fn unit_count(&self) -> u32 {
        self.world.units.len() as u32
    }

    pub fn unit_team(&self, unit: u32) -> i32 {
        self.world.units.get(unit as usize).map(|u| u.team).unwrap_or(-1)
    }

    /// Units whose bots run this tick. Order alternates teams and rotates with the tick, so neither team
    /// always writes shared memory first. Dead units only run outside the action phases.
    pub fn turn_order(&self) -> Vec<u32> {
        let w = &self.world;
        if w.phase == abi::PHASE_MATCH_END {
            return Vec::new();
        }
        let n = w.team_size as u32;
        let mut out = Vec::with_capacity(2 * n as usize);
        let first = w.tick % 2;
        for i in 0..n {
            let slot = (i + w.tick) % n;
            for t in 0..2 {
                let id = ((first + t) % 2) * n + slot;
                let u = &w.units[id as usize];
                if u.faulted || (w.in_action() && !u.alive) {
                    continue;
                }
                out.push(id);
            }
        }
        out
    }

    /// Fuel for this unit this tick, before any per-language multiplier.
    pub fn fuel_budget(&self, _unit: u32) -> u64 {
        let c = &self.world.rules.core;
        let mult = match self.world.phase {
            abi::PHASE_DRAFT => c.fuel_first_tick_mult,
            abi::PHASE_BUY => c.fuel_buy_mult,
            _ => 1,
        };
        (c.fuel_per_tick.max(0) as u64) * (mult.max(1) as u64)
    }

    /// Inputs accepted since the last call. Call right before `step()` when recording a replay.
    pub fn take_inputs(&mut self) -> Vec<Input> {
        std::mem::take(&mut self.inputs)
    }

    /// Replay path: re-apply a recorded input.
    pub fn apply_input(&mut self, input: &Input) {
        use abi::Host;
        match input {
            Input::Act { unit, kind, a, b, c } => {
                self.act(*unit, *kind, *a, *b, *c);
            }
            Input::MemWrite { unit, scope, off, data } => {
                self.mem_write(*unit, *scope, *off, data);
            }
        }
    }

    pub fn step(&mut self) {
        self.world.step();
    }

    pub fn finished(&self) -> bool {
        self.world.phase == abi::PHASE_MATCH_END
    }

    pub fn winner(&self) -> Option<i32> {
        self.world.winner
    }

    pub fn score(&self) -> [i32; 2] {
        self.world.score
    }

    pub fn tick(&self) -> u32 {
        self.world.tick
    }

    pub fn state_hash(&self) -> u64 {
        self.world.state_hash()
    }

    /// The bot of this unit crashed: it idles for the rest of the match.
    pub fn fault(&mut self, unit: u32) {
        if let Some(u) = self.world.units.get_mut(unit as usize) {
            u.faulted = true;
        }
    }

    /// Full, unfogged state of the current tick as one JSON object (viewer and analysis tools).
    pub fn frame_json(&self) -> String {
        host::frame_json(&self.world)
    }

    /// End-of-match statistics as JSON.
    pub fn summary_json(&self) -> String {
        host::summary_json(&self.world)
    }
}
