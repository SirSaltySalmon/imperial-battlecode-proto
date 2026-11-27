//! Wall grid, cached distance-field pathfinder and a stuck-proof `Navigator`.
//!
//! **This is expensive, and it is meant as a starting point.** These helpers answer the general question
//! (exact 8-connected shortest paths, tile by tile, over the whole map) and a path search is the largest
//! fuel cost most bots have. If you want the fuel for something else, copy this module and cut it down, or
//! write your own: a coarser grid, a graph of a few waypoints between the spawns and the sites, or routes
//! worked out once and kept for the match.
//!
//! Fuel measured with the wasm runner on the map pool (the probe is `sdk/rust/examples/nav_fuel.rs` in the
//! platform repository). With the current rules a unit gets 100,000 per tick (`fuel_per_tick`), four times
//! that on a buy tick and ten times on its first tick (the draft, where `Bot::new` runs):
//!
//! | Call | Fuel |
//! |---|---|
//! | `Navigator::new`, `Grid::load` | about 70 per map tile |
//! | A whole field: `complete`, or `path_len` to a new goal from afar | about 200 per floor tile |
//! | `next_waypoint`, `go` towards a new goal | up to 45,000 a call (160 tiles) until the field reaches you |
//! | `path_len`, `next_waypoint`, `go` once the field reaches you | about 210, 500 and 850 |
//! | `tactics::approach_point` | a field search towards the origin, then 3,000 to 5,000 |
//! | `tactics::hold_spot`, radius 3 | 11,000 to 38,000 (two `los` and a `raycast` per tile) |
//!
//! | Map | Tiles (floor) | `Navigator::new` | Whole field | First `go` to a new goal |
//! |---|---:|---:|---:|---:|
//! | `tunnels` | 312 (208) | 22,500 | 45,800 | 35,700 |
//! | `crossfire` | 544 (396) | 37,500 | 80,900 | 39,100 |
//! | `delta` | 1,221 (850) | 82,500 | 152,800 | 41,900 |
//! | `steppe` | 1,496 (856) | 99,300 | 158,500 | 42,300 |
//!
//! When to call what:
//!
//! - **First tick (draft):** `Navigator::new`, then `complete` the goals you will use all match (site
//!   centres, spawns). They are kept and never evicted, so every later question about them is a lookup.
//!   On `steppe`, the largest map in the pool, the grid and five whole fields take about 890,000.
//! - **Buy ticks:** `path_len`, `approach_point` and `hold_spot` for anything new, or `complete` with a
//!   budget of tiles, a slice per tick.
//! - **Action ticks:** `go` and `next_waypoint` towards goals whose field already reaches you. A new goal
//!   costs up to 45,000 on its first call and `go` walks straight at it until the field arrives.
//!
//! A `Pathfinder` keeps 6 fields, replacing the oldest when a seventh goal comes, plus every field it was
//! asked to `complete`; each takes 4 bytes per map tile. Aim at a few stable goals (a site centre), not a
//! new tile every tick.
use crate::geom::{angle_to, dist};
use crate::{sys, Error, UnitInfo, TILE, TILE_FLOOR, TILE_WALL, T_MAP_ROW};

mod field;
use field::{Field, UNREACHABLE};

/// Tile containing a world position.
#[inline]
pub fn tile_of(x: i32, y: i32) -> (i32, i32) {
    (x.div_euclid(TILE), y.div_euclid(TILE))
}

