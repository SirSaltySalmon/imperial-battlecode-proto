//! Wall grid, cached distance-field pathfinder and a stuck-proof `Navigator`.
use crate::geom::{angle_to, dist};
use crate::{sys, Error, UnitInfo, TILE, TILE_FLOOR, TILE_WALL, T_MAP_ROW};

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
    /// Load every map row from the host.
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
        let mut grid = Grid { width, height, tiles, steps: Vec::new() };
        let mut steps = Vec::with_capacity(grid.tiles.len());
        for ty in 0..height {
            for tx in 0..width {
                let mut bits = 0u8;
                for (d, &(dx, dy, _)) in NEIGHBOURS.iter().enumerate() {
                    bits |= (grid.can_step(tx, ty, dx, dy) as u8) << d;
                }
                steps.push(bits);
            }
        }
        grid.steps = steps;
        grid
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

const UNREACHABLE: u32 = u32::MAX;
/// Distance fields kept; the oldest is evicted first.
const CACHE: usize = 6;
/// Tiles a single `next_waypoint` call may settle (roughly 200 wasm instructions each).
const EXPAND_PER_CALL: u32 = 160;

/// Shortest-path costs from every tile to one goal tile, grown outwards from the goal on demand
/// (Dijkstra with a bucket queue: steps cost 5 or 7, so 8 buckets are enough).
struct Field {
    goal: usize,
    cost: Vec<u32>,
    buckets: [Vec<u32>; 8],
    /// Cost level being drained; every tile with `cost <= level` is final.
    level: u32,
    queued: usize,
}

impl Field {
    fn new(grid: &Grid, goal: usize) -> Field {
        let mut field =
            Field { goal, cost: vec![UNREACHABLE; grid.tiles.len()], buckets: Default::default(), level: 0, queued: 1 };
        field.cost[goal] = 0;
        field.buckets[0].push(goal as u32);
        field
    }

    /// Grow the field until the cost of `tile` is final, settling at most `budget` tiles.
    /// Returns false if the budget ran out first.
    fn settle(&mut self, grid: &Grid, tile: usize, mut budget: u32) -> bool {
        let offsets = NEIGHBOURS.map(|(dx, dy, _)| dy * grid.width + dx);
        loop {
            if self.cost[tile] <= self.level || self.queued == 0 {
                return true;
            }
            let Some(at) = self.buckets[(self.level & 7) as usize].pop() else {
                self.level += 1;
                continue;
            };
            self.queued -= 1;
            let at = at as usize;
            if self.cost[at] != self.level {
                continue; // stale entry: this tile was reached more cheaply already
            }
            if budget == 0 {
                self.buckets[(self.level & 7) as usize].push(at as u32);
                self.queued += 1;
                return false;
            }
            budget -= 1;
            // Stepping is symmetric between floor tiles, so the goal-outwards search can use the forward
            // step masks. A blocked goal still seeds its orthogonal neighbours ("as close as possible").
            let mut bits = grid.steps[at];
            if at == self.goal && grid.tiles[at] != TILE_FLOOR {
                bits &= ORTH_BITS;
            }
            for d in 0..8 {
                if bits & (1 << d) == 0 {
                    continue;
                }
                let n = (at as i32 + offsets[d]) as usize;
                let c = self.level + NEIGHBOURS[d].2;
                if c < self.cost[n] {
                    self.cost[n] = c;
                    self.buckets[(c & 7) as usize].push(n as u32);
                    self.queued += 1;
                }
            }
        }
    }
}

/// 8-connected shortest paths over the wall grid, without cutting corners. Each distinct goal tile
/// gets a cached distance field that is grown lazily from the goal until it covers the asking unit,
/// so a query for a known goal costs a handful of array reads and a new goal costs at most
/// `EXPAND_PER_CALL` settled tiles per `next_waypoint` call.
#[derive(Default)]
pub struct Pathfinder {
    fields: Vec<Field>,
    evict: usize,
}

impl Pathfinder {
    pub fn new() -> Pathfinder {
        Pathfinder::default()
    }

    fn field(&mut self, grid: &Grid, goal: (i32, i32)) -> &mut Field {
        let g = grid.index(goal.0, goal.1);
        if let Some(i) = self.fields.iter().position(|f| f.goal == g) {
            return &mut self.fields[i];
        }
        let field = Field::new(grid, g);
        if self.fields.len() < CACHE {
            self.fields.push(field);
            self.fields.last_mut().unwrap()
        } else {
            self.evict = (self.evict + 1) % CACHE;
            self.fields[self.evict] = field;
            &mut self.fields[self.evict]
        }
    }

    /// Path length in world units from `from` to `goal` (tile resolution), or `None` if unreachable.
    /// Always exact, so on a goal that is new and far away it does the whole search in this call
    /// (about 200 wasm instructions per map tile): use it in the buy phase, not mid-fight.
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
    /// (work per call is capped so a turn never blows its fuel; ask again next tick).
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
        // Every neighbour that is cheaper than this tile is already final.
        let here = field.cost[tile];
        let mut best = (here, f);
        for (d, &(dx, dy, step)) in NEIGHBOURS.iter().enumerate() {
            if grid.steps[tile] & (1 << d) == 0 {
                continue;
            }
            let c = field.cost[grid.index(f.0 + dx, f.1 + dy)];
            if c != UNREACHABLE && c + step <= here && c < best.0 {
                best = (c, (f.0 + dx, f.1 + dy));
            }
        }
        Some(if best.1 == g { goal } else { tile_centre(best.1 .0, best.1 .1) })
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
    /// Loads the map grid from the host.
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
    /// `Ok(false)` while travelling. If no path exists it walks straight at the goal.
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
}

impl Default for Navigator {
    fn default() -> Self {
        Navigator::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
