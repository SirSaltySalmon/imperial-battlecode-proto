//! The same small arenas the combat tests build. Scripts below place and arm units directly, as the tests do:
//! the shop only sells what `rules/economy.toml` lists.

use ibc_engine::abi::*;
use ibc_engine::fixed::V2;
use ibc_engine::{Map, Match, Ruleset};

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

/// Two rooms, a doorway column, and a pair of walls that touch only at a corner.
/// Same text as `combat_ballistics::WALLED`.
pub const WALLED: &str = "name: walled\nteam_size: 2\n---\n\
####################\n\
#a.......#........d#\n\
#a.......#........d#\n\
#...#....#...#.....#\n\
#....#.......#.....#\n\
#........#.........#\n\
#..##....#....##...#\n\
#........#.......11#\n\
#........#.......11#\n\
####################\n";

pub fn start(map: &str, seed: u64) -> Match {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(map).unwrap(), seed);
    while m.world.phase != PHASE_ACTION {
        m.step();
    }
    m
}

pub fn at(tx: i32, ty: i32) -> V2 {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

pub fn place(m: &mut Match, u: usize, pos: V2, facing: i32) {
    let unit = &mut m.world.units[u];
    unit.pos = pos;
    unit.vel = (0, 0);
    unit.facing = facing & 0xFFFF;
}

/// Put `weapon` (full magazine) into its slot and into the unit's hands.
pub fn arm(m: &mut Match, u: usize, weapon: i32) {
    let wr = m.world.rules.weapons[weapon as usize];
    let unit = &mut m.world.units[u];
    unit.weapons[wr.slot as usize] = weapon;
    unit.ammo[wr.slot as usize] = wr.magazine;
    unit.slot = wr.slot as usize;
    unit.combat = Default::default();
}

pub fn weapon_id(name: &str) -> i32 {
    Ruleset::builtin()
        .weapon_names
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("no weapon `{name}`")) as i32
}

/// Shooter on the left looking along +x, the other unit parked in the corner. Same layout as
/// `combat_firing::range`.
pub fn range(weapon: i32) -> Match {
    let mut m = start(&arena(34, 15, 1), 1);
    place(&mut m, 0, at(4, 7), 0);
    place(&mut m, 1, at(1, 1), 0);
    arm(&mut m, 0, weapon);
    m
}

/// 2 v 2 open room. Units 0 and 1 attack; 2 and 3 defend. Same as `combat_drops::room`.
pub fn room() -> Match {
    let mut m = start(&arena(24, 11, 2), 1);
    place(&mut m, 0, at(4, 3), 0);
    place(&mut m, 1, at(4, 7), 0);
    place(&mut m, 2, at(18, 3), 32768);
    place(&mut m, 3, at(18, 7), 32768);
    m
}
