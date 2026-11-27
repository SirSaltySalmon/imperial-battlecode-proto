//! Structural checks: is this file a playable map at all? (Whether it is a *good* map is `analyze`.)

use crate::raw::{is_tile_char, RawMap};
use crate::sight::spawn_sightline;
use crate::tilegraph::TileGraph;
use crate::{tile_of, used_spawns, Map, Ruleset, TILE};

/// Largest map the engine accepts.
pub const MAX_SIDE: usize = 128;
/// A site smaller than this has no room for a carrier plus anyone guarding it.
pub const MIN_SITE_TILES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    /// Stable identifier of the check, e.g. `open-border`.
    pub code: &'static str,
    pub message: String,
}

fn error(code: &'static str, message: String) -> Issue {
    Issue {
        severity: Severity::Error,
        code,
        message,
    }
}

fn warning(code: &'static str, message: String) -> Issue {
    Issue {
        severity: Severity::Warning,
        code,
        message,
    }
}

/// Run every check on a map file's text. `file_stem` (if known) is compared with the `name` header.
pub fn lint(text: &str, file_stem: Option<&str>, rules: &Ruleset) -> Vec<Issue> {
    let mut issues = Vec::new();
    let raw = match RawMap::parse(text) {
        Ok(raw) => raw,
        Err(e) => return vec![error("no-grid", e)],
    };
    check_text(&raw, file_stem, &mut issues);
    if issues.iter().any(|i| i.severity == Severity::Error) {
        return issues; // the grid is not well formed; the engine would reject or misread it
    }
    match Map::parse(text) {
        Ok(map) => check_map(&map, rules, &mut issues),
        Err(e) => issues.push(error(
            "engine-parse",
            format!("the engine rejects this map: {e}"),
        )),
    }
    issues
}

/// Checks on the raw text: shape, characters, border, header.
fn check_text(raw: &RawMap, file_stem: Option<&str>, issues: &mut Vec<Issue>) {
    let (w, h) = (raw.width(), raw.height());
    for (y, row) in raw.rows.iter().enumerate() {
        if row.len() != w {
            issues.push(error(
                "ragged-row",
                format!("row {y} is {} wide but row 0 is {w}", row.len()),
            ));
        }
        if let Some(x) = row.iter().position(|&c| !is_tile_char(c)) {
            issues.push(error(
                "bad-tile",
                format!("unknown tile `{}` at ({x},{y})", row[x] as char),
            ));
        }
    }
    if w > MAX_SIDE || h > MAX_SIDE {
        issues.push(error(
            "too-large",
            format!("map is {w}x{h}; the limit is {MAX_SIDE}x{MAX_SIDE}"),
        ));
    }
    if w < 3 || h < 3 {
        issues.push(error(
            "too-small",
            format!("map is {w}x{h}; it needs a border and at least one floor tile"),
        ));
    }
    if issues.iter().any(|i| i.severity == Severity::Error) {
        return;
    }
    let open: Vec<(usize, usize)> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| (x == 0 || y == 0 || x == w - 1 || y == h - 1) && raw.rows[y][x] != b'#')
        .collect();
    if let Some(&(x, y)) = open.first() {
        issues.push(error(
            "open-border",
            format!(
                "border is not closed: `{}` at ({x},{y}), {} border tile(s) are not `#`",
                raw.rows[y][x] as char,
                open.len()
            ),
        ));
    }
    // Site digits must be 1..n without holes: the engine stops at the first missing number.
    let present: Vec<bool> = (b'1'..=b'9')
        .map(|d| raw.rows.iter().any(|r| r.contains(&d)))
        .collect();
    let count = present.iter().take_while(|&&p| p).count();
    if let Some(skipped) = present.iter().skip(count).position(|&p| p) {
        issues.push(error(
            "site-numbering",
            format!(
                "site `{}` exists but site `{}` does not; sites must be numbered 1..n",
                count + skipped + 1,
                count + 1
            ),
        ));
    }
    match raw.get("name") {
        None => issues.push(warning(
            "no-name",
            "header has no `name:`; the engine will call the map `unnamed`".into(),
        )),
        Some(name) => {
            if file_stem.is_some_and(|stem| stem != name) {
                issues.push(warning(
                    "name-mismatch",
                    format!(
                        "`name: {name}` differs from the file name `{}`",
                        file_stem.unwrap_or("")
                    ),
                ));
            }
        }
    }
    match raw.get("team_size").map(|v| v.parse::<i32>()) {
        None => issues.push(warning(
            "no-team-size",
            "header has no `team_size:`; the engine will guess it from the spawn tiles".into(),
        )),
        Some(Ok(n)) if (3..=5).contains(&n) => {}
        Some(Ok(n)) => issues.push(warning(
            "team-size",
            format!("team_size {n} is outside the 3..5 the game is designed for"),
        )),
        Some(Err(_)) => issues.push(error("team-size", "team_size is not a number".into())),
    }
}

