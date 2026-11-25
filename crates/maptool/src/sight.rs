//! Line-of-sight measurements. Every test is the engine's `Map::segment_wall` between tile centres,
//! so "visible" here is exactly what the vision and bullet code decides for units standing there.

use crate::{tile_centre, used_spawns, Map, V2};
use ibc_engine::fixed;

/// Clear line between two points (walls block; low cover does not).
pub fn los(map: &Map, a: V2, b: V2) -> bool {
    map.segment_wall(a, b).is_none()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sightline {
    pub a: (i32, i32),
    pub b: (i32, i32),
    /// Length in world units.
    pub len: i64,
}

/// The `count` longest sightlines between standable tile centres, skipping near-duplicates (both
/// ends within `spread` tiles of an already chosen line's ends).
pub fn longest_sightlines(map: &Map, stand: &[bool], count: usize, spread: i32) -> Vec<Sightline> {
    let tiles: Vec<(i32, i32)> =
        (0..stand.len()).filter(|&i| stand[i]).map(|i| (i as i32 % map.width, i as i32 / map.width)).collect();
    let mut all = Vec::new();
    for (k, &a) in tiles.iter().enumerate() {
        for &b in &tiles[k + 1..] {
            let (pa, pb) = (tile_centre(a.0, a.1), tile_centre(b.0, b.1));
            if los(map, pa, pb) {
                all.push(Sightline { a, b, len: fixed::dist(pa, pb) as i64 });
            }
        }
    }
    all.sort_by_key(|s| (std::cmp::Reverse(s.len), s.a.1, s.a.0, s.b.1, s.b.0));
    let near = |p: (i32, i32), q: (i32, i32)| (p.0 - q.0).abs() <= spread && (p.1 - q.1).abs() <= spread;
    let mut picked: Vec<Sightline> = Vec::new();
    for s in all {
        if picked.len() == count {
            break;
        }
        if !picked.iter().any(|p| (near(s.a, p.a) && near(s.b, p.b)) || (near(s.a, p.b) && near(s.b, p.a))) {
            picked.push(s);
        }
    }
    picked
}

/// A pair of (attacker spawn tile, defender spawn tile) that can see each other at round start.
pub fn spawn_sightline(map: &Map) -> Option<(V2, V2)> {
    for &a in used_spawns(map, 0) {
        for &d in used_spawns(map, 1) {
            if los(map, a, d) || los(map, d, a) {
                return Some((a, d));
            }
        }
    }
    None
}

/// Indices of the standable tiles whose centre has a clear line to `p` within `range`.
pub fn tiles_seeing(map: &Map, stand: &[bool], p: V2, range: i32) -> Vec<usize> {
    (0..stand.len())
        .filter(|&i| {
            let c = tile_centre(i as i32 % map.width, i as i32 / map.width);
            stand[i] && fixed::dist(c, p) <= range && los(map, c, p)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tilegraph::TileGraph;

    const M: &str = "name: t\n---\n##########\n#a.....#d#\n#.##.#.#.#\n#....o...#\n#.#....1.#\n##########\n";

    #[test]
    fn los_matches_segment_wall_for_every_tile_pair() {
        let m = Map::parse(M).unwrap();
        let g = TileGraph::build(&m, 300);
        let mut visible = 0;
        for i in (0..g.stand.len()).filter(|&i| g.stand[i]) {
            let seen = tiles_seeing(&m, &g.stand, tile_centre(g.coords(i).0, g.coords(i).1), i32::MAX);
            for j in (0..g.stand.len()).filter(|&j| g.stand[j]) {
                let (a, b) = (g.coords(j), g.coords(i));
                let clear = m.segment_wall(tile_centre(a.0, a.1), tile_centre(b.0, b.1)).is_none();
                assert_eq!(seen.contains(&j), clear, "{a:?} -> {b:?}");
                visible += clear as u32;
            }
        }
        assert!(visible > 0);
    }

    #[test]
    fn low_cover_does_not_block_sight_and_walls_do() {
        let m = Map::parse(M).unwrap();
        assert!(los(&m, tile_centre(1, 3), tile_centre(8, 3)), "row 3 has only low cover in the way");
        assert!(!los(&m, tile_centre(1, 1), tile_centre(8, 1)), "wall at x=7 on row 1");
        assert!(spawn_sightline(&m).is_none());
    }

    #[test]
    fn longest_line_is_found_and_duplicates_are_skipped() {
        let m = Map::parse(M).unwrap();
        let g = TileGraph::build(&m, 300);
        let lines = longest_sightlines(&m, &g.stand, 3, 1);
        assert_eq!((lines[0].a, lines[0].b, lines[0].len), ((1, 3), (8, 4), 7240));
        for (k, s) in lines.iter().enumerate() {
            assert!(los(&m, tile_centre(s.a.0, s.a.1), tile_centre(s.b.0, s.b.1)));
            assert!(k == 0 || s.len <= lines[k - 1].len);
        }
    }

    #[test]
    fn facing_spawns_are_reported() {
        let m = Map::parse("name: t\n---\n######\n#a.1d#\n######\n").unwrap();
        assert!(spawn_sightline(&m).is_some());
    }
}
