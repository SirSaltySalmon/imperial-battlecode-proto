//! Exact integer predicates for a circle moving through the tile grid.
//!
//! The engine's `Map::circle_blocked` says a circle is blocked when a movement-blocking tile comes
//! strictly closer than its radius. `swept_free` is the same test for every point of a segment at
//! once: the segment must keep at least `r` away from every blocking tile.

use crate::{Map, TILE, TILE_FLOOR, V2};

/// Axis-aligned rectangle `[x0, x1] x [y0, y1]` in world units.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn tile(tx: i32, ty: i32) -> Rect {
        Rect {
            x0: tx * TILE,
            y0: ty * TILE,
            x1: (tx + 1) * TILE,
            y1: (ty + 1) * TILE,
        }
    }

    fn corners(&self) -> [V2; 4] {
        [
            (self.x0, self.y0),
            (self.x1, self.y0),
            (self.x1, self.y1),
            (self.x0, self.y1),
        ]
    }
}

fn cross(a: V2, b: V2, p: V2) -> i64 {
    (b.0 - a.0) as i64 * (p.1 - a.1) as i64 - (b.1 - a.1) as i64 * (p.0 - a.0) as i64
}

pub fn dist2(a: V2, b: V2) -> i64 {
    let (dx, dy) = ((a.0 - b.0) as i64, (a.1 - b.1) as i64);
    dx * dx + dy * dy
}

/// True if point `p` is strictly closer than `r` to segment `a-b`.
pub fn point_seg_within(p: V2, a: V2, b: V2, r: i64) -> bool {
    let len2 = dist2(a, b);
    let t = (p.0 - a.0) as i64 * (b.0 - a.0) as i64 + (p.1 - a.1) as i64 * (b.1 - a.1) as i64;
    if t <= 0 {
        dist2(p, a) < r * r
    } else if t >= len2 {
        dist2(p, b) < r * r
    } else {
        let c = cross(a, b, p) as i128;
        c * c < (r * r) as i128 * len2 as i128
    }
}

/// True if the closed segment touches the closed rectangle.
pub fn seg_rect_intersects(a: V2, b: V2, q: Rect) -> bool {
    if a.0.max(b.0) < q.x0 || a.0.min(b.0) > q.x1 || a.1.max(b.1) < q.y0 || a.1.min(b.1) > q.y1 {
        return false;
    }
    // With overlapping bounding boxes the segment misses the rectangle only if all four corners
    // lie strictly on one side of its line.
    let (mut pos, mut neg, mut zero) = (false, false, false);
    for c in q.corners() {
        let s = cross(a, b, c);
        pos |= s > 0;
        neg |= s < 0;
        zero |= s == 0;
    }
    zero || (pos && neg)
}

/// True if the segment comes strictly closer than `r` to the rectangle.
pub fn seg_rect_within(a: V2, b: V2, q: Rect, r: i64) -> bool {
    if r <= 0 {
        return false;
    }
    if seg_rect_intersects(a, b, q) {
        return true;
    }
    // Disjoint convex shapes: the closest pair involves a rectangle corner or a segment endpoint.
    if q.corners().iter().any(|&c| point_seg_within(c, a, b, r)) {
        return true;
    }
    [a, b]
        .iter()
        .any(|&p| dist2(p, (p.0.clamp(q.x0, q.x1), p.1.clamp(q.y0, q.y1))) < r * r)
}

/// Conservative y-range of the part of segment `a-b` whose x lies in `[x_lo, x_hi]`.
fn y_span(a: V2, b: V2, x_lo: i32, x_hi: i32) -> (i32, i32) {
    if a.0 == b.0 {
        return (a.1.min(b.1), a.1.max(b.1));
    }
    let (l, rgt) = if a.0 < b.0 { (a, b) } else { (b, a) };
    let (xa, xb) = (x_lo.clamp(l.0, rgt.0), x_hi.clamp(l.0, rgt.0));
    let at = |x: i32| -> i64 {
        l.1 as i64 + ((x - l.0) as i64 * (rgt.1 - l.1) as i64).div_euclid((rgt.0 - l.0) as i64)
    };
    let (ya, yb) = (at(xa), at(xb));
    ((ya.min(yb) - 1) as i32, (ya.max(yb) + 1) as i32)
}

/// True if a circle of radius `r` can slide its centre along the whole segment `a-b` without
/// `Map::circle_blocked` ever being true.
pub fn swept_free(map: &Map, a: V2, b: V2, r: i32) -> bool {
    let tx0 = (a.0.min(b.0) - r).div_euclid(TILE);
    let tx1 = (a.0.max(b.0) + r).div_euclid(TILE);
    for tx in tx0..=tx1 {
        let (lo, hi) = y_span(a, b, tx * TILE - r, (tx + 1) * TILE + r);
        let (ty0, ty1) = ((lo - r).div_euclid(TILE), (hi + r).div_euclid(TILE));
        for ty in ty0..=ty1 {
            if map.tile(tx, ty) != TILE_FLOOR && seg_rect_within(a, b, Rect::tile(tx, ty), r as i64)
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_segment_distance_is_strict() {
        assert!(point_seg_within((0, 299), (-100, 0), (100, 0), 300));
        assert!(!point_seg_within((0, 300), (-100, 0), (100, 0), 300));
        assert!(point_seg_within((399, 0), (-100, 0), (100, 0), 300));
        assert!(!point_seg_within((400, 0), (-100, 0), (100, 0), 300));
    }

    #[test]
    fn segment_rect_cases() {
        let q = Rect {
            x0: 0,
            y0: 0,
            x1: 1024,
            y1: 1024,
        };
        assert!(seg_rect_intersects((-500, 512), (1500, 512), q));
        assert!(seg_rect_intersects((-100, 150), (150, -100), q));
        assert!(
            seg_rect_intersects((-100, 100), (100, -100), q),
            "touching a corner counts"
        );
        assert!(!seg_rect_intersects((-100, 0), (0, -101), q));
        assert!(seg_rect_within((-100, -200), (1200, -200), q, 300));
        assert!(!seg_rect_within((-100, -300), (1200, -300), q, 300));
    }

    #[test]
    fn swept_test_agrees_with_circle_blocked_samples() {
        let m =
            Map::parse("name: t\n---\n########\n#a..#.d#\n#....o.#\n#.#..1.#\n########\n").unwrap();
        let pts: Vec<V2> = (0..40)
            .map(|i| (1100 + i * 157 % 6000, 1100 + i * 311 % 3000))
            .collect();
        let mut free = 0;
        for &a in &pts {
            for &b in &pts {
                let sampled = (0..=256).any(|k| {
                    let p = (a.0 + (b.0 - a.0) * k / 256, a.1 + (b.1 - a.1) * k / 256);
                    m.circle_blocked(p, 300)
                });
                // A blocked sample means the sweep is blocked; the exact test may also catch a graze
                // that falls between samples, never the other way round.
                if sampled {
                    assert!(!swept_free(&m, a, b, 300), "{a:?}->{b:?}");
                }
                free += swept_free(&m, a, b, 300) as u32;
            }
        }
        assert!(
            free > 100,
            "the layout has open floor, most short hops are free ({free})"
        );
    }
}
