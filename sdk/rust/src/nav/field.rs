//! The distance field behind `Pathfinder`: Dijkstra from the goal outwards, settled on demand.
use super::{Grid, DIAG, NEIGHBOURS, ORTH, ORTH_BITS};
use crate::TILE_FLOOR;

pub(super) const UNREACHABLE: u32 = u32::MAX;

/// `f::<m>(args)` for a nibble `m` known at run time: one jump into 16 straight-line bodies instead of a
/// test per direction.
macro_rules! by_nibble {
    ($f:ident, $m:expr, $args:tt) => {
        by_nibble!(@ $f, $m, $args, 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)
    };
    (@ $f:ident, $m:expr, $args:tt, $($n:literal)*) => {
        match $m {
            $($n => $f::<$n> $args,)*
            _ => {}
        }
    };
}

/// Shortest-path costs from every tile to one goal tile, grown outwards from the goal on demand
/// (Dijkstra with a bucket queue, "Dial's algorithm": steps cost 5 or 7, so 8 buckets are enough).
/// A queued entry is `tile << 4 | code`, where `code` is the direction the tile was reached in, or
/// `FROM_GOAL` (see `KEEP`).
pub(super) struct Field {
    pub(super) goal: usize,
    pub(super) cost: Vec<u32>,
    buckets: [Vec<u32>; 8],
    /// Cost level being drained; every tile with `cost <= level` is final.
    level: u32,
    /// The goal itself has been settled (it is the first tile, and it counts against the budget).
    started: bool,
    /// The queue ran dry: every tile the goal can be reached from is final.
    done: bool,
}

/// The `code` of a tile reached from the goal: nothing is pruned.
const FROM_GOAL: usize = 8;

impl Field {
    pub(super) fn new(grid: &Grid, goal: usize) -> Field {
        let mut field = Field::vacant();
        field.reset(grid, goal);
        field
    }

    /// An empty cache slot (`Pathfinder::complete` took its field); no goal matches it.
    pub(super) fn vacant() -> Field {
        Field { goal: usize::MAX, cost: Vec::new(), buckets: Default::default(), level: 0, started: true, done: true }
    }

    /// Start again for `goal`, keeping the allocations.
    pub(super) fn reset(&mut self, grid: &Grid, goal: usize) {
        self.goal = goal;
        self.cost.clear();
        self.cost.resize(grid.tiles.len(), UNREACHABLE);
        self.cost[goal] = 0;
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        (self.level, self.started, self.done) = (0, false, false);
    }

    /// Is the cost of `tile` final? (No tile matches `usize::MAX`.)
    #[inline]
    fn is_final(&self, tile: usize) -> bool {
        self.done || self.cost.get(tile).is_some_and(|&c| c <= self.level)
    }

    /// Settle the goal: queue its neighbours. Stepping is symmetric between floor tiles, so the
    /// goal-outwards search uses the forward step masks; a blocked goal only seeds its orthogonal
    /// neighbours ("as close as possible").
    fn start(&mut self, grid: &Grid) {
        let g = self.goal;
        let seed = grid.steps[g] & if grid.tiles[g] == TILE_FLOOR { !0 } else { ORTH_BITS };
        for (d, &(dx, dy, step)) in NEIGHBOURS.iter().enumerate() {
            if seed & (1 << d) != 0 {
                let n = (g as i32 + dy * grid.width + dx) as usize;
                self.cost[n] = step;
                self.buckets[step as usize & 7].push((n << 4 | FROM_GOAL) as u32);
            }
        }
        self.started = true;
    }

    /// Grow the field until the cost of `tile` is final, settling at most `budget` tiles.
    /// Returns false if the budget ran out first.
    pub(super) fn settle(&mut self, grid: &Grid, tile: usize, mut budget: u32) -> bool {
        if self.is_final(tile) {
            return true;
        }
        if !self.started {
            if budget == 0 {
                return false;
            }
            budget -= 1;
            self.start(grid);
        }
        let Field { cost, buckets: [b0, b1, b2, b3, b4, b5, b6, b7], level, done, .. } = self;
        let cost = &mut cost[..];
        // Levels in a row whose bucket was empty: 8 means every bucket is (a step costs at most 7).
        let mut idle = 0;
        loop {
            // This level's bucket and the two that its steps (+5, +7) fill.
            let (queue, q5, q7) = match *level & 7 {
                0 => (&mut *b0, &mut *b5, &mut *b7),
                1 => (&mut *b1, &mut *b6, &mut *b0),
                2 => (&mut *b2, &mut *b7, &mut *b1),
                3 => (&mut *b3, &mut *b0, &mut *b2),
                4 => (&mut *b4, &mut *b1, &mut *b3),
                5 => (&mut *b5, &mut *b2, &mut *b4),
                6 => (&mut *b6, &mut *b3, &mut *b5),
                _ => (&mut *b7, &mut *b4, &mut *b6),
            };
            idle = if queue.is_empty() { idle + 1 } else { 0 };
            if idle == 8 {
                *done = true;
                return true;
            }
            match drain(queue, q5, q7, cost, grid, *level, budget) {
                Some(left) => budget = left,
                None => return false,
            }
            *level += 1;
            if cost.get(tile).is_some_and(|&c| c <= *level) {
                return true;
            }
        }
    }
}

