//! Deterministic, float-free geometry. Angles are 0..65535 per turn, 0 = +x, increasing towards +y.
use ibc_abi::FULL_TURN;

/// atan(i/128) in angle units, i = 0..=128.
const ATAN: [u16; 129] = [
    0, 81, 163, 244, 326, 407, 489, 570, 651, 732, 813, 894, 975, 1056, 1136, 1217, 1297, 1377, 1457, 1537, 1617,
    1696, 1775, 1854, 1933, 2012, 2090, 2168, 2246, 2324, 2401, 2478, 2555, 2632, 2708, 2784, 2860, 2935, 3010,
    3085, 3159, 3233, 3307, 3380, 3453, 3526, 3599, 3670, 3742, 3813, 3884, 3955, 4025, 4095, 4164, 4233, 4302,
    4370, 4438, 4505, 4572, 4639, 4705, 4771, 4836, 4901, 4966, 5030, 5094, 5157, 5220, 5282, 5344, 5406, 5467,
    5528, 5589, 5649, 5708, 5768, 5826, 5885, 5943, 6000, 6058, 6114, 6171, 6227, 6282, 6337, 6392, 6446, 6500,
    6554, 6607, 6660, 6712, 6764, 6815, 6867, 6917, 6968, 7018, 7068, 7117, 7166, 7214, 7262, 7310, 7358, 7405,
    7451, 7498, 7544, 7589, 7635, 7679, 7724, 7768, 7812, 7856, 7899, 7942, 7984, 8026, 8068, 8110, 8151, 8192,
];

/// sin(i/64 * quarter turn) * 32768, i = 0..=64.
const SIN: [u16; 65] = [
    0, 804, 1608, 2411, 3212, 4011, 4808, 5602, 6393, 7180, 7962, 8740, 9512, 10279, 11039, 11793, 12540, 13279,
    14010, 14733, 15447, 16151, 16846, 17531, 18205, 18868, 19520, 20160, 20788, 21403, 22006, 22595, 23170, 23732,
    24279, 24812, 25330, 25833, 26320, 26791, 27246, 27684, 28106, 28511, 28899, 29269, 29622, 29957, 30274, 30572,
    30853, 31114, 31357, 31581, 31786, 31972, 32138, 32286, 32413, 32522, 32610, 32679, 32729, 32758, 32768,
];

/// Wrap any angle into 0..65535.
#[inline]
pub fn norm_angle(a: i32) -> i32 {
    a & (FULL_TURN - 1)
}

/// Integer atan2: the angle of the vector (dx, dy), 0..65535. `angle_to(0, 0)` is 0.
pub fn angle_to(dx: i32, dy: i32) -> i32 {
    let (ax, ay) = ((dx as i64).abs(), (dy as i64).abs());
    if ax == 0 && ay == 0 {
        return 0;
    }
    let (lo, hi) = if ax >= ay { (ay, ax) } else { (ax, ay) };
    // ratio in 0..=128*256, then table lookup with linear interpolation
    let r = (lo * 32768 / hi) as i32;
    let (i, f) = ((r >> 8) as usize, r & 255);
    let a = if i >= 128 { 8192 } else { ATAN[i] as i32 + (((ATAN[i + 1] as i32 - ATAN[i] as i32) * f + 128) >> 8) };
    let a = if ax >= ay { a } else { 16384 - a };
    let a = if dx < 0 { 32768 - a } else { a };
    norm_angle(if dy < 0 { -a } else { a })
}

/// Signed shortest turn from `a` to `b`, in -32768..=32767 (positive = towards +y / clockwise on screen).
#[inline]
pub fn angle_diff(a: i32, b: i32) -> i32 {
    (b.wrapping_sub(a) as i16) as i32
}

/// Floor of the square root.
pub fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = 1i64 << (64 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) >> 1;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// Euclidean distance between two points, rounded down, saturating at `i32::MAX`.
#[inline]
pub fn dist(a: (i32, i32), b: (i32, i32)) -> i32 {
    len_wide(b.0 as i64 - a.0 as i64, b.1 as i64 - a.1 as i64)
}

/// Squared distance, saturating at `i64::MAX` (only points about 3e9 units apart reach it).
#[inline]
pub fn dist2(a: (i32, i32), b: (i32, i32)) -> i64 {
    match squares(b.0 as i64 - a.0 as i64, b.1 as i64 - a.1 as i64) {
        Some(n) if n <= i64::MAX as u64 => n as i64,
        _ => i64::MAX,
    }
}

/// Length of a vector, rounded down, saturating at `i32::MAX`.
#[inline]
pub fn len(dx: i32, dy: i32) -> i32 {
    len_wide(dx as i64, dy as i64)
}