/// World position of a tile's centre.
#[inline]
pub fn tile_centre(tx: i32, ty: i32) -> (i32, i32) {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

const ORTH: u32 = 5;
const DIAG: u32 = 7;
/// The 8 neighbours: (dx, dy, step cost). Orthogonal first.
const NEIGHBOURS: [(i32, i32, u32); 8] = [
    (1, 0, ORTH),
    (0, 1, ORTH),
    (-1, 0, ORTH),
    (0, -1, ORTH),
    (1, 1, DIAG),
    (-1, 1, DIAG),
    (-1, -1, DIAG),
    (1, -1, DIAG),
];
const ORTH_BITS: u8 = 0b0000_1111;

/// The whole tile grid, fetched once (`height` host calls). Walls never change during a match.
#[derive(Clone, Debug, Default)]
pub struct Grid {
    pub width: i32,
    pub height: i32,
    tiles: Vec<u8>,
    /// Per tile: bit `d` is set if a unit may step to `NEIGHBOURS[d]`.
    steps: Vec<u8>,
}

impl Grid {
    /// Load every map row from the host: about 70 fuel per map tile (99,000 on `steppe`). Once per match.
    pub fn load() -> Grid {
        let info = crate::map_info();
        let (w, h) = (info.width.max(0), info.height.max(0));
        let mut tiles = vec![TILE_WALL; (w * h) as usize];
        for y in 0..h {
            let row = &mut tiles[(y * w) as usize..((y + 1) * w) as usize];
            sys::sys_get(T_MAP_ROW, y as u32, row);
        }
        Grid::from_tiles(w, h, tiles)
    }

    /// Build from raw `TILE_*` values, row by row.
    pub fn from_tiles(width: i32, height: i32, tiles: Vec<u8>) -> Grid {
        assert_eq!(tiles.len(), (width * height) as usize);
        let steps = step_masks(width.max(0) as usize, height.max(0) as usize, &tiles);
        Grid { width, height, tiles, steps }
    }

    /// `TILE_*` value of a tile; anything outside the map is a wall.
    #[inline]
    pub fn tile(&self, tx: i32, ty: i32) -> u8 {
        if tx < 0 || ty < 0 || tx >= self.width || ty >= self.height {
            TILE_WALL
        } else {
            self.tiles[(ty * self.width + tx) as usize]
        }
    }

    /// `TILE_*` value under a world position.
    #[inline]
    pub fn tile_at(&self, x: i32, y: i32) -> u8 {
        let (tx, ty) = tile_of(x, y);
        self.tile(tx, ty)
    }

    /// Blocks vision and bullets (full wall).
    #[inline]
    pub fn is_wall(&self, tx: i32, ty: i32) -> bool {
        self.tile(tx, ty) == TILE_WALL
    }

    /// Blocks movement (wall or low cover).
    #[inline]
    pub fn is_blocked(&self, tx: i32, ty: i32) -> bool {
        self.tile(tx, ty) != TILE_FLOOR
    }

    #[inline]
    fn inside(&self, tx: i32, ty: i32) -> bool {
        tx >= 0 && ty >= 0 && tx < self.width && ty < self.height
    }

    #[inline]
    fn index(&self, tx: i32, ty: i32) -> usize {
        (ty * self.width + tx) as usize
    }

    /// Can a unit step from tile (tx, ty) to the neighbour at (dx, dy)? Diagonals may not cut corners.
    #[inline]
    pub fn can_step(&self, tx: i32, ty: i32, dx: i32, dy: i32) -> bool {
        !self.is_blocked(tx + dx, ty + dy)
            && (dx == 0 || dy == 0 || (!self.is_blocked(tx + dx, ty) && !self.is_blocked(tx, ty + dy)))
    }
}

/// `Grid::steps` for every tile: a 3x3 block of floor flags slides along each row (a copy with a blocked
/// border, since outside the map is wall) and indexes `STEP_LUT`.
fn step_masks(w: usize, h: usize, tiles: &[u8]) -> Vec<u8> {
    let mut steps = vec![0u8; w * h];
    if w == 0 {
        return steps;
    }
    let pw = w + 2;
    let mut open = vec![0u8; pw * (h + 2)];
    for (row, src) in open.chunks_exact_mut(pw).skip(1).zip(tiles.chunks_exact(w)) {
        for (o, &t) in row[1..].iter_mut().zip(src) {
            *o = (t == TILE_FLOOR) as u8;
        }
    }
    let column = |u: u8, m: u8, d: u8| (u | m << 1 | d << 2) as usize;
    for (out, rows) in steps.chunks_exact_mut(w).zip(open.windows(3 * pw).step_by(pw)) {
        let (up, rest) = rows.split_at(pw);
        let (mid, down) = rest.split_at(pw);
        let mut block = column(up[0], mid[0], down[0]) << 3 | column(up[1], mid[1], down[1]) << 6;
        let next = up[2..].iter().zip(&mid[2..]).zip(&down[2..]);
        for (s, ((&u, &m), &d)) in out.iter_mut().zip(next) {
            block = block >> 3 | column(u, m, d) << 6;
            *s = STEP_LUT[block & 511];
        }
    }
    steps
}

/// The step mask of the centre of a 3x3 block of floor flags, where bit `3 * (dx + 1) + (dy + 1)` is the
/// tile at (dx, dy): `Grid::can_step` for every direction.
static STEP_LUT: [u8; 512] = step_lut();

const fn step_lut() -> [u8; 512] {
    let mut lut = [0u8; 512];
    let mut block = 0;
    while block < 512 {
        let mut d = 0;
        while d < 8 {
            let (dx, dy, _) = NEIGHBOURS[d];
            let can =
                floor_in(block, dx, dy) && (dx == 0 || dy == 0 || (floor_in(block, dx, 0) && floor_in(block, 0, dy)));
            lut[block] |= (can as u8) << d;
            d += 1;
        }
        block += 1;
    }
    lut
}

const fn floor_in(block: usize, dx: i32, dy: i32) -> bool {
    (block >> (3 * (dx + 1) + dy + 1)) & 1 == 1
}

/// Distance fields kept; the oldest is evicted first.
const CACHE: usize = 6;
/// Tiles a single `next_waypoint` call may settle.
const EXPAND_PER_CALL: u32 = 160;

/// 8-connected shortest paths over the wall grid, without cutting corners. Each distinct goal tile
/// gets a distance field that is grown lazily from the goal until it covers the asking unit (about 200
/// fuel per floor tile it settles), so a query for a known goal costs a handful of array reads and a new
/// goal costs at most 160 settled tiles per `next_waypoint` call. Keeps 6 fields, the oldest replaced
/// first, plus every field `complete` built. Costs per call and per map are in the module comment.
#[derive(Default)]
pub struct Pathfinder {
    fields: Vec<Field>,
    evict: usize,
    /// Fields `complete` was asked for: kept for the match, never evicted.
    kept: Vec<Field>,
}

impl Pathfinder {
    pub fn new() -> Pathfinder {
        Pathfinder::default()
    }

    fn field(&mut self, grid: &Grid, goal: (i32, i32)) -> &mut Field {
        let g = grid.index(goal.0, goal.1);
        if let Some(i) = self.kept.iter().position(|f| f.goal == g) {
            return &mut self.kept[i];
        }
        if let Some(i) = self.fields.iter().position(|f| f.goal == g) {
            return &mut self.fields[i];
        }
        if self.fields.len() < CACHE {
            self.fields.push(Field::new(grid, g));
            self.fields.last_mut().unwrap()
        } else {
            self.evict = (self.evict + 1) % CACHE;
            let field = &mut self.fields[self.evict];
            field.reset(grid, g);
            field
        }
    }

    /// Path length in world units from `from` to `goal` (tile resolution), or `None` if unreachable.
    /// Always exact, so on a goal that is new and far away it does the whole search in this call: about 200
    /// fuel per floor tile, 80,000 on `crossfire` and 160,000 on `steppe`. About 210 once the field reaches
    /// `from`. Use it in the draft or the buy phase for new goals, not mid-fight.
    pub fn path_len(&mut self, grid: &Grid, from: (i32, i32), goal: (i32, i32)) -> Option<i32> {
        let (f, g) = (tile_of(from.0, from.1), tile_of(goal.0, goal.1));
        if !grid.inside(g.0, g.1) || grid.is_blocked(f.0, f.1) {
            return None;
        }
        let (field, tile) = (self.field(grid, g), grid.index(f.0, f.1));
        field.settle(grid, tile, u32::MAX);
        let c = field.cost[tile];
        (c != UNREACHABLE).then(|| (c as i64 * TILE as i64 / ORTH as i64) as i32)
    }

    /// The point to walk towards right now to get from `from` to `goal`: the goal itself when it is in
    /// this tile or the next one, otherwise the centre of the next tile on a shortest path.
    /// `None` if there is no path, or if the search for a new goal needs more calls to reach `from`
    /// (work per call is capped so a turn never blows its fuel; ask again next tick). A call settles at
    /// most 160 tiles, up to 45,000 fuel; about 500 once the field reaches `from`.
    pub fn next_waypoint(&mut self, grid: &Grid, from: (i32, i32), goal: (i32, i32)) -> Option<(i32, i32)> {
        let (f, g) = (tile_of(from.0, from.1), tile_of(goal.0, goal.1));
        if !grid.inside(g.0, g.1) || grid.is_blocked(f.0, f.1) {
            return None;
        }
        if f == g {
            return Some(goal);
        }
        let (field, tile) = (self.field(grid, g), grid.index(f.0, f.1));
        if !field.settle(grid, tile, EXPAND_PER_CALL) || field.cost[tile] == UNREACHABLE {
            return None;
        }
        // Every neighbour that is cheaper than this tile is already final. The cheapest wins, the first
        // in `NEIGHBOURS` order on a tie.
        let (here, mask) = (field.cost[tile], grid.steps[tile]);
        let mut best = (here, f);
        for (d, &(dx, dy, step)) in NEIGHBOURS.iter().enumerate() {
            if mask & (1 << d) != 0 {
                let c = field.cost[(tile as i32 + dy * grid.width + dx) as usize];
                if c < best.0 && c + step <= here {
                    best = (c, (f.0 + dx, f.1 + dy));
                }
            }
        }
        Some(if best.1 == g { goal } else { tile_centre(best.1 .0, best.1 .1) })
    }

    /// Build the whole distance field to `goal` and keep it for the rest of the match: it is never
    /// evicted, so `path_len`, `next_waypoint` and `Navigator::go` to this goal tile are lookups from
    /// anywhere. Settles at most `budget` tiles in this call (`u32::MAX` for all, about 200 fuel each:
    /// 46,000 on `tunnels`, 160,000 on `steppe`) and returns true once the field is whole; false if the
    /// budget ran out (call again, it carries on). Made for the first tick: site centres and spawns.
    pub fn complete(&mut self, grid: &Grid, goal: (i32, i32), budget: u32) -> bool {
        let g = tile_of(goal.0, goal.1);
        if !grid.inside(g.0, g.1) {
            return true;
        }
        let g = grid.index(g.0, g.1);
        let i = match self.kept.iter().position(|f| f.goal == g) {
            Some(i) => i,
            None => {
                // Keep the work already done for this goal; its cache slot stays empty until it is reused.
                let field = match self.fields.iter().position(|f| f.goal == g) {
                    Some(j) => std::mem::replace(&mut self.fields[j], Field::vacant()),
                    None => Field::new(grid, g),
                };
                self.kept.push(field);
                self.kept.len() - 1
            }
        };
        self.kept[i].settle(grid, usize::MAX, budget)
    }
}

/// Ticks without progress before side-stepping, and how long the side-step lasts.
const STUCK_TICKS: u8 = 4;
const DODGE_TICKS: u8 = 6;
/// Closer than this to the goal counts as arrived.
const ARRIVE: i32 = 48;

/// Grid + pathfinder + stuck detection: call `go` every tick you want to travel somewhere.
pub struct Navigator {
    pub grid: Grid,
    pub paths: Pathfinder,
    /// Direction of the last move issued by `go` (face it to look where you walk).
    pub heading: i32,
    max_speed: i32,
    last: (i32, i32),
    stuck: u8,
    dodge: u8,
    dodge_angle: i32,
    flips: i32,
}

impl Navigator {
    /// Loads the map grid from the host (`Grid::load`, about 70 fuel per map tile): once, in `Bot::new`.
    pub fn new() -> Navigator {
        Navigator {
            grid: Grid::load(),
            paths: Pathfinder::new(),
            heading: 0,
            max_speed: crate::core_rules().run_speed,
            last: (0, 0),
            stuck: 0,
            dodge: 0,
            dodge_angle: 0,
            flips: 0,
        }
    }

    /// Walk towards (x, y) around walls at `speed` (capped at the run speed). Returns `Ok(true)` once there (no move issued),
    /// `Ok(false)` while travelling. If no path exists it walks straight at the goal, and so it does while
    /// the field of a new goal is still growing towards it (`next_waypoint`: up to 45,000 fuel a call).
    /// About 850 fuel a call once the field reaches the unit.
    pub fn go(&mut self, me: &UnitInfo, x: i32, y: i32, speed: i32) -> Result<bool, Error> {
        let pos = (me.x, me.y);
        let left = dist(pos, (x, y));
        if left <= ARRIVE {
            self.stuck = 0;
            return Ok(true);
        }
        // Slow down for the last step so we stop on the goal instead of orbiting it.
        let speed = speed.clamp(1, self.max_speed.max(1));
        let step = speed.min(left);
        self.stuck = if dist(pos, self.last) < (step / 8).max(2) { self.stuck + 1 } else { 0 };
        self.last = pos;
        let way = self.paths.next_waypoint(&self.grid, pos, (x, y)).unwrap_or((x, y));
        let heading = angle_to(way.0 - me.x, way.1 - me.y);
        if self.stuck >= STUCK_TICKS && self.dodge == 0 {
            // Blocked by a teammate or a corner: slide sideways for a moment, alternating sides.
            self.flips += 1;
            let side = if (me.id + self.flips) & 1 == 0 { 16384 } else { -16384 };
            self.dodge_angle = heading + side + if self.flips % 3 == 0 { side / 2 } else { 0 };
            self.dodge = DODGE_TICKS;
            self.stuck = 0;
        }
        self.heading = heading;
        if self.dodge > 0 {
            self.dodge -= 1;
            crate::move_dir(self.dodge_angle, speed)?;
        } else {
            crate::move_dir(heading, step)?;
        }
        Ok(false)
    }

    /// Path length in world units, or `None` if unreachable.
    pub fn path_len(&mut self, from: (i32, i32), goal: (i32, i32)) -> Option<i32> {
        self.paths.path_len(&self.grid, from, goal)
    }

    /// `Pathfinder::complete` on this navigator's grid and pathfinder: `go` and `path_len` to `goal` become
    /// lookups for the rest of the match.
    pub fn complete(&mut self, goal: (i32, i32), budget: u32) -> bool {
        self.paths.complete(&self.grid, goal, budget)
    }
}

impl Default for Navigator {
    fn default() -> Self {
        Navigator::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TILE_LOW;

    /// A `w` x `h` grid with about a fifth of the tiles wall and a sixteenth low cover, fixed by `seed`.
    pub(super) fn random_grid(w: i32, h: i32, mut seed: u64) -> Grid {
        let tiles = (0..w * h)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                match seed >> 60 {
                    0..=2 => TILE_WALL,
                    3 => TILE_LOW,
                    _ => TILE_FLOOR,
                }
            })
            .collect();
        Grid::from_tiles(w, h, tiles)
    }

    fn grid(rows: &[&str]) -> Grid {
        let tiles = rows.iter().flat_map(|r| r.bytes().map(|b| if b == b'#' { 1 } else { 0 })).collect();
        Grid::from_tiles(rows[0].len() as i32, rows.len() as i32, tiles)
    }

    fn walk(g: &Grid, from: (i32, i32), goal: (i32, i32)) -> Option<usize> {
        let mut pf = Pathfinder::new();
        let (mut at, target) = (tile_centre(from.0, from.1), tile_centre(goal.0, goal.1));
        pf.path_len(g, at, target)?;
        for steps in 0..200 {
            if at == target {
                return Some(steps);
            }
            let next = pf.next_waypoint(g, at, target)?;
            let (a, b) = (tile_of(at.0, at.1), tile_of(next.0, next.1));
            assert!(g.can_step(a.0, a.1, b.0 - a.0, b.1 - a.1), "illegal step {a:?} -> {b:?}");
            at = next;
        }
        panic!("did not arrive");
    }

    #[test]
    fn routes_around_walls_without_cutting_corners() {
        let g = grid(&["#######", "#..#..#", "#..#..#", "#.....#", "#######"]);
        assert_eq!(walk(&g, (1, 1), (5, 1)), Some(6));
        assert_eq!(walk(&g, (1, 1), (2, 2)), Some(1));
        let mut pf = Pathfinder::new();
        assert_eq!(pf.path_len(&g, tile_centre(1, 1), tile_centre(1, 3)), Some(2 * TILE));
    }

    #[test]
    fn unreachable_is_none() {
        let g = grid(&["#####", "#.#.#", "#####"]);
        assert_eq!(walk(&g, (1, 1), (3, 1)), None);
        let mut pf = Pathfinder::new();
        assert_eq!(pf.next_waypoint(&g, tile_centre(1, 1), (-5000, 0)), None);
    }

    #[test]
    fn cache_eviction_keeps_answers_right() {
        let g = grid(&["..........", "..........", ".........."]);
        let mut pf = Pathfinder::new();
        for round in 0..3 {
            for x in 0..10 {
                let len = pf.path_len(&g, tile_centre(0, 0), tile_centre(x, 0));
                assert_eq!(len, Some(x * TILE), "round {round}");
            }
        }
    }

    #[test]
    fn big_searches_are_spread_over_calls_and_stay_exact() {
        let rows = vec![".".repeat(60); 60];
        let g = grid(&rows.iter().map(String::as_str).collect::<Vec<_>>());
        let (from, goal) = (tile_centre(0, 0), tile_centre(59, 30));
        let mut lazy = Pathfinder::new();
        let mut calls = 1;
        let first = loop {
            match lazy.next_waypoint(&g, from, goal) {
                Some(p) => break p,
                None => calls += 1,
            }
            assert!(calls < 100, "search never finished");
        };
        assert!(calls > 5, "a 3600-tile search should not fit in one call ({calls})");
        assert_eq!(first, tile_centre(1, 1));
        // 30 diagonal + 29 straight steps, and the same answer when asked exactly up front.
        let want = Some(((30 * DIAG + 29 * ORTH) as i64 * TILE as i64 / ORTH as i64) as i32);
        assert_eq!(lazy.path_len(&g, from, goal), want);
        assert_eq!(Pathfinder::new().path_len(&g, from, goal), want);
    }

    #[test]
    fn step_masks_follow_can_step() {
        for (w, h, seed) in [(17, 11, 1), (1, 1, 2), (1, 6, 3), (6, 1, 4), (2, 2, 5)] {
            let g = random_grid(w, h, seed);
            for (t, &mask) in g.steps.iter().enumerate() {
                let (tx, ty) = (t as i32 % w, t as i32 / w);
                for (d, &(dx, dy, _)) in NEIGHBOURS.iter().enumerate() {
                    assert_eq!(mask & (1 << d) != 0, g.can_step(tx, ty, dx, dy), "{w}x{h} tile {t} dir {d}");
                }
            }
        }
    }

    #[test]
    fn complete_fields_are_whole_and_kept() {
        let g = random_grid(60, 40, 7);
        let centre = |t: i32| tile_centre(t % 60, t / 60);
        let goal = (0..2400).map(centre).find(|&p| g.tile_at(p.0, p.1) == TILE_FLOOR).unwrap();
        let mut kept = Pathfinder::new();
        let slices = (1..).find(|_| kept.complete(&g, goal, 100)).unwrap();
        assert!(slices > 5, "a 2400-tile field should not fit in 100 tiles ({slices})");
        // Other goals cycle through the cache; the kept field stays whole.
        for t in 0..20 {
            let _ = kept.path_len(&g, centre(t), centre(2399 - t));
        }
        let mut fresh = Pathfinder::new();
        for from in (0..2400).map(centre) {
            assert_eq!(kept.path_len(&g, from, goal), fresh.path_len(&g, from, goal));
            assert_eq!(kept.next_waypoint(&g, from, goal), fresh.next_waypoint(&g, from, goal));
        }
        assert!(kept.complete(&g, goal, 0), "already whole");
    }

    #[test]
    fn blocked_goal_leads_next_to_it() {
        let g = grid(&["#####", "#...#", "#.#.#", "#####"]);
        let mut pf = Pathfinder::new();
        let goal = tile_centre(2, 2);
        // Tiles beside the wall count as one step away and are where the walk ends.
        assert_eq!(pf.path_len(&g, tile_centre(1, 2), goal), Some(TILE));
        assert_eq!(pf.path_len(&g, tile_centre(1, 1), goal), Some(2 * TILE));
        assert_eq!(pf.next_waypoint(&g, tile_centre(1, 1), goal), Some(tile_centre(2, 1)));
        assert_eq!(pf.next_waypoint(&g, tile_centre(2, 1), goal), Some(tile_centre(2, 1)));
    }
}
