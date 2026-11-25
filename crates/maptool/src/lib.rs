//! Map tooling for Imperial Battlecode: lint, analysis, rendering and transforms of `.ibcmap` files.
//!
//! Everything that mirrors engine behaviour (who can stand where, what blocks sight, how far a unit
//! travels) is integer math on the engine's own `Map`, so a number printed here is a number a bot can
//! reproduce. Floats appear only when formatting reports and drawing SVG.

pub mod analyze;
pub mod bands;
pub mod flow;
pub mod geom;
pub mod lint;
pub mod path;
pub mod raw;
pub mod render;
pub mod report;
pub mod sight;
pub mod tilegraph;
pub mod transform;

pub use ibc_engine::abi::{SiteInfo, TILE, TILE_FLOOR, TILE_LOW, TILE_WALL};
pub use ibc_engine::fixed::V2;
pub use ibc_engine::{Map, Ruleset};

/// World position of a tile's centre.
pub fn tile_centre(tx: i32, ty: i32) -> V2 {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

/// Tile containing a world position.
pub fn tile_of(p: V2) -> (i32, i32) {
    (p.0.div_euclid(TILE), p.1.div_euclid(TILE))
}

/// The spawn tiles the engine actually uses for a side (0 = attackers, 1 = defenders): the first
/// `team_size` in row-major order.
pub fn used_spawns(map: &Map, side: usize) -> &[V2] {
    let n = (map.team_size.max(1) as usize).min(map.spawns[side].len());
    &map.spawns[side][..n]
}

/// Ticks needed to cover `dist` world units at `speed` units per tick (rounded up).
pub fn ticks_for(dist: i64, speed: i32) -> i64 {
    let s = speed.max(1) as i64;
    (dist.max(0) + s - 1) / s
}