/// Length of a vector whose components are differences of two `i32`s, saturating at `i32::MAX`.
fn len_wide(dx: i64, dy: i64) -> i32 {
    match squares(dx, dy) {
        // Below 2^62 the root is below 2^31; from there on it is past `i32::MAX`.
        Some(n) if n < 1 << 62 => isqrt(n as i64) as i32,
        _ => i32::MAX,
    }
}

/// `dx * dx + dy * dy` for components below 2^32 in magnitude, or `None` past `u64::MAX`.
fn squares(dx: i64, dy: i64) -> Option<u64> {
    // Each square is below 2^64, so the wrapped product read as a u64 is exact.
    (dx.wrapping_mul(dx) as u64).checked_add(dy.wrapping_mul(dy) as u64)
}

/// (cos, sin) of an angle, scaled by 32768.
pub fn cos_sin(angle: i32) -> (i32, i32) {
    (sin_q15(angle + 16384), sin_q15(angle))
}

fn sin_q15(angle: i32) -> i32 {
    let a = norm_angle(angle);
    let (quad, r) = (a >> 14, a & 16383);
    let r = if quad & 1 == 1 { 16384 - r } else { r };
    let (i, f) = ((r >> 8) as usize, r & 255);
    let v = if i >= 64 { 32768 } else { SIN[i] as i32 + (((SIN[i + 1] as i32 - SIN[i] as i32) * f + 128) >> 8) };
    if quad >= 2 {
        -v
    } else {
        v
    }
}

/// The point `d` units from (x, y) in direction `angle`.
pub fn offset(x: i32, y: i32, angle: i32, d: i32) -> (i32, i32) {
    let (c, s) = cos_sin(angle);
    (x + ((c as i64 * d as i64) >> 15) as i32, y + ((s as i64 * d as i64) >> 15) as i32)
}

/// Where to aim so a bullet of `bullet_speed` (units/tick), leaving after `windup` ticks,
/// meets a target at `target` moving with constant velocity `vel` (units/tick).
/// Each coordinate saturates at the `i32` range (a very slow bullet aimed at a fast target).
pub fn lead(from: (i32, i32), target: (i32, i32), vel: (i32, i32), bullet_speed: i32, windup: i32) -> (i32, i32) {
    let speed = bullet_speed.max(1) as i64;
    let mut p = target;
    for _ in 0..3 {
        // Ticks until impact: windup plus at most `dist`, so within -2^31..2^32.
        let t = windup as i64 + (dist(from, p) as i64 + speed - 1) / speed;
        p = (along(target.0, vel.0, t), along(target.1, vel.1, t));
    }
    p
}

/// `x + v * t`, saturated to the i32 range. Exact in i64 for any `t` in -2^32..2^32.
fn along(x: i32, v: i32, t: i64) -> i32 {
    (x as i64 + v as i64 * t).clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atan_cardinals_and_diagonals() {
        assert_eq!(angle_to(10, 0), 0);
        assert_eq!(angle_to(0, 10), 16384);
        assert_eq!(angle_to(-10, 0), 32768);
        assert_eq!(angle_to(0, -10), 49152);
        assert_eq!(angle_to(7, 7), 8192);
        assert_eq!(angle_to(-7, 7), 24576);
        assert_eq!(angle_to(-7, -7), 40960);
        assert_eq!(angle_to(7, -7), 57344);
        assert_eq!(angle_to(0, 0), 0);
    }

    #[test]
    fn atan_matches_float_within_two_units() {
        for &(dx, dy) in &[(1000, 1), (1000, 333), (123, 4567), (-5000, 77), (-3, -90000), (40000, -39999)] {
            let want = ((dy as f64).atan2(dx as f64) / std::f64::consts::TAU * 65536.0).round() as i32;
            assert!(angle_diff(angle_to(dx, dy), want).abs() <= 2, "{dx},{dy}");
        }
    }

    #[test]
    fn diff_and_sqrt() {
        assert_eq!(angle_diff(65000, 100), 636);
        assert_eq!(angle_diff(100, 65000), -636);
        assert_eq!(angle_diff(0, 32768), -32768);
        for n in [0i64, 1, 2, 3, 4, 15, 16, 17, 1 << 40, (1 << 40) + 1, i64::MAX] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1).checked_mul(r + 1).is_none_or(|s| s > n), "{n}");
        }
        assert_eq!(dist((0, 0), (3000, 4000)), 5000);
    }

    #[test]
    fn offset_round_trips_through_angle_to() {
        for a in (0..65536).step_by(997) {
            let (x, y) = offset(0, 0, a, 100_000);
            assert!(angle_diff(angle_to(x, y), a).abs() <= 6, "{a}");
            assert!((len(x, y) - 100_000).abs() < 200);
        }
    }
}
