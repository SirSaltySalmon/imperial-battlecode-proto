//! Pictures of a map: SVG for docs and review, ASCII for terminals. Overlays draw `analyze` results
//! on top (routes, chokepoints, sightlines, time-to-reach heatmaps).

use crate::analyze::Analysis;
use crate::path::UNREACHABLE;
use crate::{ticks_for, tile_of, Map, TILE, TILE_LOW, TILE_WALL};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// Fastest attacker and defender route to each site.
    Routes,
    /// Chokepoint tiles from the route min-cut.
    Chokes,
    /// The longest sightlines.
    Sightlines,
    /// Heatmap of attacker / defender ticks to reach each tile.
    TimeAttack,
    TimeDefend,
    /// Who reaches each tile first, and by how much.
    Advantage,
}

impl Overlay {
    pub const NAMES: &'static str = "routes, chokes, sightlines, time-a, time-d, advantage";

    pub fn parse_list(list: &str) -> Result<Vec<Overlay>, String> {
        list.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| match s {
                "routes" => Ok(Overlay::Routes),
                "chokes" => Ok(Overlay::Chokes),
                "sightlines" => Ok(Overlay::Sightlines),
                "time-a" => Ok(Overlay::TimeAttack),
                "time-d" => Ok(Overlay::TimeDefend),
                "advantage" => Ok(Overlay::Advantage),
                other => Err(format!(
                    "unknown overlay `{other}` (known: {})",
                    Overlay::NAMES
                )),
            })
            .collect()
    }
}

const CELL: i32 = 16;
const ATTACK: &str = "#d9480f";
const DEFEND: &str = "#1c6dd0";

/// World units to SVG pixels.
fn px(v: i32) -> f64 {
    v as f64 * CELL as f64 / TILE as f64
}

fn mix(from: (u8, u8, u8), to: (u8, u8, u8), t: f64) -> String {
    let t = t.clamp(0.0, 1.0);
    let ch = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        ch(from.0, to.0),
        ch(from.1, to.1),
        ch(from.2, to.2)
    )
}

/// Ticks for a side to reach tile `i`, if it can.
fn tile_ticks(a: &Analysis, side: usize, i: usize) -> Option<i64> {
    let d = a.tile_dist[side][i];
    (d < UNREACHABLE).then(|| ticks_for(d, a.rules.run_speed))
}

fn heat_colour(a: &Analysis, overlays: &[Overlay], i: usize) -> Option<String> {
    const PALE: (u8, u8, u8) = (0xf4, 0xf1, 0xea);
    let side_heat = |side: usize, deep: (u8, u8, u8)| {
        let max = (0..a.tile_dist[side].len())
            .filter_map(|j| tile_ticks(a, side, j))
            .max()
            .unwrap_or(1)
            .max(1);
        tile_ticks(a, side, i).map(|t| mix(PALE, deep, t as f64 / max as f64))
    };
    if overlays.contains(&Overlay::Advantage) {
        let (ta, td) = (tile_ticks(a, 0, i)?, tile_ticks(a, 1, i)?);
        // Positive: defenders first (blue). Negative: attackers first (orange). Saturates at 20 ticks.
        let lead = (ta - td) as f64 / 20.0;
        return Some(if lead >= 0.0 {
            mix(PALE, (0x4c, 0x8f, 0xe0), lead)
        } else {
            mix(PALE, (0xf0, 0x8c, 0x3c), -lead)
        });
    }
    if overlays.contains(&Overlay::TimeAttack) {
        return side_heat(0, (0xe0, 0x5a, 0x1a));
    }
    if overlays.contains(&Overlay::TimeDefend) {
        return side_heat(1, (0x2a, 0x6f, 0xd6));
    }
    None
}

/// Render the map as a standalone SVG document. Overlays need the analysis.
pub fn svg(map: &Map, analysis: Option<&Analysis>, overlays: &[Overlay]) -> String {
    let (w, h) = (map.width * CELL, map.height * CELL);
    let mut o = String::new();
    let _ = writeln!(
        o,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {}" width="{w}" height="{}" font-family="monospace">"##,
        h + 18,
        h + 18
    );
    let _ = writeln!(
        o,
        r##"<rect width="{w}" height="{}" fill="#ffffff"/>"##,
        h + 18
    );
    for ty in 0..map.height {
        for tx in 0..map.width {
            let i = (ty * map.width + tx) as usize;
            let (x, y) = (tx * CELL, ty * CELL);
            let tile = map.tiles[i];
            let site = map.site_of[i];
            let fill = if tile == TILE_WALL {
                "#2b3038".to_string()
            } else if let Some(c) = analysis.and_then(|a| heat_colour(a, overlays, i)) {
                c
            } else if site > 0 {
                "#f7dc9c".to_string()
            } else {
                "#f4f1ea".to_string()
            };
            let _ = writeln!(
                o,
                r##"<rect x="{x}" y="{y}" width="{CELL}" height="{CELL}" fill="{fill}" stroke="#d9d4c7" stroke-width="0.5"/>"##
            );
            if tile == TILE_LOW {
                let _ = writeln!(
                    o,
                    r##"<rect x="{}" y="{}" width="{}" height="{}" rx="3" fill="#a9824a" stroke="#6e5026" stroke-width="1"/>"##,
                    x + 2,
                    y + 2,
                    CELL - 4,
                    CELL - 4
                );
            }
            if site > 0 && tile != TILE_WALL {
                let _ = writeln!(
                    o,
                    r##"<rect x="{x}" y="{y}" width="{CELL}" height="{CELL}" fill="none" stroke="#c98a00" stroke-width="1"/>"##
                );
            }
        }
    }
    for (side, colour, label) in [(0usize, ATTACK, "A"), (1, DEFEND, "D")] {
        for (k, &p) in map.spawns[side].iter().enumerate() {
            let used = (k as i32) < map.team_size;
            let _ = writeln!(
                o,
                r##"<circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{colour}" fill-opacity="{}"/>"##,
                px(p.0),
                px(p.1),
                px(300),
                if used { "0.9" } else { "0.35" }
            );
            if k == 0 {
                let _ = writeln!(
                    o,
                    r##"<text x="{:.1}" y="{:.1}" font-size="11" font-weight="bold" fill="{colour}">{label}</text>"##,
                    px(p.0) - 4.0,
                    px(p.1) - 9.0
                );
            }
        }
    }
    for s in &map.sites {
        let _ = writeln!(
            o,
            r##"<text x="{:.1}" y="{:.1}" font-size="13" font-weight="bold" text-anchor="middle" fill="#7a5200">{}</text>"##,
            px(s.centre_x),
            px(s.centre_y) + 4.5,
            s.id + 1
        );
    }
    if let Some(a) = analysis {
        overlay_shapes(&mut o, a, overlays);
    }
    let _ = writeln!(
        o,
        r##"<text x="2" y="{}" font-size="11" fill="#333">{} {}x{} {}v{}  A=attackers D=defenders  brown=low cover</text>"##,
        h + 13,
        map.name,
        map.width,
        map.height,
        map.team_size,
        map.team_size
    );
    o.push_str("</svg>\n");
    o
}

