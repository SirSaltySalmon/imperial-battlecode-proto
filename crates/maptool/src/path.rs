//! Any-angle shortest paths for a unit-sized circle: a visibility graph over the inflated obstacles.
//!
//! The shortest path of a disc among polygons runs along straight segments that are tangent to the
//! obstacles grown by the disc's radius; around a convex corner the grown obstacle is a quarter
//! circle. We replace each quarter circle by a three-point polyline that circumscribes it (so the
//! chords never come closer than the radius), connect every pair of points the disc can travel
//! between in a straight line (`geom::swept_free`, the engine's own blocking test), and run Dijkstra.
//! The result overestimates the true optimum by a few world units per corner turned and never
//! reports a path the engine would block. Lengths are integer world units.

use crate::geom::swept_free;
use crate::{tile_centre, Map, TILE, TILE_FLOOR, V2};
use ibc_engine::fixed;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub const UNREACHABLE: i64 = i64::MAX / 4;
const NONE: u32 = u32::MAX;

/// Offsets of the three polyline points for a corner arc of radius `r`: `(axis, diagonal)` such
/// that the chord from `(axis, 0)` to `(diagonal, diagonal)` stays at least `r` from the corner.
pub fn corner_offsets(r: i32) -> (i32, i32) {
    // 1 / cos(22.5 deg) = 1.0824; the +2 and the rounding up of the diagonal absorb integer error.
    let axis = (r as i64 * 10824 + 9999) / 10000 + 2;
    let diag = (axis * 70711 + 99999) / 100000;
    (axis as i32, diag as i32)
}

pub struct VisGraph<'m> {
    pub map: &'m Map,
    pub r: i32,
    pub nodes: Vec<V2>,
    adj: Vec<Vec<(u32, i64)>>,
    /// Per tile: the nodes its centre can travel to in a straight line (empty for blocked tiles).
    tile_vis: Vec<Vec<(u32, i64)>>,
}

/// Shortest distances from a set of source points to every graph node.
pub struct Field {
    pub sources: Vec<V2>,
    dist: Vec<i64>,
    /// Previous node on the way back to a source, or `NONE` when the node sees its source directly.
    prev: Vec<u32>,
    /// Index of the source a node's path starts from.
    origin: Vec<u32>,
}

fn len(a: V2, b: V2) -> i64 {
    fixed::dist(a, b) as i64
}