/// Settle the tiles queued at `level` (the last queued first), whose steps fill `q5` and `q7`. Returns the
/// budget left, or `None` if it ran out first (the tile that did not fit stays queued).
#[inline(always)]
fn drain(
    queue: &mut Vec<u32>,
    q5: &mut Vec<u32>,
    q7: &mut Vec<u32>,
    cost: &mut [u32],
    grid: &Grid,
    level: u32,
    mut budget: u32,
) -> Option<u32> {
    let (w, steps) = (grid.width as usize, &grid.steps[..]);
    let (c5, c7) = (level + ORTH, level + DIAG);
    while let Some(entry) = queue.pop() {
        let (at, code) = ((entry >> 4) as usize, (entry & 15) as usize);
        if cost[at] != level {
            continue; // stale entry: this tile was reached more cheaply already
        }
        if budget == 0 {
            queue.push(entry);
            return None;
        }
        budget -= 1;
        let mask = KEEP[code][steps[at] as usize];
        by_nibble!(orth, mask & 15, (cost, q5, at, w, c5));
        by_nibble!(diag, mask >> 4, (cost, q7, at, w, c7));
    }
    Some(budget)
}

/// Relax the orthogonal steps in `M` (bits 0-3 of a step mask, `NEIGHBOURS` order), at cost `c`.
#[inline(always)]
fn orth<const M: u8>(cost: &mut [u32], queue: &mut Vec<u32>, at: usize, w: usize, c: u32) {
    if M & 1 != 0 {
        relax(cost, queue, at + 1, 0, c);
    }
    if M & 2 != 0 {
        relax(cost, queue, at + w, 1, c);
    }
    if M & 4 != 0 {
        relax(cost, queue, at - 1, 2, c);
    }
    if M & 8 != 0 {
        relax(cost, queue, at - w, 3, c);
    }
}

/// Relax the diagonal steps in `M` (bits 4-7 of a step mask, shifted down), at cost `c`.
#[inline(always)]
fn diag<const M: u8>(cost: &mut [u32], queue: &mut Vec<u32>, at: usize, w: usize, c: u32) {
    if M & 1 != 0 {
        relax(cost, queue, at + w + 1, 4, c);
    }
    if M & 2 != 0 {
        relax(cost, queue, at + w - 1, 5, c);
    }
    if M & 4 != 0 {
        relax(cost, queue, at - w - 1, 6, c);
    }
    if M & 8 != 0 {
        relax(cost, queue, at - w + 1, 7, c);
    }
}

/// Lower the cost of tile `n`, reached in direction `code`, to `c` and queue it, if that is cheaper.
#[inline(always)]
fn relax(cost: &mut [u32], queue: &mut Vec<u32>, n: usize, code: usize, c: u32) {
    if c < cost[n] {
        cost[n] = c;
        queue.push((n << 4 | code) as u32);
    }
}

/// `KEEP[code][mask]`: the steps in `mask` still worth trying from a tile reached in direction `code`.
/// A step is dropped when its target is the parent (the tile it was reached from), or a tile the parent can
/// step to itself: the parent was settled first and is at least 5 cheaper, and a step costs at most 7, so
/// the way through this tile can never be cheaper (the neighbour pruning of Jump Point Search, applied to
/// Dijkstra). The search settles the same tiles at the same costs; it only skips relaxations that cannot
/// succeed. Codes 8 and up (`FROM_GOAL`) keep everything.
static KEEP: [[u8; 256]; 16] = keep_table();

