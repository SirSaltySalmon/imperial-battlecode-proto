//! The numbers a designer needs for an attack/defend map. All travel is any-angle Euclidean for a
//! unit-radius circle (`path::VisGraph`), all sight is the engine's `segment_wall`, and all times are
//! ticks at the ruleset's `run_speed`.

use crate::bands::{Band, Bands};
use crate::flow::min_cut;
use crate::path::{VisGraph, UNREACHABLE};
use crate::sight::{longest_sightlines, los, spawn_sightline, tiles_seeing};
use crate::tilegraph::TileGraph;
use crate::{
    ticks_for, tile_centre, tile_of, used_spawns, Map, Ruleset, TILE, TILE_LOW, TILE_WALL, V2,
};
use ibc_engine::fixed;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct RulesUsed {
    pub unit_radius: i32,
    pub run_speed: i32,
    pub round_ticks: i32,
    pub plant_ticks: i32,
    pub fuse_ticks: i32,
    pub defuse_ticks: i32,
    pub interact_reach: i32,
    pub vision_range: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChokeReport {
    /// Middle tile of the choke.
    pub at: (i32, i32),
    pub width_tiles: u32,
    pub tiles: Vec<(i32, i32)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Hold {
    pub at: (i32, i32),
    pub share_pct: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Retake {
    /// Where the defender starts: `site N` or `defender spawn`.
    pub from: String,
    /// Ticks to run within `interact_reach` of the charge.
    pub run_ticks: i64,
    /// Run plus `defuse_ticks`.
    pub total_ticks: i64,
    /// `fuse_ticks` minus total; negative means the charge detonates first.
    pub slack_ticks: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SiteReport {
    /// Site number as written in the map file (1-based).
    pub site: i32,
    pub tiles: usize,
    /// The plant point (`SiteInfo.plant_x/plant_y`): where bots plant, and where every number below is
    /// measured from. Unlike the bounding-box centre it is on the site whatever the site's shape.
    pub plant: V2,
    /// World units and ticks from the fastest spawn tile to the plant point; -1 if unreachable.
    pub attacker_dist: i64,
    pub attacker_ticks: i64,
    pub defender_dist: i64,
    pub defender_ticks: i64,
    /// Attacker ticks minus defender ticks (positive: defenders are there first).
    pub advantage_ticks: i64,
    /// Earliest tick of the action phase at which a plant can complete.
    pub earliest_plant_tick: i64,
    /// Distinct chokepoints between the attacker spawn zone and the site; -1 when there is no choke.
    pub routes: i64,
    pub cut_width: i64,
    pub chokes: Vec<ChokeReport>,
    pub hold_positions: usize,
    pub best_hold_share_pct: i64,
    pub mean_hold_share_pct: i64,
    pub best_holds: Vec<Hold>,
    pub cover_wall_pct: i64,
    pub cover_low_pct: i64,
    pub cover_pct: i64,
    /// Walkable tiles with a clear line to a charge planted on the plant point.
    pub postplant_visible_tiles: usize,
    pub postplant_visible_pct: i64,
    pub postplant_farthest: Option<(i32, i32)>,
    pub retakes: Vec<Retake>,
    /// Shortest routes to the plant point (world units), for rendering.
    pub attacker_path: Vec<V2>,
    pub defender_path: Vec<V2>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Rotation {
    pub from_site: i32,
    pub to_site: i32,
    pub dist: i64,
    pub ticks: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SightlineReport {
    pub from: (i32, i32),
    pub to: (i32, i32),
    pub length: i64,
    /// Length in tiles, times ten.
    pub tiles_x10: i64,
    pub beyond_vision: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Flag {
    pub metric: &'static str,
    /// `map`, `site N` or `site N -> site M`.
    pub scope: String,
    pub value: i64,
    pub band: Band,
    /// `low`, `high` or `fail`.
    pub verdict: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub map: String,
    pub width: i32,
    pub height: i32,
    pub team_size: i32,
    pub walkable_tiles: usize,
    pub rules: RulesUsed,
    /// Attacker and defender spawn tiles that see each other at round start, if any.
    pub spawn_sightline: Option<(V2, V2)>,
    /// Ticks for the two teams to meet if both run straight at each other's spawn.
    pub spawn_to_spawn_ticks: i64,
    pub sites: Vec<SiteReport>,
    pub rotations: Vec<Rotation>,
    pub sightlines: Vec<SightlineReport>,
    pub bands: Bands,
    pub flags: Vec<Flag>,
    /// Per tile: travel distance from the attacker / defender spawns (`path::UNREACHABLE` if none).
    #[serde(skip)]
    pub tile_dist: [Vec<i64>; 2],
}

fn pct(n: usize, d: usize) -> i64 {
    if d == 0 {
        0
    } else {
        (n as i64 * 100 + d as i64 / 2) / d as i64
    }
}

pub fn analyze(map: &Map, rules: &Ruleset, bands: &Bands) -> Analysis {
    let core = &rules.core;
    let r = core.unit_radius;
    let g = TileGraph::build(map, r);
    let vis = VisGraph::build(map, r);
    let fields = [
        vis.field(used_spawns(map, 0)),
        vis.field(used_spawns(map, 1)),
    ];
    let tile_dist = [vis.tile_dists(&fields[0]), vis.tile_dists(&fields[1])];
    let walkable = g.stand.iter().filter(|&&s| s).count();
    let plants: Vec<V2> = map.sites.iter().map(|s| (s.plant_x, s.plant_y)).collect();
    let site_fields: Vec<_> = plants.iter().map(|&c| vis.field(&[c])).collect();

    // Attacker spawn zone: the source side of every route cut.
    let starts: Vec<usize> = used_spawns(map, 0)
        .iter()
        .map(|&p| g.index(tile_of(p).0, tile_of(p).1))
        .collect();
    let near_spawn: Vec<bool> = g
        .steps_from(&starts)
        .iter()
        .map(|&d| (d as i64) <= bands.spawn_zone_steps)
        .collect();

    let mut sites = Vec::new();
    for (k, site) in map.sites.iter().enumerate() {
        let c = plants[k];
        let own: Vec<bool> = map.site_of.iter().map(|&s| s as usize == k + 1).collect();
        let site_tiles: Vec<usize> = (0..own.len()).filter(|&i| own[i]).collect();
        let timing = |side: usize| match vis.dist_to(&fields[side], c) {
            Some(d) => (d, ticks_for(d, core.run_speed)),
            None => (-1, -1),
        };
        let ((a_dist, a_ticks), (d_dist, d_ticks)) = (timing(0), timing(1));

        // Source side of the route cut: the spawn surroundings plus the first part of the approach.
        let start = a_dist.max(0) * bands.approach_start_pct / 100;
        let zone: Vec<bool> = (0..g.stand.len())
            .map(|i| near_spawn[i] || (g.stand[i] && tile_dist[0][i] <= start))
            .collect();
        let cut = min_cut(&g, &zone, &own);
        let chokes: Vec<ChokeReport> = cut
            .iter()
            .flat_map(|c| &c.chokes)
            .map(|ch| ChokeReport {
                at: ch.tiles[ch.tiles.len() / 2],
                width_tiles: ch.width(),
                tiles: ch.tiles.clone(),
            })
            .collect();

        // Hold positions: off-site tiles near the site that defenders reach clearly first and that
        // see at least one site tile.
        let mut holds: Vec<Hold> = Vec::new();
        for i in (0..g.stand.len()).filter(|&i| g.stand[i] && !own[i]) {
            let (tx, ty) = g.coords(i);
            let p = tile_centre(tx, ty);
            let d = fixed::dist(p, c) as i64;
            let in_ring = d >= bands.hold_radius_tiles[0] * TILE as i64
                && d <= bands.hold_radius_tiles[1] * TILE as i64;
            let first = tile_dist[1][i] < UNREACHABLE
                && ticks_for(tile_dist[1][i], core.run_speed) + bands.hold_lead_ticks
                    <= ticks_for(tile_dist[0][i], core.run_speed);
            if !in_ring || !first {
                continue;
            }
            let seen = site_tiles
                .iter()
                .filter(|&&j| {
                    let q = tile_centre(g.coords(j).0, g.coords(j).1);
                    fixed::dist(p, q) <= core.vision_range && los(map, p, q)
                })
                .count();
            if seen > 0 {
                holds.push(Hold {
                    at: (tx, ty),
                    share_pct: pct(seen, site_tiles.len()),
                });
            }
        }
        holds.sort_by_key(|h| (std::cmp::Reverse(h.share_pct), h.at.1, h.at.0));
        let mean_share = if holds.is_empty() {
            0
        } else {
            holds.iter().map(|h| h.share_pct).sum::<i64>() / holds.len() as i64
        };

        // Cover density in a disc around the plant point (site tiles themselves excluded).
        let radius = bands.cover_radius_tiles * TILE as i64;
        let (mut area, mut walls, mut lows) = (0usize, 0usize, 0usize);
        for ty in 0..map.height {
            for tx in 0..map.width {
                let i = g.index(tx, ty);
                if own[i] || fixed::dist(tile_centre(tx, ty), c) as i64 > radius {
                    continue;
                }
                area += 1;
                walls += (map.tiles[i] == TILE_WALL) as usize;
                lows += (map.tiles[i] == TILE_LOW) as usize;
            }
        }

        let seeing = tiles_seeing(map, &g.stand, c, core.vision_range);
        let farthest = seeing.iter().copied().max_by_key(|&i| {
            let (tx, ty) = g.coords(i);
            (fixed::dist(tile_centre(tx, ty), c), std::cmp::Reverse(i))
        });

        let mut retakes = Vec::new();
        let mut retake = |from: String, dist: Option<i64>| {
            if let Some(d) = dist {
                let run_ticks = ticks_for(d - core.interact_reach as i64, core.run_speed);
                let total = run_ticks + core.defuse_ticks as i64;
                retakes.push(Retake {
                    from,
                    run_ticks,
                    total_ticks: total,
                    slack_ticks: core.fuse_ticks as i64 - total,
                });
            }
        };
        for (j, f) in site_fields.iter().enumerate() {
            if j != k {
                retake(format!("site {}", j + 1), vis.dist_to(f, c));
            }
        }
        retake("defender spawn".into(), (d_dist >= 0).then_some(d_dist));

        sites.push(SiteReport {
            site: site.id + 1,
            tiles: site_tiles.len(),
            plant: c,
            attacker_dist: a_dist,
            attacker_ticks: a_ticks,
            defender_dist: d_dist,
            defender_ticks: d_ticks,
            advantage_ticks: a_ticks - d_ticks,
            earliest_plant_tick: a_ticks + core.plant_ticks as i64,
            routes: cut.as_ref().map_or(-1, |c| c.chokes.len() as i64),
            cut_width: cut.as_ref().map_or(-1, |c| c.width as i64),
            chokes,
            hold_positions: holds.len(),
            best_hold_share_pct: holds.first().map_or(0, |h| h.share_pct),
            mean_hold_share_pct: mean_share,
            best_holds: holds.into_iter().take(3).collect(),
            cover_wall_pct: pct(walls, area),
            cover_low_pct: pct(lows, area),
            cover_pct: pct(walls + lows, area),
            postplant_visible_tiles: seeing.len(),
            postplant_visible_pct: pct(seeing.len(), walkable),
            postplant_farthest: farthest.map(|i| g.coords(i)),
            retakes,
            attacker_path: vis.path_to(&fields[0], c).unwrap_or_default(),
            defender_path: vis.path_to(&fields[1], c).unwrap_or_default(),
        });
    }

    let mut rotations = Vec::new();
    for i in 0..plants.len() {
        for j in i + 1..plants.len() {
            if let Some(d) = vis.dist_to(&site_fields[i], plants[j]) {
                rotations.push(Rotation {
                    from_site: i as i32 + 1,
                    to_site: j as i32 + 1,
                    dist: d,
                    ticks: ticks_for(d, core.run_speed),
                });
            }
        }
    }

    let sightlines: Vec<SightlineReport> =
        longest_sightlines(map, &g.stand, bands.sightlines_listed.max(1) as usize, 3)
            .into_iter()
            .map(|s| SightlineReport {
                from: s.a,
                to: s.b,
                length: s.len,
                tiles_x10: s.len * 10 / TILE as i64,
                beyond_vision: s.len > core.vision_range as i64,
            })
            .collect();

    // The teams close on each other at twice the run speed.
    let spawn_gap = used_spawns(map, 1)
        .iter()
        .filter_map(|&d| vis.dist_to(&fields[0], d))
        .min();
    let spawn_to_spawn_ticks = spawn_gap.map_or(-1, |d| ticks_for(d, core.run_speed * 2));

    let mut a = Analysis {
        map: map.name.clone(),
        width: map.width,
        height: map.height,
        team_size: map.team_size,
        walkable_tiles: walkable,
        rules: RulesUsed {
            unit_radius: r,
            run_speed: core.run_speed,
            round_ticks: core.round_ticks,
            plant_ticks: core.plant_ticks,
            fuse_ticks: core.fuse_ticks,
            defuse_ticks: core.defuse_ticks,
            interact_reach: core.interact_reach,
            vision_range: core.vision_range,
        },
        spawn_sightline: spawn_sightline(map),
        spawn_to_spawn_ticks,
        sites,
        rotations,
        sightlines,
        bands: bands.clone(),
        flags: Vec::new(),
        tile_dist,
    };
    a.flags = flags(&a);
    a
}

/// Every value outside its band.
fn flags(a: &Analysis) -> Vec<Flag> {
    let b = &a.bands;
    let mut out = Vec::new();
    let mut check = |metric: &'static str, scope: String, value: i64, band: Band| {
        if value < band[0] || value > band[1] {
            out.push(Flag {
                metric,
                scope,
                value,
                band,
                verdict: if value < band[0] { "low" } else { "high" },
            });
        }
    };
    for s in &a.sites {
        let scope = format!("site {}", s.site);
        if s.attacker_ticks < 0 || s.defender_ticks < 0 {
            check("reachable", scope, 0, [1, 1]);
            continue;
        }
        check(
            "attacker_ticks",
            scope.clone(),
            s.attacker_ticks,
            b.attacker_ticks,
        );
        check(
            "defender_ticks",
            scope.clone(),
            s.defender_ticks,
            b.defender_ticks,
        );
        check(
            "advantage_ticks",
            scope.clone(),
            s.advantage_ticks,
            b.advantage_ticks,
        );
        // "No choke at all" counts as too many routes.
        check(
            "routes",
            scope.clone(),
            if s.routes < 0 {
                i64::from(u8::MAX)
            } else {
                s.routes
            },
            b.routes,
        );
        if s.cut_width >= 0 {
            check("cut_width", scope.clone(), s.cut_width, b.cut_width);
        }
        check(
            "best_hold_share_pct",
            scope.clone(),
            s.best_hold_share_pct,
            b.best_hold_share_pct,
        );
        check(
            "mean_hold_share_pct",
            scope.clone(),
            s.mean_hold_share_pct,
            b.mean_hold_share_pct,
        );
        check("cover_pct", scope.clone(), s.cover_pct, b.cover_pct);
        check(
            "postplant_visible_pct",
            scope.clone(),
            s.postplant_visible_pct,
            b.postplant_visible_pct,
        );
        // The retake that matters is from the nearest other site (or from spawn on a one-site map).
        let from_site = s
            .retakes
            .iter()
            .filter(|r| r.from.starts_with("site"))
            .map(|r| r.slack_ticks)
            .max();
        let slack = from_site.or_else(|| s.retakes.iter().map(|r| r.slack_ticks).max());
        if let Some(slack) = slack {
            check(
                "retake_slack_ticks",
                scope.clone(),
                slack,
                b.retake_slack_ticks,
            );
        }
        if s.earliest_plant_tick > a.rules.round_ticks as i64 {
            check(
                "plant_in_time",
                scope,
                s.earliest_plant_tick,
                [0, a.rules.round_ticks as i64],
            );
        }
    }
    for r in &a.rotations {
        check(
            "rotation_ticks",
            format!("site {} -> site {}", r.from_site, r.to_site),
            r.ticks,
            b.rotation_ticks,
        );
    }
    if let Some(s) = a.sightlines.first() {
        check(
            "longest_sightline_tiles",
            "map".into(),
            s.tiles_x10 / 10,
            b.longest_sightline_tiles,
        );
    }
    if a.spawn_to_spawn_ticks >= 0 {
        check(
            "spawn_to_spawn_ticks",
            "map".into(),
            a.spawn_to_spawn_ticks,
            b.spawn_to_spawn_ticks,
        );
    }
    if a.spawn_sightline.is_some() {
        out.push(Flag {
            metric: "spawn_sightline",
            scope: "map".into(),
            value: 1,
            band: [0, 0],
            verdict: "fail",
        });
    }
    out
}
