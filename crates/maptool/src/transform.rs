//! Whole-map edits on the text grid: flips, rotations, side swap, mirroring a half onto the other,
//! and a scaffold for new maps.

use crate::raw::RawMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    FlipH,
    FlipV,
    Transpose,
    Rot90,
    Rot180,
    Rot270,
    SwapSides,
}

impl Op {
    pub const NAMES: &'static str = "flip-h, flip-v, transpose, rot90, rot180, rot270, swap-sides";

    pub fn parse(s: &str) -> Result<Op, String> {
        Ok(match s {
            "flip-h" => Op::FlipH,
            "flip-v" => Op::FlipV,
            "transpose" => Op::Transpose,
            "rot90" => Op::Rot90,
            "rot180" => Op::Rot180,
            "rot270" => Op::Rot270,
            "swap-sides" => Op::SwapSides,
            other => return Err(format!("unknown transform `{other}` (known: {})", Op::NAMES)),
        })
    }
}

fn check_rect(raw: &RawMap) -> Result<(), String> {
    let w = raw.width();
    match raw.rows.iter().position(|r| r.len() != w) {
        Some(y) => Err(format!("row {y} is {} wide but row 0 is {w}; fix the grid first", raw.rows[y].len())),
        None => Ok(()),
    }
}

fn swap_side(c: u8) -> u8 {
    match c {
        b'a' => b'd',
        b'd' => b'a',
        c => c,
    }
}

fn transpose(rows: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let w = rows.first().map_or(0, |r| r.len());
    (0..w).map(|x| rows.iter().map(|r| r[x]).collect()).collect()
}

fn flip_h(rows: &mut [Vec<u8>]) {
    rows.iter_mut().for_each(|r| r.reverse());
}

/// Apply one transform. Rotations are clockwise on screen. Spawn order (which tiles a team of
/// `team_size` uses) follows the new row-major order.
pub fn apply(raw: &RawMap, op: Op) -> Result<RawMap, String> {
    check_rect(raw)?;
    let mut out = raw.clone();
    match op {
        Op::FlipH => flip_h(&mut out.rows),
        Op::FlipV => out.rows.reverse(),
        Op::Transpose => out.rows = transpose(&raw.rows),
        Op::Rot90 => {
            out.rows.reverse();
            out.rows = transpose(&out.rows);
        }
        Op::Rot180 => {
            out.rows.reverse();
            flip_h(&mut out.rows);
        }
        Op::Rot270 => {
            out.rows = transpose(&raw.rows);
            out.rows.reverse();
        }
        Op::SwapSides => out.rows.iter_mut().flatten().for_each(|c| *c = swap_side(*c)),
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Left half copied onto the right half.
    LeftRight,
    /// Top half copied onto the bottom half.
    TopBottom,
    /// Top half rotated 180 degrees onto the bottom half.
    Point,
}

impl Axis {
    pub fn parse(s: &str) -> Result<Axis, String> {
        Ok(match s {
            "h" | "left-right" => Axis::LeftRight,
            "v" | "top-bottom" => Axis::TopBottom,
            "point" => Axis::Point,
            other => return Err(format!("unknown mirror axis `{other}` (known: h, v, point)")),
        })
    }
}

/// Overwrite one half of the map with the mirror image of the other. In the copied half, site
/// digits are shifted past the source half's highest (so a mirrored site 1 becomes a new site), and
/// with `swap_sides` attacker and defender spawns trade places. A middle row/column is left alone.
pub fn mirror(raw: &RawMap, axis: Axis, swap_sides: bool) -> Result<RawMap, String> {
    check_rect(raw)?;
    let (w, h) = (raw.width(), raw.height());
    let in_source = |x: usize, y: usize| match axis {
        Axis::LeftRight => x < w / 2,
        Axis::TopBottom | Axis::Point => y < h / 2,
    };
    let highest = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| in_source(x, y))
        .filter_map(|(x, y)| raw.rows[y][x].is_ascii_digit().then(|| raw.rows[y][x] - b'0'))
        .max()
        .unwrap_or(0);
    let mut out = raw.clone();
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = match axis {
                Axis::LeftRight => (w - 1 - x, y),
                Axis::TopBottom => (x, h - 1 - y),
                Axis::Point => (w - 1 - x, h - 1 - y),
            };
            if in_source(x, y) || !in_source(sx, sy) {
                continue;
            }
            let mut c = raw.rows[sy][sx];
            if c.is_ascii_digit() {
                let shifted = c - b'0' + highest;
                if shifted > 9 {
                    return Err(format!("mirroring would create site {shifted}; the format allows 1..9"));
                }
                c = b'0' + shifted;
            }
            out.rows[y][x] = if swap_sides { swap_side(c) } else { c };
        }
    }
    Ok(out)
}