impl<'m> VisGraph<'m> {
    pub fn build(map: &'m Map, r: i32) -> VisGraph<'m> {
        let nodes = corner_nodes(map, r);
        let mut adj = vec![Vec::new(); nodes.len()];
        for i in 0..nodes.len() {
            for j in i + 1..nodes.len() {
                if swept_free(map, nodes[i], nodes[j], r) {
                    let d = len(nodes[i], nodes[j]);
                    adj[i].push((j as u32, d));
                    adj[j].push((i as u32, d));
                }
            }
        }
        let mut g = VisGraph { map, r, nodes, adj, tile_vis: Vec::new() };
        let mut tile_vis = vec![Vec::new(); (map.width * map.height) as usize];
        for ty in 0..map.height {
            for tx in 0..map.width {
                let c = tile_centre(tx, ty);
                if map.tile(tx, ty) == TILE_FLOOR && !map.circle_blocked(c, r) {
                    tile_vis[(ty * map.width + tx) as usize] = g.visible_nodes(c);
                }
            }
        }
        g.tile_vis = tile_vis;
        g
    }

    /// Graph nodes the circle can reach from `p` in a straight line, with distances.
    pub fn visible_nodes(&self, p: V2) -> Vec<(u32, i64)> {
        (0..self.nodes.len())
            .filter(|&i| swept_free(self.map, p, self.nodes[i], self.r))
            .map(|i| (i as u32, len(p, self.nodes[i])))
            .collect()
    }

    /// Dijkstra from several sources at once (a unit may start at any of them).
    pub fn field(&self, sources: &[V2]) -> Field {
        let n = self.nodes.len();
        let mut f =
            Field { sources: sources.to_vec(), dist: vec![UNREACHABLE; n], prev: vec![NONE; n], origin: vec![NONE; n] };
        let mut heap = BinaryHeap::new();
        for (si, &s) in sources.iter().enumerate() {
            if self.map.circle_blocked(s, self.r) {
                continue;
            }
            for (i, d) in self.visible_nodes(s) {
                if d < f.dist[i as usize] {
                    f.dist[i as usize] = d;
                    f.origin[i as usize] = si as u32;
                    heap.push(Reverse((d, i)));
                }
            }
        }
        while let Some(Reverse((d, i))) = heap.pop() {
            if d > f.dist[i as usize] {
                continue;
            }
            for &(j, w) in &self.adj[i as usize] {
                let nd = d + w;
                if nd < f.dist[j as usize] {
                    f.dist[j as usize] = nd;
                    f.prev[j as usize] = i;
                    f.origin[j as usize] = f.origin[i as usize];
                    heap.push(Reverse((nd, j)));
                }
            }
        }
        f
    }

    /// Best way to reach `p`: (distance, last node or `NONE` for a straight run, source index).
    fn best(&self, f: &Field, p: V2, vis: &[(u32, i64)]) -> Option<(i64, u32, u32)> {
        if self.map.circle_blocked(p, self.r) {
            return None;
        }
        let mut best: Option<(i64, u32, u32)> = None;
        for (si, &s) in f.sources.iter().enumerate() {
            let d = len(s, p);
            if best.is_none_or(|b| d < b.0) && !self.map.circle_blocked(s, self.r) && swept_free(self.map, s, p, self.r)
            {
                best = Some((d, NONE, si as u32));
            }
        }
        for &(i, d) in vis {
            let total = f.dist[i as usize] + d;
            if f.dist[i as usize] < UNREACHABLE && best.is_none_or(|b| total < b.0) {
                best = Some((total, i, f.origin[i as usize]));
            }
        }
        best
    }

    /// Shortest travel distance from the field's sources to `p`, if it can be reached.
    pub fn dist_to(&self, f: &Field, p: V2) -> Option<i64> {
        self.best(f, p, &self.visible_nodes(p)).map(|b| b.0)
    }

    /// The shortest path itself, source first, `p` last.
    pub fn path_to(&self, f: &Field, p: V2) -> Option<Vec<V2>> {
        let (_, mut node, source) = self.best(f, p, &self.visible_nodes(p))?;
        let mut pts = vec![p];
        while node != NONE {
            pts.push(self.nodes[node as usize]);
            node = f.prev[node as usize];
        }
        pts.push(f.sources[source as usize]);
        pts.reverse();
        Some(pts)
    }

    /// Distance from the field's sources to every tile centre (`UNREACHABLE` for blocked tiles).
    pub fn tile_dists(&self, f: &Field) -> Vec<i64> {
        let mut out = vec![UNREACHABLE; self.tile_vis.len()];
        for ty in 0..self.map.height {
            for tx in 0..self.map.width {
                let i = (ty * self.map.width + tx) as usize;
                if let Some(b) = self.best(f, tile_centre(tx, ty), &self.tile_vis[i]) {
                    out[i] = b.0;
                }
            }
        }
        out
    }
}

/// Polyline points around every convex corner of the movement-blocking tiles.
fn corner_nodes(map: &Map, r: i32) -> Vec<V2> {
    let (axis, diag) = corner_offsets(r);
    let blocked = |tx: i32, ty: i32| map.tile(tx, ty) != TILE_FLOOR;
    let mut nodes = Vec::new();
    for vy in 0..=map.height {
        for vx in 0..=map.width {
            // The four tiles around grid vertex (vx, vy), as (dx, dy) quadrant signs.
            let quads = [(-1, -1), (1, -1), (1, 1), (-1, 1)];
            let is_blocked = |q: (i32, i32)| blocked(vx + (q.0 - 1) / 2, vy + (q.1 - 1) / 2);
            let count = quads.iter().filter(|&&q| is_blocked(q)).count();
            if count != 1 {
                continue; // flat wall, inside corner, open floor, or a diagonal pinch nothing passes
            }
            let q = quads.into_iter().find(|&q| is_blocked(q)).expect("one blocked quadrant");
            // The path bends in the quadrant opposite the blocked tile.
            let (sx, sy) = (-q.0, -q.1);
            let (cx, cy) = (vx * TILE, vy * TILE);
            for p in [(cx + sx * axis, cy), (cx + sx * diag, cy + sy * diag), (cx, cy + sy * axis)] {
                if !map.circle_blocked(p, r) {
                    nodes.push(p);
                }
            }
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::point_seg_within;

    #[test]
    fn corner_polyline_clears_the_corner_for_any_radius() {
        for r in [1, 50, 128, 300, 301, 400, 511, 700] {
            let (a, d) = corner_offsets(r);
            assert!(!point_seg_within((0, 0), (a, 0), (d, d), r as i64), "r={r}");
            assert!(!point_seg_within((0, 0), (d, d), (0, a), r as i64), "r={r}");
            assert!(a <= r + r / 10 + 4, "r={r}: polyline hugs the arc");
        }
    }

    fn graph_dist(text: &str, from: (i32, i32), to: (i32, i32)) -> i64 {
        let m = Map::parse(text).unwrap();
        let g = VisGraph::build(&m, 300);
        let f = g.field(&[tile_centre(from.0, from.1)]);
        g.dist_to(&f, tile_centre(to.0, to.1)).expect("reachable")
    }

    #[test]
    fn open_floor_is_a_straight_line() {
        let text = "name: t\n---\n########\n#a.....#\n#......#\n#......#\n#....1d#\n########\n";
        // (1,1) -> (6,4): sqrt(5^2 + 3^2) tiles = 5.83 tiles, not the 8-connected 3*sqrt2 + 2 = 6.24.
        let d = graph_dist(text, (1, 1), (6, 4));
        assert_eq!(d, fixed::isqrt(((5 * 1024i64).pow(2) + (3 * 1024i64).pow(2)) as u64) as i64);
    }

    #[test]
    fn path_wraps_a_wall_end_at_unit_radius() {
        // A wall from the top border down to row 3; start and goal on row 1 either side of it.
        let text = "name: t\n---\n#########\n#a..#..1#\n#...#...#\n#...#...#\n#.......#\n#d......#\n#########\n";
        let d = graph_dist(text, (2, 1), (6, 1));
        // Hand computation: the wall end is the segment y = 4096, x in [4096, 5120]. The disc centre
        // passes below it at distance 300: a tangent from (2560, 1536) to the circle of radius 300
        // about corner (4096, 4096), an arc until the heading is along +x, the flat run of 1024
        // under the wall end, and the mirror image on the far side.
        let corner_dist2 = (1536f64).powi(2) + (2560f64).powi(2);
        let tangent = (corner_dist2 - 300f64.powi(2)).sqrt();
        // The tangent heads steeper than the start->corner line by asin(r / distance); the arc turns
        // that heading back to horizontal.
        let to_corner = (2560f64).atan2(1536f64);
        let wrap = to_corner + (300f64 / corner_dist2.sqrt()).asin();
        let exact = 2.0 * (tangent + 300.0 * wrap) + 1024.0;
        assert!(d as f64 >= exact - 4.0, "never shorter than the true optimum: {d} vs {exact:.1}");
        assert!((d as f64) < exact * 1.01, "within 1% of the true optimum: {d} vs {exact:.1}");
        // The 8-connected grid answer would be 3 down, 4 across, 3 up = 10 tiles.
        assert!(d < 8 * 1024);
    }

    #[test]
    fn unreachable_and_blocked_targets() {
        let m = Map::parse("name: t\n---\n#######\n#a.#.d#\n#..#.1#\n#######\n").unwrap();
        let g = VisGraph::build(&m, 300);
        let f = g.field(&[tile_centre(1, 1)]);
        assert!(g.dist_to(&f, tile_centre(5, 1)).is_none(), "other side of a solid wall");
        assert!(g.dist_to(&f, tile_centre(3, 1)).is_none(), "inside a wall");
        assert_eq!(g.dist_to(&f, tile_centre(2, 2)), Some(1448));
    }

    #[test]
    fn path_points_are_mutually_passable() {
        let text = "name: t\n---\n##########\n#a..#....#\n#...#.o..#\n#...#...1#\n#........#\n#d..o....#\n##########\n";
        let m = Map::parse(text).unwrap();
        let g = VisGraph::build(&m, 300);
        let f = g.field(&[tile_centre(1, 1)]);
        let path = g.path_to(&f, tile_centre(8, 1)).unwrap();
        assert!(path.len() >= 3);
        let mut total = 0;
        for w in path.windows(2) {
            assert!(swept_free(&m, w[0], w[1], 300));
            total += len(w[0], w[1]);
        }
        assert_eq!(Some(total), g.dist_to(&f, tile_centre(8, 1)));
        let tiles = g.tile_dists(&f);
        assert_eq!(tiles[(m.width + 8) as usize], total);
    }
}
