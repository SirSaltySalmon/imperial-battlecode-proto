//! Segment geometry the abilities need on top of `fixed`: exact integer segment crossing and
//! point-to-segment distance. Coordinates must be inside the map (callers validate bot-supplied points),
//! so every product below fits an i64 with room to spare.

use crate::fixed::{V2, ONE};

fn sub(a: V2, b: V2) -> (i64, i64) {
    (a.0 as i64 - b.0 as i64, a.1 as i64 - b.1 as i64)
}

fn cross(a: (i64, i64), b: (i64, i64)) -> i64 {
    a.0 * b.1 - a.1 * b.0
}

/// Where a->b crosses c->d, as a Q14 fraction along a->b. Touching counts; parallel segments never cross.
pub fn segment_segment(a: V2, b: V2, c: V2, d: V2) -> Option<i64> {
    let (r, s, qp) = (sub(b, a), sub(d, c), sub(c, a));
    let mut den = cross(r, s);
    if den == 0 {
        return None;
    }
    let (mut tn, mut un) = (cross(qp, s), cross(qp, r));
    if den < 0 {
        (den, tn, un) = (-den, -tn, -un);
    }
    if tn < 0 || tn > den || un < 0 || un > den {
        return None;
    }
    Some(tn * ONE / den)
}

/// Squared distance from `p` to the segment a->b.
pub fn point_segment_dist2(p: V2, a: V2, b: V2) -> i64 {
    let (ab, ap) = (sub(b, a), sub(p, a));
    let len2 = ab.0 * ab.0 + ab.1 * ab.1;
    if len2 == 0 {
        return ap.0 * ap.0 + ap.1 * ap.1;
    }
    let t = (ap.0 * ab.0 + ap.1 * ab.1).clamp(0, len2);
    let (dx, dy) = (ap.0 - ab.0 * t / len2, ap.1 - ab.1 * t / len2);
    dx * dx + dy * dy
}

/// The point `num / den` of the way from `a` to `b` (`den` > 0).
pub fn lerp(a: V2, b: V2, num: i64, den: i64) -> V2 {
    let d = sub(b, a);
    (a.0 + (d.0 * num / den) as i32, a.1 + (d.1 * num / den) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_and_distance() {
        let t = segment_segment((0, 0), (1000, 0), (250, -100), (250, 100)).unwrap();
        assert_eq!(t, ONE / 4);
        assert!(segment_segment((0, 0), (1000, 0), (250, 10), (250, 100)).is_none());
        assert!(segment_segment((0, 0), (200, 0), (250, -100), (250, 100)).is_none());
        assert!(segment_segment((0, 0), (1000, 0), (0, 50), (1000, 50)).is_none());
        // Direction of either segment does not matter.
        assert_eq!(segment_segment((0, 0), (1000, 0), (250, 100), (250, -100)), Some(ONE / 4));
        assert_eq!(point_segment_dist2((500, 300), (0, 0), (1000, 0)), 300 * 300);
        assert_eq!(point_segment_dist2((1300, 400), (0, 0), (1000, 0)), 300 * 300 + 400 * 400);
        assert_eq!(lerp((0, 0), (1000, -500), 1, 2), (500, -250));
    }
}
