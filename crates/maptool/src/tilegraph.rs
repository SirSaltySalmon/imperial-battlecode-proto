//! Which tiles a unit can stand on and which neighbouring tiles it can move between, decided with the
//! engine's circle test rather than assumed from the tile type. With the stock `unit_radius` (under
//! half a tile) this comes out as 4-connectivity of floor tiles: a diagonal step between two blocked
//! tiles is a zero-width pinch no circle passes. A ruleset with fatter units closes 1-tile corridors
//! too, and this graph follows.

use crate::geom::swept_free;
use crate::{tile_centre, Map, TILE_FLOOR};

pub struct TileGraph {
    pub w: i32,
    pub h: i32,
    /// A unit can stand on the tile centre.
    pub stand: Vec<bool>,
    /// A unit can move from the tile to its right / lower neighbour (centre to centre).
    right: Vec<bool>,
    down: Vec<bool>,
}

impl TileGraph {
    pub fn build(map: &Map, r: i32) -> TileGraph {
        let (w, h) = (map.width, map.height);
        let n = (w * h) as usize;
        let mut g = TileGraph {
            w,
            h,
            stand: vec![false; n],
            right: vec![false; n],
            down: vec![false; n],
        };
        for ty in 0..h {
            for tx in 0..w {
                let i = (ty * w + tx) as usize;
                g.stand[i] =
                    map.tile(tx, ty) == TILE_FLOOR && !map.circle_blocked(tile_centre(tx, ty), r);
            }
        }
        for ty in 0..h {
            for tx in 0..w {
                let i = (ty * w + tx) as usize;
                if !g.stand[i] {
                    continue;
                }
                let c = tile_centre(tx, ty);
                g.right[i] =
                    tx + 1 < w && g.stand[i + 1] && swept_free(map, c, tile_centre(tx + 1, ty), r);
                g.down[i] = ty + 1 < h
                    && g.stand[i + w as usize]
                    && swept_free(map, c, tile_centre(tx, ty + 1), r);
            }
        }
        g
    }

    pub fn index(&self, tx: i32, ty: i32) -> usize {
        (ty * self.w + tx) as usize
    }

    pub fn coords(&self, i: usize) -> (i32, i32) {
        (i as i32 % self.w, i as i32 / self.w)
    }

    /// Tiles a unit can move to directly from tile `i`.
    pub fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let w = self.w as usize;
        let (tx, ty) = self.coords(i);
        let mut out = [usize::MAX; 4];
        if self.right[i] {
            out[0] = i + 1;
        }
        if self.down[i] {
            out[1] = i + w;
        }
        if tx > 0 && self.right[i - 1] {
            out[2] = i - 1;
        }
        if ty > 0 && self.down[i - w] {
            out[3] = i - w;
        }
        out.into_iter().filter(|&j| j != usize::MAX)
    }

    /// Every tile reachable from the start tiles.
    pub fn flood(&self, starts: &[usize]) -> Vec<bool> {
        let mut seen = vec![false; self.stand.len()];
        let mut stack: Vec<usize> = starts.iter().copied().filter(|&i| self.stand[i]).collect();
        for &i in &stack {
            seen[i] = true;
        }
        while let Some(i) = stack.pop() {
            for j in self.neighbours(i) {
                if !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        seen
    }

    /// Step counts (in tiles) from the start tiles; `u32::MAX` where unreachable.
    pub fn steps_from(&self, starts: &[usize]) -> Vec<u32> {
        let mut d = vec![u32::MAX; self.stand.len()];
        let mut queue = std::collections::VecDeque::new();
        for &i in starts {
            if self.stand[i] && d[i] == u32::MAX {
                d[i] = 0;
                queue.push_back(i);
            }
        }
        while let Some(i) = queue.pop_front() {
            for j in self.neighbours(i) {
                if d[j] == u32::MAX {
                    d[j] = d[i] + 1;
                    queue.push_back(j);
                }
            }
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINCH: &str = "name: t\n---\n#####\n#a#1#\n#.#.#\n#d.##\n#.#.#\n#####\n";

    #[test]
    fn diagonal_pinch_is_not_a_connection() {
        let m = Map::parse(PINCH).unwrap();
        let g = TileGraph::build(&m, 300);
        let seen = g.flood(&[g.index(1, 1)]);
        assert!(seen[g.index(2, 3)], "orthogonal neighbours connect");
        assert!(
            !seen[g.index(3, 2)],
            "(2,3)->(3,2) is a zero-width diagonal gap"
        );
        assert!(!seen[g.index(3, 4)]);
    }

    #[test]
    fn fat_units_cannot_use_one_tile_corridors() {
        let m = Map::parse(PINCH).unwrap();
        let g = TileGraph::build(&m, 600);
        assert!(
            !g.stand[g.index(1, 1)],
            "a radius over half a tile does not fit a 1-tile corridor"
        );
    }
}
