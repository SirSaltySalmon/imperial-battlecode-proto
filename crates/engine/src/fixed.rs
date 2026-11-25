//! Integer-only geometry. Positions: 1024 units per tile. Angles: 65536 per turn, 0 = +x, increasing to +y.
//! Everything here must give identical results on every platform, so: no floats, no usize in results.

pub const ONE: i64 = 16384; // trig fixed-point scale (Q14)
pub type V2 = (i32, i32);

/// sin(angle) * ONE. Bhaskara I approximation, max error about 0.16%.
pub fn sin(angle: i32) -> i64 {
    let a = (angle & 0xFFFF) as i64;
    let x = a & 0x7FFF; // position within the half turn
    let p = x * (32768 - x); // <= 2^28
    let v = (16 * p * ONE) / (5 * (1 << 30) - 4 * p);
    if a >= 32768 {
        -v
    } else {
        v
    }
}

pub fn cos(angle: i32) -> i64 {
    sin(angle + 16384)
}

/// Vector of length `len` pointing along `angle`.
pub fn polar(angle: i32, len: i32) -> V2 {
    (((cos(angle) * len as i64) / ONE) as i32, ((sin(angle) * len as i64) / ONE) as i32)
}

/// Integer atan2 returning 0..65535. Max error about 0.25 degrees. (0,0) returns 0.
pub fn atan2(dy: i32, dx: i32) -> i32 {
    if dx == 0 && dy == 0 {
        return 0;
    }
    let ax = (dx as i64).abs();
    let ay = (dy as i64).abs();
    let (lo, hi) = if ax >= ay { (ay, ax) } else { (ax, ay) };
    let r = (lo << 12) / hi; // Q12 in [0, 4096]
    // atan(r) ~ r*45deg + 0.273*r*(1-r) rad  -> brads: 8192*r + 2847*r*(1-r)
    let mut a = (8192 * r + (2847 * r * (4096 - r)) / 4096) / 4096; // 0..8192
    if ay > ax {
        a = 16384 - a;
    }
    if dx < 0 {
        a = 32768 - a;
    }
    if dy < 0 {
        a = 65536 - a;
    }
    (a & 0xFFFF) as i32
}

/// Signed shortest difference `b - a` in -32768..32767.
pub fn angle_diff(a: i32, b: i32) -> i32 {
    (((b - a) & 0xFFFF) as u16) as i16 as i32
}

pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = 1u64 << ((64 - n.leading_zeros() + 1) / 2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

pub fn dist2(a: V2, b: V2) -> i64 {
    let dx = (a.0 - b.0) as i64;
    let dy = (a.1 - b.1) as i64;
    dx * dx + dy * dy
}

pub fn dist(a: V2, b: V2) -> i32 {
    isqrt(dist2(a, b) as u64) as i32
}

/// Earliest parameter t in [0, 1] (as a Q14 fraction) where the segment a->b comes within `r` of `c`, if any.
pub fn segment_circle(a: V2, b: V2, c: V2, r: i32) -> Option<i64> {
    let dx = (b.0 - a.0) as i64;
    let dy = (b.1 - a.1) as i64;
    let fx = (a.0 - c.0) as i64;
    let fy = (a.1 - c.1) as i64;
    let rr = (r as i64) * (r as i64);
    if fx * fx + fy * fy <= rr {
        return Some(0);
    }
    let len2 = dx * dx + dy * dy;
    if len2 == 0 {
        return None;
    }
    // Closest approach along the segment, clamped.
    let t_num = -(fx * dx + fy * dy);
    if t_num <= 0 {
        return None; // moving away, and the start is outside
    }
    let t_num = t_num.min(len2);
    let px = fx + dx * t_num / len2;
    let py = fy + dy * t_num / len2;
    if px * px + py * py > rr {
        return None;
    }
    // Entry point: back off from the closest point by the half chord.
    let d2 = px * px + py * py;
    let half = isqrt(((rr - d2) as u64) * ONE as u64 * ONE as u64 / len2 as u64) as i64; // Q14 of segment length
    let t = (t_num * ONE / len2 - half).max(0);
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trig_is_close() {
        for i in 0..64 {
            let a = i * 1024;
            let f = (a as f64) * std::f64::consts::TAU / 65536.0;
            assert!((sin(a) as f64 / ONE as f64 - f.sin()).abs() < 0.003, "sin {a}");
            assert!((cos(a) as f64 / ONE as f64 - f.cos()).abs() < 0.003, "cos {a}");
        }
    }

    #[test]
    fn atan2_roundtrip() {
        for i in 0..256 {
            let a = i * 256;
            let (dx, dy) = polar(a, 20000);
            assert!(angle_diff(a, atan2(dy, dx)).abs() < 80, "angle {a} got {}", atan2(dy, dx));
        }
    }

    #[test]
    fn sqrt_and_circle() {
        assert_eq!(isqrt(1_000_000), 1000);
        assert_eq!(isqrt(999_999), 999);
        assert!(segment_circle((0, 0), (10000, 0), (5000, 200), 300).is_some());
        assert!(segment_circle((0, 0), (10000, 0), (5000, 400), 300).is_none());
        assert!(segment_circle((0, 0), (1000, 0), (5000, 0), 300).is_none());
        let t = segment_circle((0, 0), (10000, 0), (5000, 0), 300).unwrap();
        assert!((t - ONE * 47 / 100).abs() < 20);
    }
}
