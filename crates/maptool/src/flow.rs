//! Chokepoints by minimum vertex cut (Menger / max-flow min-cut) on the tile graph.
//!
//! Every walkable tile is split into an in-node and an out-node joined by a capacity-1 edge, moves
//! between tiles have unlimited capacity, and the maximum flow from the source tiles to the sink
//! tiles equals the smallest number of tiles that must be blocked to separate them. The tiles of that
//! cut, grouped into touching clusters, are the distinct chokepoints: their count is the number of
//! separate routes and each cluster's size is that choke's width in tiles.

use crate::tilegraph::TileGraph;
use std::collections::VecDeque;

const INF: u32 = u32::MAX / 2;
/// Flows at or above this are reported as "open" (no meaningful choke).
pub const OPEN_WIDTH: u32 = 48;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choke {
    /// Tiles of this choke, row-major.
    pub tiles: Vec<(i32, i32)>,
}

impl Choke {
    pub fn width(&self) -> u32 {
        self.tiles.len() as u32
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cut {
    /// Total tiles in the minimum cut.
    pub width: u32,
    pub chokes: Vec<Choke>,
}

struct Net {
    head: Vec<u32>,
    next: Vec<u32>,
    to: Vec<u32>,
    cap: Vec<u32>,
}

impl Net {
    fn new(nodes: usize) -> Net {
        Net { head: vec![u32::MAX; nodes], next: Vec::new(), to: Vec::new(), cap: Vec::new() }
    }

    fn edge(&mut self, a: usize, b: usize, cap: u32) {
        for (from, to, c) in [(a, b, cap), (b, a, 0)] {
            self.to.push(to as u32);
            self.cap.push(c);
            self.next.push(self.head[from]);
            self.head[from] = self.to.len() as u32 - 1;
        }
    }

    /// One shortest augmenting path of capacity 1 (Edmonds-Karp step). False when none is left.
    fn augment(&mut self, s: usize, t: usize) -> bool {
        let mut via = vec![u32::MAX; self.head.len()];
        let mut seen = vec![false; self.head.len()];
        let mut queue = VecDeque::from([s]);
        seen[s] = true;
        while let Some(u) = queue.pop_front() {
            if u == t {
                break;
            }
            let mut e = self.head[u];
            while e != u32::MAX {
                let v = self.to[e as usize] as usize;
                if self.cap[e as usize] > 0 && !seen[v] {
                    seen[v] = true;
                    via[v] = e;
                    queue.push_back(v);
                }
                e = self.next[e as usize];
            }
        }
        if !seen[t] {
            return false;
        }
        let mut v = t;
        while v != s {
            let e = via[v] as usize;
            self.cap[e] -= 1;
            self.cap[e ^ 1] += 1;
            v = self.to[e ^ 1] as usize;
        }
        true
    }

    fn reachable(&self, s: usize) -> Vec<bool> {
        let mut seen = vec![false; self.head.len()];
        let mut stack = vec![s];
        seen[s] = true;
        while let Some(u) = stack.pop() {
            let mut e = self.head[u];
            while e != u32::MAX {
                let v = self.to[e as usize] as usize;
                if self.cap[e as usize] > 0 && !seen[v] {
                    seen[v] = true;
                    stack.push(v);
                }
                e = self.next[e as usize];
            }
        }
        seen
    }
}

/// Minimum tile cut between the `source` tiles and the `sink` tiles (neither may be cut). Returns
/// `None` when they touch or overlap, or when the cut is at least `OPEN_WIDTH` tiles: there is no
/// choke to speak of.
pub fn min_cut(g: &TileGraph, source: &[bool], sink: &[bool]) -> Option<Cut> {
    let n = g.stand.len();
    let (s, t) = (2 * n, 2 * n + 1);
    let mut net = Net::new(2 * n + 2);
    for i in 0..n {
        if !g.stand[i] {
            continue;
        }
        let fixed = source[i] || sink[i];
        net.edge(2 * i, 2 * i + 1, if fixed { INF } else { 1 });
        if source[i] {
            net.edge(s, 2 * i, INF);
        }
        if sink[i] {
            net.edge(2 * i + 1, t, INF);
        }
        for j in g.neighbours(i) {
            net.edge(2 * i + 1, 2 * j, INF);
        }
    }
    let mut flow = 0;
    while net.augment(s, t) {
        flow += 1;
        if flow >= OPEN_WIDTH {
            return None;
        }
    }
    // Cut tiles: entered from the source side but not left.
    let seen = net.reachable(s);
    let cut: Vec<usize> = (0..n).filter(|&i| g.stand[i] && seen[2 * i] && !seen[2 * i + 1]).collect();
    debug_assert_eq!(cut.len() as u32, flow);
    Some(Cut { width: flow, chokes: cluster(g, &cut) })
}

/// Group cut tiles that touch (including diagonally) into chokes.
fn cluster(g: &TileGraph, cut: &[usize]) -> Vec<Choke> {
    let mut left: Vec<(i32, i32)> = cut.iter().map(|&i| g.coords(i)).collect();
    let mut chokes = Vec::new();
    while let Some(first) = left.pop() {
        let mut tiles = vec![first];
        let mut k = 0;
        while k < tiles.len() {
            let (x, y) = tiles[k];
            let (near, far): (Vec<_>, Vec<_>) =
                left.into_iter().partition(|&(a, b)| (a - x).abs() <= 1 && (b - y).abs() <= 1);
            tiles.extend(near);
            left = far;
            k += 1;
        }
        tiles.sort_by_key(|&(x, y)| (y, x));
        chokes.push(Choke { tiles });
    }
    chokes.sort_by_key(|c| (c.tiles[0].1, c.tiles[0].0));
    chokes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Map;

    fn cut_of(text: &str) -> Option<Cut> {
        let m = Map::parse(text).unwrap();
        let g = TileGraph::build(&m, 300);
        let n = g.stand.len();
        let mut sink = vec![false; n];
        // Source: everything within 3 steps of the attacker spawn, so the spawn tile's own
        // neighbourhood is not the cut.
        let starts: Vec<usize> = m.spawns[0].iter().map(|p| g.index(p.0 / 1024, p.1 / 1024)).collect();
        let source: Vec<bool> = g.steps_from(&starts).iter().map(|&d| d <= 3).collect();
        for (i, &s) in m.site_of.iter().enumerate() {
            sink[i] = s == 1;
        }
        min_cut(&g, &source, &sink)
    }

    #[test]
    fn two_doors_of_different_width() {
        let text = "name: t\n---\n#########\n#a..#...#\n#...#.1.#\n#.......#\n#...#...#\n#...#...#\n#.......#\n#...#..d#\n#########\n";
        // The dividing wall at x=4 has a 1-tile door at row 3 and another at row 6.
        let cut = cut_of(text).unwrap();
        assert_eq!(cut.width, 2);
        assert_eq!(cut.chokes.len(), 2);
        // Of the equal cuts along each door's approach, the one nearest the source is reported.
        assert_eq!(cut.chokes[0].tiles, vec![(3, 3)]);
        assert_eq!(cut.chokes[1].tiles, vec![(3, 6)]);
    }

    #[test]
    fn wide_door_is_one_choke() {
        let text = "name: t\n---\n#########\n#a..#...#\n#.....1.#\n#.......#\n#.......#\n#...#..d#\n#########\n";
        let cut = cut_of(text).unwrap();
        assert_eq!((cut.width, cut.chokes.len()), (3, 1));
        assert_eq!(cut.chokes[0].width(), 3);
    }

    #[test]
    fn touching_source_and_sink_have_no_cut() {
        assert_eq!(cut_of("name: t\n---\n#####\n#a1d#\n#####\n"), None);
    }

    #[test]
    fn separated_regions_cut_at_zero() {
        let cut = cut_of("name: t\n---\n#######\n#a.#1d#\n#######\n").unwrap();
        assert_eq!((cut.width, cut.chokes.len()), (0, 0));
    }
}
