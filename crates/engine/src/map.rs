//! Map text format (`.ibcmap`): `key: value` header lines, a `---` line, then the tile grid.
//! `#` wall, `.` floor, `o` low cover (blocks movement only), `a` attacker spawn, `d` defender spawn,
//! `1`..`9` plant-site tiles (floor). Every row must have the same width and the border should be walls.

use crate::fixed::{self, V2};
use ibc_abi::{SiteInfo, TILE, TILE_FLOOR, TILE_LOW, TILE_WALL};

#[derive(Clone, Debug)]
pub struct Map {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub tiles: Vec<u8>,
    pub team_size: i32,
    pub sites: Vec<SiteInfo>,
    /// Spawn points (tile centres), index 0 = attackers, 1 = defenders.
    pub spawns: [Vec<V2>; 2],
    /// Which site (index + 1) each tile belongs to, 0 for none.
    pub site_of: Vec<u8>,
}

fn centre(tx: i32, ty: i32) -> V2 {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

/// What bots read about site `id` (0-based), whose tiles span the box `b` = (min_tx, min_ty, max_tx, max_ty),
/// max exclusive.
fn site_info(id: i32, b: (i32, i32, i32, i32), site_of: &[u8], width: i32) -> SiteInfo {
    let plant = plant_point(id, b, site_of, width);
    SiteInfo {
        id,
        min_tx: b.0,
        min_ty: b.1,
        max_tx: b.2,
        max_ty: b.3,
        centre_x: (b.0 + b.2) * TILE / 2,
        centre_y: (b.1 + b.3) * TILE / 2,
        plant_x: plant.0,
        plant_y: plant.1,
    }
}

/// The plant point of site `id`: the centre of its tile whose centre is nearest the box centre, so a unit
/// standing on it is on the site whatever the site's shape. Distances are compared in half tiles, which is
/// exact; ties go to the first tile in row-major order.
fn plant_point(id: i32, b: (i32, i32, i32, i32), site_of: &[u8], width: i32) -> V2 {
    let mut best = (i64::MAX, (b.0, b.1));
    for ty in b.1..b.3 {
        for tx in b.0..b.2 {
            if site_of[(ty * width + tx) as usize] as i32 != id + 1 {
                continue;
            }
            let dx = (2 * tx + 1 - b.0 - b.2) as i64;
            let dy = (2 * ty + 1 - b.1 - b.3) as i64;
            if dx * dx + dy * dy < best.0 {
                best = (dx * dx + dy * dy, (tx, ty));
            }
        }
    }
    centre(best.1 .0, best.1 .1)
}

impl Map {
    pub fn parse(text: &str) -> Result<Map, String> {
        let mut name = String::from("unnamed");
        let mut team_size = 0;
        let mut lines = text.lines();
        for line in lines.by_ref() {
            let line = line.trim_end();
            if line == "---" {
                break;
            }
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            let (k, v) = line.split_once(':').ok_or_else(|| format!("bad header line: {line}"))?;
            match k.trim() {
                "name" => name = v.trim().to_string(),
                "team_size" => team_size = v.trim().parse().map_err(|_| "bad team_size".to_string())?,
                _ => {}
            }
        }
        let rows: Vec<&str> = lines.map(|l| l.trim_end()).filter(|l| !l.is_empty()).collect();
        if rows.is_empty() {
            return Err("map has no grid (missing `---`?)".into());
        }
        let width = rows[0].len() as i32;
        let height = rows.len() as i32;
        if width > 128 || height > 128 {
            return Err("map larger than 128x128".into());
        }
        let mut tiles = vec![TILE_FLOOR; (width * height) as usize];
        let mut site_of = vec![0u8; (width * height) as usize];
        let mut spawns: [Vec<V2>; 2] = [Vec::new(), Vec::new()];
        let mut boxes: Vec<Option<(i32, i32, i32, i32)>> = vec![None; 9];
        for (ty, row) in rows.iter().enumerate() {
            if row.len() as i32 != width {
                return Err(format!("row {ty} has width {} but row 0 has {width}", row.len()));
            }
            for (tx, ch) in row.bytes().enumerate() {
                let (tx, ty) = (tx as i32, ty as i32);
                let i = (ty * width + tx) as usize;
                match ch {
                    b'#' => tiles[i] = TILE_WALL,
                    b'o' => tiles[i] = TILE_LOW,
                    b'.' => {}
                    b'a' => spawns[0].push(centre(tx, ty)),
                    b'd' => spawns[1].push(centre(tx, ty)),
                    b'1'..=b'9' => {
                        let s = (ch - b'1') as usize;
                        site_of[i] = s as u8 + 1;
                        let b = boxes[s].get_or_insert((tx, ty, tx + 1, ty + 1));
                        *b = (b.0.min(tx), b.1.min(ty), b.2.max(tx + 1), b.3.max(ty + 1));
                    }
                    _ => return Err(format!("unknown tile `{}` at {tx},{ty}", ch as char)),
                }
            }
        }
        let mut sites = Vec::new();
        for b in boxes.iter() {
            match *b {
                Some(b) => sites.push(site_info(sites.len() as i32, b, &site_of, width)),
                None => break,
            }
        }
        if sites.is_empty() {
            return Err("map has no plant site".into());
        }
        if spawns[0].is_empty() || spawns[1].is_empty() {
            return Err("map needs attacker (`a`) and defender (`d`) spawns".into());
        }
        let cap = spawns[0].len().min(spawns[1].len()) as i32;
        if team_size == 0 {
            team_size = cap.min(5);
        }
        if team_size > cap {
            return Err(format!("team_size {team_size} but only {cap} spawn tiles per side"));
        }
        Ok(Map { name, width, height, tiles, team_size, sites, spawns, site_of })
    }

    pub fn tile(&self, tx: i32, ty: i32) -> u8 {
        if tx < 0 || ty < 0 || tx >= self.width || ty >= self.height {
            TILE_WALL
        } else {
            self.tiles[(ty * self.width + tx) as usize]
        }
    }

    /// Site index (0-based) under a point, if any.
    pub fn site_at(&self, p: V2) -> Option<usize> {
        let (tx, ty) = (p.0.div_euclid(TILE), p.1.div_euclid(TILE));
        if tx < 0 || ty < 0 || tx >= self.width || ty >= self.height {
            return None;
        }
        match self.site_of[(ty * self.width + tx) as usize] {
            0 => None,
            s => Some(s as usize - 1),
        }
    }

    /// True if a circle of radius `r` at `p` overlaps a tile that blocks movement.
    pub fn circle_blocked(&self, p: V2, r: i32) -> bool {
        let (x0, x1) = ((p.0 - r).div_euclid(TILE), (p.0 + r).div_euclid(TILE));
        let (y0, y1) = ((p.1 - r).div_euclid(TILE), (p.1 + r).div_euclid(TILE));
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                if self.tile(tx, ty) == TILE_FLOOR {
                    continue;
                }
                // Closest point of the tile to the circle centre.
                let cx = p.0.clamp(tx * TILE, (tx + 1) * TILE);
                let cy = p.1.clamp(ty * TILE, (ty + 1) * TILE);
                if fixed::dist2(p, (cx, cy)) < (r as i64) * (r as i64) {
                    return true;
                }
            }
        }
        false
    }

    /// Walk the segment a->b through the grid (exact integer DDA). Returns the first point where it enters a
    /// wall tile, or `None` if the way is clear. Low cover does not stop sight or bullets. Each step is a
    /// comparison and a tile lookup; the entry point (two divisions) is worked out only for the wall the walk
    /// stops at.
    pub fn segment_wall(&self, a: V2, b: V2) -> Option<V2> {
        let (mut tx, mut ty) = (a.0.div_euclid(TILE), a.1.div_euclid(TILE));
        if self.tile(tx, ty) == TILE_WALL {
            return Some(a);
        }
        let (ex, ey) = (b.0.div_euclid(TILE), b.1.div_euclid(TILE));
        let dx = (b.0 - a.0) as i64;
        let dy = (b.1 - a.1) as i64;
        let (sx, sy) = (dx.signum() as i32, dy.signum() as i32);
        let (adx, ady) = (dx.abs(), dy.abs());
        // Distance along each axis to the next tile boundary.
        let mut nx = if sx > 0 { ((tx + 1) * TILE - a.0) as i64 } else { (a.0 - tx * TILE) as i64 };
        let mut ny = if sy > 0 { ((ty + 1) * TILE - a.1) as i64 } else { (a.1 - ty * TILE) as i64 };
        let mut guard = 0;
        while (tx, ty) != (ex, ey) && guard < 512 {
            guard += 1;
            // Compare nx/adx with ny/ady without dividing. An axis that does not move never wins.
            let step_x = if adx == 0 {
                false
            } else if ady == 0 {
                true
            } else {
                nx * ady <= ny * adx
            };
            if step_x {
                tx += sx;
                if self.tile(tx, ty) == TILE_WALL {
                    return Some((a.0 + (sx as i64 * nx) as i32, a.1 + (dy * nx / adx) as i32));
                }
                nx += TILE as i64;
            } else {
                ty += sy;
                if self.tile(tx, ty) == TILE_WALL {
                    return Some((a.0 + (dx * ny / ady) as i32, a.1 + (sy as i64 * ny) as i32));
                }
                ny += TILE as i64;
            }
        }
        None
    }

    pub fn hash(&self) -> u64 {
        let mut h = crate::Fnv::new();
        h.i32(self.width);
        h.i32(self.height);
        h.i32(self.team_size);
        h.bytes(&self.tiles);
        h.bytes(&self.site_of);
        for s in &self.spawns {
            for p in s {
                h.i32(p.0);
                h.i32(p.1);
            }
            h.i32(-1);
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: &str = "name: t\nteam_size: 1\n---\n#######\n#a.#.d#\n#..#.1#\n#.....#\n#######\n";

    #[test]
    fn parse_and_los() {
        let m = Map::parse(M).unwrap();
        assert_eq!((m.width, m.height, m.sites.len()), (7, 5, 1));
        let c = |x, y| centre(x, y);
        assert!(m.segment_wall(c(1, 1), c(4, 1)).is_some());
        assert!(m.segment_wall(c(1, 3), c(5, 3)).is_none());
        assert!(m.segment_wall(c(2, 2), c(4, 2)).is_some());
        assert!(m.segment_wall(c(1, 1), c(1, 3)).is_none());
        assert_eq!(m.site_at(c(5, 2)), Some(0));
        assert!(m.circle_blocked((TILE + 100, TILE + 512), 300));
        assert!(!m.circle_blocked(c(1, 1), 300));
    }

    #[test]
    fn plant_point_of_a_rectangle() {
        // 3x3: the box centre is the middle tile's centre, and that is the plant point.
        let m = Map::parse("name: r\n---\n#######\n#a111d#\n#.111.#\n#.111.#\n#######\n").unwrap();
        let s = m.sites[0];
        assert_eq!((s.plant_x, s.plant_y), (s.centre_x, s.centre_y));
        assert_eq!((s.plant_x, s.plant_y), centre(3, 2));
        // 3x2: the box centre lies on the edge between rows 1 and 2; the tie goes to the upper row.
        let m = Map::parse("name: r\n---\n#######\n#a111d#\n#.111.#\n#######\n").unwrap();
        let s = m.sites[0];
        assert_eq!((s.centre_x, s.centre_y), (3 * TILE + TILE / 2, 2 * TILE));
        assert_eq!((s.plant_x, s.plant_y), centre(3, 1));
    }
}
