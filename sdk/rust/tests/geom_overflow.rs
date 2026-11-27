//! `len`, `dist`, `dist2` and `lead` used to overflow on extreme inputs: a panic in a debug build, a wrapped
//! result in release. They now compute in wider integers and saturate. These tests check that every input
//! the old code handled gives the same answer as before, and that every input gives the exact answer,
//! saturated to the result type (run them in a debug build, where an overflow would panic).
use ibc::{dist, dist2, lead, len};

type P = (i32, i32);

/// The code before the fix, with each operator in its checked form: `None` exactly where it panicked in a
/// debug build, or (the `as i32` at the end of `len`) silently wrapped in both.
mod old {
    use super::P;
    use ibc::isqrt;

    pub fn len(dx: i32, dy: i32) -> Option<i32> {
        let n = (dx as i64 * dx as i64).checked_add(dy as i64 * dy as i64)?;
        i32::try_from(isqrt(n)).ok()
    }

    pub fn dist(a: P, b: P) -> Option<i32> {
        len(b.0.checked_sub(a.0)?, b.1.checked_sub(a.1)?)
    }

    pub fn dist2(a: P, b: P) -> Option<i64> {
        let (dx, dy) = (b.0.checked_sub(a.0)? as i64, b.1.checked_sub(a.1)? as i64);
        (dx * dx).checked_add(dy * dy)
    }

    pub fn lead(from: P, target: P, vel: P, bullet_speed: i32, windup: i32) -> Option<P> {
        let speed = bullet_speed.max(1);
        let mut p = target;
        for _ in 0..3 {
            let t =
                windup.checked_add(dist(from, p)?.checked_add(speed)?.checked_sub(1)? / speed)?;
            p = (
                target.0.checked_add(vel.0.checked_mul(t)?)?,
                target.1.checked_add(vel.1.checked_mul(t)?)?,
            );
        }
        Some(p)
    }
}

/// The intended answers, in i128 with an independent square root, saturated where a result is an i32/i64.
mod exact {
    use super::P;

    fn sqrt(n: u128) -> u128 {
        let (mut lo, mut hi) = (0u128, 1 << 34); // n < 2^66
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if mid * mid <= n {
                lo = mid
            } else {
                hi = mid
            }
        }
        lo
    }

    fn sat32(v: i128) -> i32 {
        v.clamp(i32::MIN.into(), i32::MAX.into()) as i32
    }

    fn sq(dx: i128, dy: i128) -> i128 {
        dx * dx + dy * dy
    }

    pub fn len(dx: i128, dy: i128) -> i32 {
        sat32(sqrt(sq(dx, dy) as u128) as i128)
    }

    pub fn dist(a: P, b: P) -> i32 {
        len(b.0 as i128 - a.0 as i128, b.1 as i128 - a.1 as i128)
    }

    pub fn dist2(a: P, b: P) -> i64 {
        sq(b.0 as i128 - a.0 as i128, b.1 as i128 - a.1 as i128).min(i64::MAX.into()) as i64
    }

    pub fn lead(from: P, target: P, vel: P, bullet_speed: i32, windup: i32) -> P {
        let speed = bullet_speed.max(1) as i128;
        let mut p = target;
        for _ in 0..3 {
            let t = windup as i128 + (dist(from, p) as i128 + speed - 1) / speed;
            p = (
                sat32(target.0 as i128 + vel.0 as i128 * t),
                sat32(target.1 as i128 + vel.1 as i128 * t),
            );
        }
        p
    }
}

/// The LCG of `sdk/cpp/tests/geom_test.cpp`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u32) as i32
    }
    /// Any i32: below 2^k in magnitude, for k uniform in 0..32, either sign.
    fn wide(&mut self) -> i32 {
        let v = (self.next() & ((1u64 << (self.next() % 32)) - 1) as u32) as i32;
        if self.next() & 1 == 1 {
            -v - 1
        } else {
            v
        }
    }
    fn point(&mut self, lo: i32, hi: i32) -> P {
        (self.range(lo, hi), self.range(lo, hi))
    }
    fn wide_point(&mut self) -> P {
        (self.wide(), self.wide())
    }
}