/// A bordered, lint-clean starting point: spawns at the left and right edges hidden from each other
/// by a centre wall, and two sites on the defenders' side.
pub fn scaffold(width: usize, height: usize, team_size: usize, name: &str) -> Result<RawMap, String> {
    if !(16..=128).contains(&width) || !(12..=128).contains(&height) {
        return Err(format!("size {width}x{height} is outside 16x12 .. 128x128"));
    }
    if !(1..=5).contains(&team_size) {
        return Err(format!("team size {team_size} is outside 1..5"));
    }
    let mut rows = vec![vec![b'.'; width]; height];
    for (y, row) in rows.iter_mut().enumerate() {
        for (x, c) in row.iter_mut().enumerate() {
            if x == 0 || y == 0 || x == width - 1 || y == height - 1 {
                *c = b'#';
            }
        }
    }
    let mid = height / 2;
    // Spawns: a 2-wide block of 3 rows around the middle (6 tiles, enough for any team size).
    for y in mid - 1..=mid + 1 {
        for x in 0..2 {
            rows[y][1 + x] = b'a';
            rows[y][width - 2 - x] = b'd';
        }
    }
    for row in rows.iter_mut().take(mid + 4).skip(mid - 3) {
        row[width / 2] = b'#';
    }
    for (digit, top) in [(b'1', 2), (b'2', height - 5)] {
        for row in rows.iter_mut().skip(top).take(3) {
            for c in row.iter_mut().skip(width - 8).take(3) {
                *c = digit;
            }
        }
    }
    let header = vec![format!("name: {name}"), format!("team_size: {team_size}")];
    Ok(RawMap { header, rows })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::{fails, lint};
    use crate::Ruleset;

    fn raw(grid: &[&str]) -> RawMap {
        RawMap { header: vec!["name: t".into()], rows: grid.iter().map(|r| r.as_bytes().to_vec()).collect() }
    }

    fn grid(raw: &RawMap) -> Vec<String> {
        raw.rows.iter().map(|r| String::from_utf8(r.clone()).unwrap()).collect()
    }

    #[test]
    fn rotations_compose() {
        let m = raw(&["#a.", "..1", "d.o", "###"]);
        assert_eq!(grid(&apply(&m, Op::Rot90).unwrap()), ["#d.#", "#..a", "#o1."]);
        let twice = apply(&apply(&m, Op::Rot90).unwrap(), Op::Rot90).unwrap();
        assert_eq!(twice, apply(&m, Op::Rot180).unwrap());
        let back = apply(&apply(&m, Op::Rot90).unwrap(), Op::Rot270).unwrap();
        assert_eq!(back, m);
        assert_eq!(grid(&apply(&m, Op::FlipH).unwrap())[0], ".a#");
        assert_eq!(grid(&apply(&m, Op::FlipV).unwrap())[0], "###");
        assert_eq!(grid(&apply(&m, Op::SwapSides).unwrap()), ["#d.", "..1", "a.o", "###"]);
        assert_eq!(apply(&apply(&m, Op::Transpose).unwrap(), Op::Transpose).unwrap(), m);
    }

    #[test]
    fn mirror_copies_half_and_renumbers_sites() {
        let m = raw(&["#a1..", "#.o..", "#...."]);
        assert_eq!(grid(&mirror(&m, Axis::LeftRight, true).unwrap()), ["#a1d#", "#.o.#", "#...#"]);
        let m = raw(&["a1.", "...", "...", "..."]);
        assert_eq!(grid(&mirror(&m, Axis::Point, true).unwrap()), ["a1.", "...", "...", ".2d"]);
        assert_eq!(grid(&mirror(&m, Axis::TopBottom, false).unwrap()), ["a1.", "...", "...", "a2."]);
    }

    #[test]
    fn scaffold_passes_lint() {
        let rules = Ruleset::builtin();
        for (w, h, n) in [(16, 12, 3), (40, 28, 5), (33, 21, 4), (128, 128, 5)] {
            let text = scaffold(w, h, n, "fresh").unwrap().to_text();
            let issues = lint(&text, Some("fresh"), &rules);
            assert!(!fails(&issues, true), "{w}x{h}: {issues:?}");
        }
        assert!(scaffold(8, 8, 5, "x").is_err());
    }
}