/// Checks on the parsed map: spawns, reachability, sites.
fn check_map(map: &Map, rules: &Ruleset, issues: &mut Vec<Issue>) {
    let r = rules.core.unit_radius;
    let g = TileGraph::build(map, r);
    let floor: Vec<usize> = (0..g.stand.len())
        .filter(|&i| map.tiles[i] == crate::TILE_FLOOR)
        .collect();

    for (side, label) in [(0usize, "attacker (`a`)"), (1, "defender (`d`)")] {
        let have = map.spawns[side].len() as i32;
        if have < map.team_size {
            issues.push(error(
                "few-spawns",
                format!(
                    "{have} {label} spawn tile(s) for team_size {}",
                    map.team_size
                ),
            ));
        }
    }

    // Floor a unit cannot stand on at all (only with unit_radius over half a tile).
    let cramped: Vec<usize> = floor.iter().copied().filter(|&i| !g.stand[i]).collect();
    if let Some(&i) = cramped.first() {
        let (x, y) = g.coords(i);
        issues.push(error(
            "narrow-gap",
            format!("{} floor tile(s) are too narrow for a unit of radius {r} to stand in, first at ({x},{y})", cramped.len()),
        ));
    }
    // Gaps that look open on the grid but no circle passes: diagonal pinches between two blocked tiles.
    for y in 0..map.height - 1 {
        for x in 0..map.width - 1 {
            let f = |dx: i32, dy: i32| g.stand[g.index(x + dx, y + dy)];
            for (a, b, c, d) in [
                ((0, 0), (1, 1), (1, 0), (0, 1)),
                ((1, 0), (0, 1), (0, 0), (1, 1)),
            ] {
                if f(a.0, a.1) && f(b.0, b.1) && !f(c.0, c.1) && !f(d.0, d.1) {
                    issues.push(warning(
                        "diagonal-pinch",
                        format!(
                            "({},{}) and ({},{}) touch only at a corner: units cannot pass, but bullets and sight can graze through",
                            x + a.0, y + a.1, x + b.0, y + b.1
                        ),
                    ));
                }
            }
        }
    }

    let spawn_tiles = |side: usize| -> Vec<usize> {
        map.spawns[side]
            .iter()
            .map(|&p| g.index(tile_of(p).0, tile_of(p).1))
            .collect()
    };
    let (a_tiles, d_tiles) = (spawn_tiles(0), spawn_tiles(1));
    let Some(&origin) = a_tiles.first() else {
        return;
    };
    let reach = g.flood(&[origin]);
    for (tiles, label) in [(&a_tiles, "attacker"), (&d_tiles, "defender")] {
        let cut_off: Vec<usize> = tiles.iter().copied().filter(|&i| !reach[i]).collect();
        if let Some(&i) = cut_off.first() {
            let (x, y) = g.coords(i);
            issues.push(error(
                "spawn-unreachable",
                format!("{} {label} spawn tile(s) cannot be reached from the first attacker spawn, first at ({x},{y})", cut_off.len()),
            ));
        }
    }
    let pockets: Vec<usize> = floor
        .iter()
        .copied()
        .filter(|&i| g.stand[i] && !reach[i])
        .collect();
    if let Some(&i) = pockets.first() {
        let (x, y) = g.coords(i);
        issues.push(error(
            "pocket",
            format!("{} floor tile(s) are unreachable from the attacker spawn (walk around unit radius {r}), first at ({x},{y})", pockets.len()),
        ));
    }

    for site in &map.sites {
        check_site(map, &g, &reach, site, issues);
        check_blast_escape(map, &g, site, rules.core.blast_radius, issues);
    }

    for (side, label) in [(0usize, "attacker"), (1, "defender")] {
        if let Some(p) = used_spawns(map, side)
            .iter()
            .find(|&&p| map.site_at(p).is_some())
        {
            let (x, y) = tile_of(*p);
            issues.push(warning(
                "spawn-on-site",
                format!("{label} spawn at ({x},{y}) is on a plant site"),
            ));
        }
    }
    if let Some((a, d)) = spawn_sightline(map) {
        let ((ax, ay), (dx, dy)) = (tile_of(a), tile_of(d));
        issues.push(error(
            "spawn-sightline",
            format!("attacker spawn ({ax},{ay}) and defender spawn ({dx},{dy}) see each other at round start"),
        ));
    }
}