const fn keep_table() -> [[u8; 256]; 16] {
    let mut table = [[0u8; 256]; 16];
    let mut code = 0;
    while code < 16 {
        let mut mask = 0;
        while mask < 256 {
            let m = mask as u8;
            table[code][mask] = if code < 8 { m & !parent_covers(code, m) } else { m };
            mask += 1;
        }
        code += 1;
    }
    table
}

/// The steps in `mask` (of a floor tile reached in direction `code` from a floor parent) whose target is
/// the parent or a tile the parent can step to. Only what `mask` proves open is taken as open.
const fn parent_covers(code: usize, mask: u8) -> u8 {
    let (px, py) = (-NEIGHBOURS[code].0, -NEIGHBOURS[code].1);
    let mut covered = 0;
    let mut e = 0;
    while e < 8 {
        let (nx, ny, _) = NEIGHBOURS[e];
        let (rx, ry) = (nx - px, ny - py); // the target, seen from the parent
        let parent_steps = rx.abs() <= 1
            && ry.abs() <= 1
            && known_floor(code, mask, nx, ny)
            && (rx == 0 || ry == 0 || (known_floor(code, mask, px + rx, py) && known_floor(code, mask, px, py + ry)));
        if mask & (1 << e) != 0 && ((nx == px && ny == py) || parent_steps) {
            covered |= 1 << e;
        }
        e += 1;
    }
    covered
}

/// Is the tile at (x, y) from this one known to be floor: this tile, its parent, or a tile that a step
/// in `mask` needs open?
const fn known_floor(code: usize, mask: u8, x: i32, y: i32) -> bool {
    if (x == 0 && y == 0) || (x == -NEIGHBOURS[code].0 && y == -NEIGHBOURS[code].1) {
        return true;
    }
    let mut e = 0;
    while e < 8 {
        let (dx, dy, _) = NEIGHBOURS[e];
        let needs = (x == dx && y == dy) || (dx != 0 && dy != 0 && ((x == dx && y == 0) || (x == 0 && y == dy)));
        if mask & (1 << e) != 0 && needs {
            return true;
        }
        e += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::tests::random_grid;
    use std::cmp::Reverse;

    /// Textbook Dijkstra over `Grid::can_step` from `goal`, with the same rule for a blocked goal.
    fn reference(g: &Grid, goal: usize) -> Vec<u32> {
        let w = g.width;
        let mut cost = vec![UNREACHABLE; g.tiles.len()];
        let mut heap = std::collections::BinaryHeap::from([Reverse((0, goal))]);
        cost[goal] = 0;
        while let Some(Reverse((c, at))) = heap.pop() {
            if c > cost[at] {
                continue;
            }
            let (x, y) = (at as i32 % w, at as i32 / w);
            for &(dx, dy, step) in &NEIGHBOURS {
                let corner_of_blocked_goal = at == goal && g.tiles[goal] != TILE_FLOOR && dx != 0 && dy != 0;
                if !corner_of_blocked_goal && g.can_step(x, y, dx, dy) {
                    let n = ((y + dy) * w + x + dx) as usize;
                    if c + step < cost[n] {
                        cost[n] = c + step;
                        heap.push(Reverse((c + step, n)));
                    }
                }
            }
        }
        cost
    }

    #[test]
    fn pruned_search_matches_textbook_dijkstra() {
        for (w, h, seed) in [(23, 17, 1), (40, 30, 2), (1, 9, 3), (9, 1, 4), (12, 12, 5)] {
            let g = random_grid(w, h, seed);
            for goal in 0..g.tiles.len() {
                let mut field = Field::new(&g, goal);
                assert!(field.settle(&g, usize::MAX, u32::MAX));
                assert_eq!(field.cost, reference(&g, goal), "{w}x{h} goal {goal}");
            }
        }
    }

    #[test]
    fn budget_counts_settled_tiles() {
        // A call returns true once every tile cheaper than the target is settled; the goal is the first.
        let g = random_grid(30, 20, 6);
        for goal in (0..g.tiles.len()).step_by(7) {
            let want = reference(&g, goal);
            for target in (0..g.tiles.len()).step_by(5) {
                let cheaper = want.iter().filter(|&&c| c < want[target]).count() as u32;
                let mut field = Field::new(&g, goal);
                let calls = (1..).find(|_| field.settle(&g, target, 7)).unwrap();
                assert_eq!(calls, cheaper.div_ceil(7).max(1), "goal {goal} target {target}");
            }
        }
    }
}