/// Values around every boundary that matters here: small, around sqrt(2^31) and sqrt(2^62), and the ends.
const EDGES: [i32; 23] = [
    0,
    1,
    -1,
    2,
    -7,
    1000,
    -60000,
    46340,
    -46341,
    65535,
    1 << 20,
    -(1 << 30),
    1 << 30,
    1_518_500_249,
    -1_518_500_250,
    2_000_000_000,
    -2_000_000_000,
    i32::MAX - 1,
    i32::MAX,
    i32::MIN + 1,
    i32::MIN,
    i32::MAX / 2,
    i32::MIN / 2,
];

/// `dist(a, b)`, `dist2(a, b)` and `len(a.0, b.1)`: the same as before wherever the old code did not
/// overflow, and the exact answer always. Returns how many of the three the old code handled.
fn check_dists(a: P, b: P) -> u32 {
    let (x, y) = (a.0, b.1);
    assert_eq!(dist(a, b), exact::dist(a, b), "dist({a:?}, {b:?})");
    assert_eq!(dist2(a, b), exact::dist2(a, b), "dist2({a:?}, {b:?})");
    assert_eq!(len(x, y), exact::len(x.into(), y.into()), "len({x}, {y})");
    let mut handled = 0;
    if let Some(v) = old::dist(a, b) {
        assert_eq!(dist(a, b), v, "dist({a:?}, {b:?})");
        handled += 1;
    }
    if let Some(v) = old::dist2(a, b) {
        assert_eq!(dist2(a, b), v, "dist2({a:?}, {b:?})");
        handled += 1;
    }
    if let Some(v) = old::len(x, y) {
        assert_eq!(len(x, y), v, "len({x}, {y})");
        handled += 1;
    }
    handled
}

#[test]
fn len_dist_and_dist2_are_unchanged_where_they_did_not_overflow() {
    let (mut handled, mut checked) = (0, 0);
    for &x in &EDGES {
        for &y in &EDGES {
            for &u in &EDGES {
                for &w in &EDGES {
                    handled += check_dists((x, y), (u, w));
                    checked += 3;
                }
            }
        }
    }
    assert!(
        handled > 500_000,
        "the grid exercises the old code: {handled} of {checked} handled"
    );

    let mut r = Rng(11);
    for _ in 0..100_000 {
        let (a, b) = (r.point(-70000, 70000), r.point(-70000, 70000));
        assert_eq!(
            check_dists(a, b),
            3,
            "map-sized inputs never overflowed: {a:?} {b:?}"
        );
    }
    let mut overflowed = 0;
    for _ in 0..100_000 {
        overflowed += 3 - check_dists(r.wide_point(), r.wide_point());
    }
    assert!(
        overflowed > 100,
        "the sample reaches the overflow: {overflowed}"
    );
}

/// One `lead` call: same as before wherever the old code did not overflow, and the exact answer always.
/// Returns whether the old code handled it.
fn check_lead(from: P, target: P, vel: P, speed: i32, windup: i32) -> bool {
    let got = lead(from, target, vel, speed, windup);
    let args = format!("lead({from:?}, {target:?}, {vel:?}, {speed}, {windup})");
    assert_eq!(got, exact::lead(from, target, vel, speed, windup), "{args}");
    match old::lead(from, target, vel, speed, windup) {
        Some(before) => {
            assert_eq!(got, before, "{args}");
            true
        }
        None => false,
    }
}