fn check_site(
    map: &Map,
    g: &TileGraph,
    reach: &[bool],
    site: &crate::SiteInfo,
    issues: &mut Vec<Issue>,
) {
    let label = site.id + 1;
    let tiles: Vec<usize> = (0..map.site_of.len())
        .filter(|&i| map.site_of[i] as i32 == label)
        .collect();
    if tiles.len() < MIN_SITE_TILES {
        issues.push(error(
            "site-small",
            format!("site {label} has {} tile(s); it needs at least {MIN_SITE_TILES} to stand and plant on", tiles.len()),
        ));
    }
    if !tiles.iter().any(|&i| reach[i]) {
        issues.push(error(
            "site-unreachable",
            format!("site {label} cannot be reached from the spawns"),
        ));
    }
    // Site tiles should form one region.
    let own = |i: usize| map.site_of[i] as i32 == label;
    let mut seen = vec![false; g.stand.len()];
    let mut stack = vec![tiles[0]];
    seen[tiles[0]] = true;
    while let Some(i) = stack.pop() {
        for j in g.neighbours(i) {
            if own(j) && !seen[j] {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    if tiles.iter().any(|&i| !seen[i]) {
        issues.push(warning(
            "site-split",
            format!("site {label} is not one connected region"),
        ));
    }
    // Where bots plant needs no check of its own: the plant point is the centre of a site tile whatever the
    // site's shape, and `narrow-gap` and `pocket` already report tile centres a unit cannot stand on or reach.
}

/// From every tile a charge can be planted on, a unit must be able to walk to a tile whose centre is outside
/// the detonation blast wherever on the plant tile the charge lies. Otherwise a lost retake also costs every
/// defender (and attacker) still near the site their gun.
fn check_blast_escape(
    map: &Map,
    g: &TileGraph,
    site: &crate::SiteInfo,
    radius: i32,
    issues: &mut Vec<Issue>,
) {
    let label = site.id + 1;
    let r2 = (radius.max(0) as i64).pow(2);
    // Closest a charge anywhere on tile `p` gets to the centre of tile `q`.
    let clear = |p: usize, q: usize| {
        let ((px, py), (qx, qy)) = (g.coords(p), g.coords(q));
        let gap = |a: i32, b: i32| ((a - b).abs() as i64 * TILE as i64 - TILE as i64 / 2).max(0);
        gap(px, qx).pow(2) + gap(py, qy).pow(2) > r2
    };
    let plantable: Vec<usize> = (0..map.site_of.len())
        .filter(|&i| map.site_of[i] as i32 == label && g.stand[i])
        .collect();
    let mut flood: Option<Vec<bool>> = None;
    let mut trapped = Vec::new();
    for &p in &plantable {
        if !flood.as_ref().is_some_and(|f| f[p]) {
            flood = Some(g.flood(&[p]));
        }
        let reach = flood.as_ref().expect("flooded above");
        if !(0..reach.len()).any(|q| reach[q] && clear(p, q)) {
            trapped.push(p);
        }
    }
    if let Some(&p) = trapped.first() {
        let (x, y) = g.coords(p);
        issues.push(error(
            "blast-escape",
            format!(
                "site {label}: from {} plant tile(s), first ({x},{y}), no reachable tile is outside the {:.1}-tile blast radius",
                trapped.len(),
                radius as f64 / TILE as f64
            ),
        ));
    }
}

/// True if any issue is an error (or a warning, when `strict`).
pub fn fails(issues: &[Issue], strict: bool) -> bool {
    issues
        .iter()
        .any(|i| strict || i.severity == Severity::Error)
}
