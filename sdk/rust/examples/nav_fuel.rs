//! Fuel probe for the navigation helpers: what `Grid::load`, `Navigator::new`, `Pathfinder::path_len`,
//! `next_waypoint`, `Pathfinder::complete`, `Navigator::go`, `tactics::approach_point` and
//! `tactics::hold_spot` cost under the real wasm metering, on the map it is run on.
//!
//! Build it like a bot (same profile as `bots/*`) and run it with a budget big enough that every call fits
//! in one tick, from the repository root:
//!
//! ```sh
//! (cd sdk/rust && CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
//!   CARGO_PROFILE_RELEASE_PANIC=abort cargo build --release --target wasm32-wasip1 --example nav_fuel)
//! ibc run sdk/rust/target/wasm32-wasip1/release/examples/nav_fuel.wasm fixtures/golden/bots/idle.wasm \
//!   --map maps/crossfire.ibcmap --fuel-mult-a 1000 --max-ticks 1 -o out/nav_fuel/crossfire
//! ```
//!
//! Slot 0 of team A writes one `name value...` line per measurement to `out/nav_fuel/crossfire/team-a.log`
//! (fuel net of the probe's own overhead, plus the answers, so two builds can be compared for behaviour).
//! Pairs are (attacker spawn or defender spawn) to each site centre; `cold` is a fresh `Pathfinder`. Run it
//! on every file in `maps/` to refresh the tables at the top of `src/nav.rs`.
use ibc::tactics::{approach_point, hold_spot};
use ibc::*;
use std::hint::black_box;

struct Probe {
    overhead: i64,
}

impl Probe {
    /// Fuel `f` uses, net of the two `fuel_remaining` calls around it.
    fn cost<T>(&self, f: impl FnOnce() -> T) -> (i64, T) {
        let before = fuel_remaining();
        let out = black_box(f());
        (before - fuel_remaining() - self.overhead, out)
    }

    /// `next_waypoint` until it answers: (total fuel, calls, most fuel in one call, the answer).
    fn waypoint(
        &self,
        pf: &mut Pathfinder,
        grid: &Grid,
        from: (i32, i32),
        goal: (i32, i32),
    ) -> (i64, u32, i64, String) {
        let (mut total, mut most) = (0, 0);
        for calls in 1..=1000 {
            let (fuel, way) = self.cost(|| black_box(&mut *pf).next_waypoint(grid, from, goal));
            (total, most) = (total + fuel, most.max(fuel));
            if way.is_some() || calls == 1000 {
                return (total, calls, most, format!("{way:?}"));
            }
        }
        unreachable!()
    }
}

fn floor_tiles(grid: &Grid) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for ty in 0..grid.height {
        for tx in 0..grid.width {
            if !grid.is_blocked(tx, ty) {
                out.push(tile_centre(tx, ty));
            }
        }
    }
    out
}

fn measure() {
    let mut p = Probe { overhead: 0 };
    p.overhead = p.cost(|| ()).0;
    let info = map_info();
    let spawns = [("att", (info.spawn_x, info.spawn_y)), ("def", (info.enemy_spawn_x, info.enemy_spawn_y))];
    let sites: Vec<(i32, i32)> = sites().map(|s| (s.centre_x, s.centre_y)).collect();
    let (fuel, grid) = p.cost(Grid::load);
    log!("grid_load {fuel}");
    let (fuel, mut nav) = p.cost(Navigator::new);
    log!("nav_new {fuel}");

    // The whole field: from the reachable tile farthest from the attacker spawn, on a fresh pathfinder.
    let goal = spawns[0].1;
    let floor = floor_tiles(&grid);
    let mut lens = Pathfinder::new();
    let reach: Vec<((i32, i32), i32)> =
        floor.iter().filter_map(|&t| lens.path_len(&grid, t, goal).map(|l| (t, l))).collect();
    let far = reach.iter().max_by_key(|&&(t, l)| (l, t)).map_or(goal, |r| r.0);
    log!("tiles {} floor {} reachable {}", grid.width * grid.height, floor.len(), reach.len());
    let (fuel, len) = p.cost(|| black_box(&mut Pathfinder::new()).path_len(&grid, far, goal));
    log!("field_full {fuel} {len:?}");
    let (fuel, done) = p.cost(|| black_box(&mut Pathfinder::new()).complete(&grid, goal, u32::MAX));
    log!("complete {fuel} {done}");
    let mut kept = Pathfinder::new();
    kept.complete(&grid, goal, u32::MAX);
    let (fuel, len) = p.cost(|| black_box(&mut kept).path_len(&grid, far, goal));
    log!("complete_lookup {fuel} {len:?}");

    for (who, from) in spawns {
        for (i, &site) in sites.iter().enumerate() {
            let pair = format!("{who}-site{i}");
            let mut pf = Pathfinder::new();
            let (fuel, len) = p.cost(|| black_box(&mut pf).path_len(&grid, from, site));
            log!("path_len_cold {pair} {fuel} {len:?}");
            let (fuel, _) = p.cost(|| black_box(&mut pf).path_len(&grid, from, site));
            log!("path_len_warm {pair} {fuel}");
            let (fuel, calls, most, way) = p.waypoint(&mut Pathfinder::new(), &grid, from, site);
            log!("next_waypoint_cold {pair} {fuel} {calls} {most} {way}");
            let (fuel, _) = p.cost(|| black_box(&mut pf).next_waypoint(&grid, from, site));
            log!("next_waypoint_warm {pair} {fuel}");

            let me = UnitInfo { id: 0, x: from.0, y: from.1, ..Default::default() };
            nav.paths = Pathfinder::new();
            let (fuel, _) = p.cost(|| black_box(&mut nav).go(&me, site.0, site.1, 700));
            log!("go_first {pair} {fuel} {}", nav.heading);
            let _ = nav.path_len(from, site);
            let (fuel, _) = p.cost(|| black_box(&mut nav).go(&me, site.0, site.1, 700));
            log!("go_warm {pair} {fuel} {}", nav.heading);

            if who == "att" {
                nav.paths = Pathfinder::new();
                let (fuel, watch) = p.cost(|| approach_point(black_box(&mut nav), site, spawns[1].1));
                log!("approach_cold site{i} {fuel} {watch:?}");
                let (fuel, _) = p.cost(|| approach_point(black_box(&mut nav), site, spawns[1].1));
                log!("approach_warm site{i} {fuel}");
                let (fuel, post) = p.cost(|| hold_spot(black_box(&nav), site, watch, 3, 0));
                log!("hold_spot site{i} {fuel} {post:?}");
            }
        }
    }
}

struct NavFuel;

impl Bot for NavFuel {
    fn new() -> Self {
        if match_info().my_team == 0 && match_info().unit_slot == 0 {
            measure();
        }
        NavFuel
    }

    fn turn(&mut self) {}
}

bot!(NavFuel);
