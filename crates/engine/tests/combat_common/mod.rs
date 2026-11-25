//! Shared set-up for the `combat_*` tests: small arenas, and units placed and armed directly (the shop only
//! sells what `rules/economy.toml` lists, so tests reach every weapon by setting `unit.weapons`).
#![allow(dead_code)]

use ibc_engine::abi::*;
use ibc_engine::fixed::V2;
use ibc_engine::{Map, Match, Ruleset};

/// An empty walled room of `w` x `h` tiles for `team` units a side. Spawns are in the left and right
/// columns, the site in the bottom-right corner.
pub fn arena(w: usize, h: usize, team: usize) -> String {
    let mut grid = vec![vec![b'.'; w]; h];
    for (y, row) in grid.iter_mut().enumerate() {
        for (x, t) in row.iter_mut().enumerate() {
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                *t = b'#';
            }
        }
    }
    for i in 0..team {
        grid[1 + i][1] = b'a';
        grid[1 + i][w - 2] = b'd';
    }
    grid[h - 2][w - 2] = b'1';
    with_grid("arena", team, &grid)
}

fn with_grid(name: &str, team: usize, grid: &[Vec<u8>]) -> String {
    let rows: Vec<String> = grid
        .iter()
        .map(|r| String::from_utf8(r.clone()).unwrap())
        .collect();
    format!(
        "name: {name}\nteam_size: {team}\n---\n{}\n",
        rows.join("\n")
    )
}

/// A match on `map`, stepped to the first tick of the first action phase.
pub fn start(map: &str, seed: u64) -> Match {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(map).unwrap(), seed);
    while m.world.phase != PHASE_ACTION {
        m.step();
    }
    m
}

/// Centre of a tile, or any point given in tiles and 1/1024ths.
pub fn at(tx: i32, ty: i32) -> V2 {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

pub fn weapon_id(rules: &Ruleset, name: &str) -> i32 {
    rules
        .weapon_names
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("no weapon `{name}`")) as i32
}

pub fn weapon(m: &Match, id: i32) -> WeaponRules {
    m.world.rules.weapons[id as usize]
}

/// Put `weapon` (full magazine) into its slot and into the unit's hands, with settled weapon state.
pub fn arm(m: &mut Match, u: usize, weapon: i32) {
    let wr = m.world.rules.weapons[weapon as usize];
    let unit = &mut m.world.units[u];
    unit.weapons[wr.slot as usize] = weapon;
    unit.ammo[wr.slot as usize] = wr.magazine;
    unit.slot = wr.slot as usize;
    unit.combat = Default::default();
}

pub fn place(m: &mut Match, u: usize, pos: V2, facing: i32) {
    let unit = &mut m.world.units[u];
    unit.pos = pos;
    unit.vel = (0, 0);
    unit.facing = facing & 0xFFFF;
}

/// Total trigger pulls of a unit so far.
pub fn shots(m: &Match, u: usize) -> i32 {
    m.world.units[u].stats.shots
}

pub fn weapon_drops(m: &Match) -> Vec<DropInfo> {
    m.world
        .drops
        .iter()
        .copied()
        .filter(|d| d.kind == DROP_WEAPON)
        .collect()
}

/// Small deterministic generator for property-style tests (xorshift64*).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Uniform-ish in `0..n`.
    pub fn below(&mut self, n: i32) -> i32 {
        ((self.next() >> 33) % n.max(1) as u64) as i32
    }
}