#[test]
fn lead_is_unchanged_where_it_did_not_overflow() {
    let mut r = Rng(12);
    // What bots pass: positions on a map, unit velocities, real bullet speeds and windups.
    let mut handled = 0;
    for _ in 0..100_000 {
        let (from, target, vel) = (r.point(0, 70000), r.point(0, 70000), r.point(-700, 700));
        handled += check_lead(from, target, vel, r.range(100, 4000), r.range(0, 30)) as u32;
    }
    assert_eq!(handled, 100_000, "no realistic input overflowed before");

    // Slow bullets, where the old code started to overflow, and inputs of every magnitude.
    let (mut handled, mut overflowed) = (0, 0);
    for i in 0..300_000 {
        let (from, target, vel, speed, windup) = if i % 2 == 0 {
            (
                r.point(0, 70000),
                r.point(0, 70000),
                r.point(-700, 700),
                r.range(-1, 40),
                r.range(0, 30),
            )
        } else {
            (
                r.wide_point(),
                r.wide_point(),
                r.wide_point(),
                r.wide(),
                r.wide(),
            )
        };
        if check_lead(from, target, vel, speed, windup) {
            handled += 1
        } else {
            overflowed += 1
        }
    }
    assert!(
        handled > 100_000 && overflowed > 100_000,
        "{handled} handled, {overflowed} overflowed before"
    );
}

#[test]
fn extreme_inputs_saturate_instead_of_wrapping() {
    const MAX: i32 = i32::MAX;
    const MIN: i32 = i32::MIN;
    assert_eq!(len(MIN, MIN), MAX);
    assert_eq!(len(MAX, MAX), MAX);
    assert_eq!(len(MIN, 0), MAX);
    assert_eq!(len(-MAX, 0), MAX);
    assert_eq!(
        len(1_518_500_249, 1_518_500_249),
        2_147_483_646,
        "the longest vector that fits"
    );
    assert_eq!(dist((MIN, MIN), (MAX, MAX)), MAX);
    assert_eq!(dist((-1, 0), (MAX, 0)), MAX);
    assert_eq!(dist((MAX, 0), (0, 0)), MAX);
    assert_eq!(dist2((MIN, MIN), (MAX, MAX)), i64::MAX);
    assert_eq!(dist2((0, 0), (MIN, MIN)), i64::MAX, "2^63 does not fit");
    assert_eq!(dist2((0, 0), (MIN, 0)), 1 << 62);
    assert_eq!(dist2((0, 0), (MAX, MAX)), 2 * (MAX as i64).pow(2));
    assert_eq!(dist2((MIN, 0), (MAX, 0)), i64::MAX);

    // A bullet of speed 1 (or 0, or negative: treated as 1) at a target running at 700 units a tick.
    for speed in [1, 0, -5, MIN] {
        assert_eq!(lead((0, 0), (60000, 0), (700, 0), speed, 0), (MAX, 0));
        assert_eq!(lead((0, 0), (60000, 0), (-700, 300), speed, 0), (MIN, MAX));
    }
    // Huge windups, both signs.
    assert_eq!(lead((0, 0), (0, 0), (1, -1), 1000, MAX), (MAX, MIN));
    // The longest flight, t = 2^32 - 2 ticks, with the most negative velocity and position.
    assert_eq!(lead((MAX, MIN), (MIN, MAX), (MIN, MAX), 1, MAX), (MIN, MAX));
    assert_eq!(lead((MAX, MIN), (MIN, MAX), (MAX, MIN), 1, MAX), (MAX, MIN));
    // A target that does not move stays where it is, however far.
    assert_eq!(lead((MIN, MIN), (MAX, MAX), (0, 0), 1, MAX), (MAX, MAX));
    for (from, target, vel, speed, windup) in [
        ((MIN, MIN), (MAX, MAX), (MIN, MIN), 1, MIN),
        ((MAX, MAX), (MIN, MIN), (MAX, MAX), MAX, MAX),
        ((0, 0), (MAX, MIN), (-1, 1), 2, 0),
        ((0, 0), (1, 1), (MAX, MIN), 1, 0),
        ((0, 0), (5, 5), (1, 0), 1000, MIN),
    ] {
        check_lead(from, target, vel, speed, windup);
    }
}