fn overlay_shapes(o: &mut String, a: &Analysis, overlays: &[Overlay]) {
    if overlays.contains(&Overlay::Chokes) {
        for s in &a.sites {
            for (tx, ty) in s.chokes.iter().flat_map(|c| &c.tiles) {
                let _ = writeln!(
                    o,
                    r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#b5179e" fill-opacity="0.3" stroke="#b5179e" stroke-width="1"/>"##,
                    tx * CELL + 1,
                    ty * CELL + 1,
                    CELL - 2,
                    CELL - 2
                );
            }
        }
    }
    if overlays.contains(&Overlay::Sightlines) {
        for s in &a.sightlines {
            let c = |t: i32| (t * CELL + CELL / 2) as f64;
            let _ = writeln!(
                o,
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="#2b9348" stroke-width="1.5" stroke-dasharray="5 3"/>"##,
                c(s.from.0),
                c(s.from.1),
                c(s.to.0),
                c(s.to.1)
            );
        }
    }
    if overlays.contains(&Overlay::Routes) {
        for s in &a.sites {
            for (path, colour) in [(&s.attacker_path, ATTACK), (&s.defender_path, DEFEND)] {
                let pts: Vec<String> = path
                    .iter()
                    .map(|p| format!("{:.1},{:.1}", px(p.0), px(p.1)))
                    .collect();
                let _ = writeln!(
                    o,
                    r##"<polyline points="{}" fill="none" stroke="{colour}" stroke-width="2" stroke-opacity="0.8" stroke-linejoin="round"/>"##,
                    pts.join(" ")
                );
            }
        }
    }
}

/// The grid with rulers. With a time overlay, floor tiles show ticks/4 as 0-9A-Z; with `advantage`,
/// `+` where defenders arrive first, `-` where attackers do, `=` within a tick.
pub fn ascii(map: &Map, analysis: Option<&Analysis>, overlays: &[Overlay]) -> String {
    let spawn_at =
        |side: usize, tx: i32, ty: i32| map.spawns[side].iter().any(|&p| tile_of(p) == (tx, ty));
    let mut o = String::new();
    for row in [|x: i32| x / 10 % 10, |x: i32| x % 10] {
        o.push_str("    ");
        for tx in 0..map.width {
            let _ = write!(o, "{}", row(tx));
        }
        o.push('\n');
    }
    for ty in 0..map.height {
        let _ = write!(o, "{ty:>3} ");
        for tx in 0..map.width {
            let i = (ty * map.width + tx) as usize;
            let base = match (map.tiles[i], map.site_of[i]) {
                (TILE_WALL, _) => '#',
                (TILE_LOW, _) => 'o',
                (_, s) if s > 0 => (b'0' + s) as char,
                _ if spawn_at(0, tx, ty) => 'a',
                _ if spawn_at(1, tx, ty) => 'd',
                _ => '.',
            };
            let shown = match analysis {
                Some(a) if base == '.' => overlay_char(a, overlays, i).unwrap_or(base),
                _ => base,
            };
            o.push(shown);
        }
        o.push('\n');
    }
    o
}

fn overlay_char(a: &Analysis, overlays: &[Overlay], i: usize) -> Option<char> {
    let scale =
        |t: i64| char::from_digit((t / 4).clamp(0, 35) as u32, 36).map(|c| c.to_ascii_uppercase());
    if overlays.contains(&Overlay::Advantage) {
        let lead = tile_ticks(a, 0, i)? - tile_ticks(a, 1, i)?;
        return Some(if lead > 1 {
            '+'
        } else if lead < -1 {
            '-'
        } else {
            '='
        });
    }
    if overlays.contains(&Overlay::TimeAttack) {
        return scale(tile_ticks(a, 0, i)?);
    }
    if overlays.contains(&Overlay::TimeDefend) {
        return scale(tile_ticks(a, 1, i)?);
    }
    None
}
