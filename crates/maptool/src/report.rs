//! `analyze` output: Markdown for people, JSON for scripts.

use crate::analyze::{Analysis, SiteReport};
use crate::bands::Band;
use crate::TILE;
use std::fmt::Write;

fn tiles(units: i64) -> String {
    format!("{:.1}", units as f64 / TILE as f64)
}

fn band(b: Band) -> String {
    format!("{}..{}", b[0], b[1])
}

/// ` (low)` / ` (high)` when a value is outside its band.
fn mark(a: &Analysis, metric: &str, scope: &str) -> String {
    match a
        .flags
        .iter()
        .find(|f| f.metric == metric && f.scope == scope)
    {
        Some(f) => format!(" **({})**", f.verdict),
        None => String::new(),
    }
}

pub fn json(a: &Analysis) -> String {
    serde_json::to_string_pretty(a).expect("analysis serialises")
}

pub fn markdown(a: &Analysis) -> String {
    let mut o = String::new();
    let b = &a.bands;
    let _ = writeln!(
        o,
        "# {} ({}x{}, {}v{})\n",
        a.map, a.width, a.height, a.team_size, a.team_size
    );
    let _ = writeln!(
        o,
        "Walkable tiles: {}. Travel is any-angle for a circle of radius {} at run speed {} per tick; sight is exact \
         engine line of sight between tile centres. Round {} ticks, plant {}, fuse {}, defuse {}.\n",
        a.walkable_tiles, a.rules.unit_radius, a.rules.run_speed, a.rules.round_ticks, a.rules.plant_ticks,
        a.rules.fuse_ticks, a.rules.defuse_ticks
    );

    let _ = writeln!(o, "## Timing\n");
    let _ = writeln!(o, "| Site | Attackers (ticks / tiles) | Defenders (ticks / tiles) | Advantage | First arrival | Earliest plant |");
    let _ = writeln!(o, "|---|---|---|---|---|---|");
    for s in &a.sites {
        let scope = format!("site {}", s.site);
        let who = match s.advantage_ticks {
            t if t > 0 => format!("defenders by {t}"),
            0 => "level".to_string(),
            t => format!("attackers by {}", -t),
        };
        let _ = writeln!(
            o,
            "| {} | {} / {}{} | {} / {}{} | {}{} | {} | tick {} |",
            s.site,
            s.attacker_ticks,
            tiles(s.attacker_dist),
            mark(a, "attacker_ticks", &scope),
            s.defender_ticks,
            tiles(s.defender_dist),
            mark(a, "defender_ticks", &scope),
            s.advantage_ticks,
            mark(a, "advantage_ticks", &scope),
            who,
            s.earliest_plant_tick
        );
    }
    let _ = writeln!(
        o,
        "\nBands: attackers {} ticks, defenders {} ticks, advantage (attacker minus defender) {} ticks. \
         Teams running at each other meet after about {} ticks{} (band {}).\n",
        band(b.attacker_ticks),
        band(b.defender_ticks),
        band(b.advantage_ticks),
        a.spawn_to_spawn_ticks,
        mark(a, "spawn_to_spawn_ticks", "map"),
        band(b.spawn_to_spawn_ticks)
    );

    if !a.rotations.is_empty() {
        let _ = writeln!(o, "## Rotation and retake\n");
        let _ = writeln!(o, "| Between | Ticks | Tiles |");
        let _ = writeln!(o, "|---|---|---|");
        for r in &a.rotations {
            let scope = format!("site {} -> site {}", r.from_site, r.to_site);
            let _ = writeln!(
                o,
                "| {} | {}{} | {} |",
                scope,
                r.ticks,
                mark(a, "rotation_ticks", &scope),
                tiles(r.dist)
            );
        }
        let _ = writeln!(o, "\nBand: rotation {} ticks.\n", band(b.rotation_ticks));
    } else {
        let _ = writeln!(o, "## Retake\n");
    }
    let _ = writeln!(
        o,
        "| Charge on | Defender from | Run | Run + defuse | Fuse slack | Verdict |"
    );
    let _ = writeln!(o, "|---|---|---|---|---|---|");
    for s in &a.sites {
        for r in &s.retakes {
            let verdict = if r.slack_ticks >= 0 {
                "can defuse"
            } else {
                "too late"
            };
            let _ = writeln!(
                o,
                "| site {} | {} | {} | {} | {} | {} |",
                s.site, r.from, r.run_ticks, r.total_ticks, r.slack_ticks, verdict
            );
        }
    }
    let _ = writeln!(
        o,
        "\nBand: slack from the nearest other site {} ticks (unopposed run; fights eat into it).\n",
        band(b.retake_slack_ticks)
    );

    let _ = writeln!(o, "## Routes and chokepoints\n");
    let _ = writeln!(
        o,
        "| Site | Routes | Total width | Chokes (middle tile x width) |"
    );
    let _ = writeln!(o, "|---|---|---|---|");
    for s in &a.sites {
        let scope = format!("site {}", s.site);
        let list: Vec<String> = s
            .chokes
            .iter()
            .map(|c| format!("({},{}) x{}", c.at.0, c.at.1, c.width_tiles))
            .collect();
        let (routes, width) = if s.routes < 0 {
            ("open".to_string(), "-".to_string())
        } else {
            (s.routes.to_string(), s.cut_width.to_string())
        };
        let _ = writeln!(
            o,
            "| {} | {}{} | {}{} | {} |",
            s.site,
            routes,
            mark(a, "routes", &scope),
            width,
            mark(a, "cut_width", &scope),
            list.join(", ")
        );
    }
    let _ = writeln!(
        o,
        "\nMinimum tile cut between the start of the attack (the first {}% of the attackers' way to the site) and the \
         site; each cluster of cut tiles is one route. Bands: routes {}, total width {} tiles.\n",
        b.approach_start_pct, band(b.routes), band(b.cut_width)
    );

    let _ = writeln!(o, "## Sight\n");
    let _ = writeln!(o, "| Longest sightlines | From | To | Tiles |");
    let _ = writeln!(o, "|---|---|---|---|");
    for (k, s) in a.sightlines.iter().enumerate() {
        let note = if s.beyond_vision {
            " (beyond vision range)"
        } else {
            ""
        };
        let flag = if k == 0 {
            mark(a, "longest_sightline_tiles", "map")
        } else {
            String::new()
        };
        let _ = writeln!(
            o,
            "| {} | ({},{}) | ({},{}) | {}{}{} |",
            k + 1,
            s.from.0,
            s.from.1,
            s.to.0,
            s.to.1,
            tiles(s.length),
            flag,
            note
        );
    }
    let _ = writeln!(
        o,
        "\nBand: longest sightline {} tiles (vision range is {} tiles). Spawn-to-spawn sightline at round start: {}.\n",
        band(b.longest_sightline_tiles),
        tiles(a.rules.vision_range as i64),
        if a.spawn_sightline.is_some() { "**YES (fail)**" } else { "none" }
    );

    let _ = writeln!(o, "## Sites\n");
    let _ = writeln!(o, "| Site | Tiles | Hold positions | Best hold sees | Mean hold sees | Cover (wall + low) | Charge visible from |");
    let _ = writeln!(o, "|---|---|---|---|---|---|---|");
    for s in &a.sites {
        let _ = writeln!(o, "{}", site_row(a, s));
    }
    let _ = writeln!(
        o,
        "\nHold positions: off-site tiles {}..{} tiles from the plant point that a defender reaches at least {} ticks before \
         any attacker and that see part of the site. Cover: blocked tiles within {} tiles of the plant point. Bands: best \
         hold {}%, \
         mean hold {}%, cover {}%, charge visibility {}% of walkable tiles.\n",
        b.hold_radius_tiles[0], b.hold_radius_tiles[1], b.hold_lead_ticks, b.cover_radius_tiles,
        band(b.best_hold_share_pct), band(b.mean_hold_share_pct), band(b.cover_pct), band(b.postplant_visible_pct)
    );
    for s in &a.sites {
        let best: Vec<String> = s
            .best_holds
            .iter()
            .map(|h| format!("({},{}) {}%", h.at.0, h.at.1, h.share_pct))
            .collect();
        let far = s
            .postplant_farthest
            .map_or("-".to_string(), |p| format!("({},{})", p.0, p.1));
        let _ = writeln!(
            o,
            "- Site {}: best holds {}; farthest tile watching the charge {}.",
            s.site,
            best.join(", "),
            far
        );
    }

    let _ = writeln!(o, "\n## Flags\n");
    if a.flags.is_empty() {
        let _ = writeln!(o, "None: every measured value is inside its band.");
    } else {
        let _ = writeln!(o, "| Metric | Where | Value | Band | Verdict |");
        let _ = writeln!(o, "|---|---|---|---|---|");
        for f in &a.flags {
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} |",
                f.metric,
                f.scope,
                f.value,
                band(f.band),
                f.verdict
            );
        }
    }
    o
}

fn site_row(a: &Analysis, s: &SiteReport) -> String {
    let scope = format!("site {}", s.site);
    format!(
        "| {} | {} | {} | {}%{} | {}%{} | {}% ({} + {}){} | {} tiles, {}%{} |",
        s.site,
        s.tiles,
        s.hold_positions,
        s.best_hold_share_pct,
        mark(a, "best_hold_share_pct", &scope),
        s.mean_hold_share_pct,
        mark(a, "mean_hold_share_pct", &scope),
        s.cover_pct,
        s.cover_wall_pct,
        s.cover_low_pct,
        mark(a, "cover_pct", &scope),
        s.postplant_visible_tiles,
        s.postplant_visible_pct,
        mark(a, "postplant_visible_pct", &scope)
    )
}
