//! The checksums that `sdk/cpp/tests/geom_test.cpp` asserts, computed from the Rust SDK with the same
//! generator, seeds and order of draws. If you change a geometry helper, change its C++ twin in
//! `sdk/cpp/ibc.h`, then update the number here and in `geom_test.cpp` together.
use ibc::{
    angle_diff, angle_to, cos_sin, dist, dist2, isqrt, lead, len, norm_angle, offset, tile_centre,
    tile_of,
};

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
    fn any(&mut self) -> i32 {
        let (a, b) = (self.next(), self.next());
        ((a << 1) ^ b) as i32
    }
}

fn mix(h: u64, v: i64) -> u64 {
    h.wrapping_mul(1000003).wrapping_add(v as u64)
}

fn sum_angle_to() -> u64 {
    let (mut r, mut h) = (Rng(1), 0);
    for _ in 0..2000 {
        let (dx, dy) = (r.range(-60000, 60000), r.range(-60000, 60000));
        h = mix(h, angle_to(dx, dy).into());
    }
    h
}

fn sum_angle_diff() -> u64 {
    let (mut r, mut h) = (Rng(2), 0);
    for _ in 0..2000 {
        let (a, b) = (r.any(), r.any());
        h = mix(h, angle_diff(a, b).into());
        h = mix(h, norm_angle(a).into());
    }
    h
}

fn sum_isqrt() -> u64 {
    let (mut r, mut h) = (Rng(3), 0);
    for _ in 0..400 {
        let (hi, lo) = (r.next() as i64, r.next() as i64);
        h = mix(h, isqrt((hi << 31) | lo));
        h = mix(h, isqrt(r.range(0, 100000).into()));
    }
    h
}

fn sum_cos_sin() -> u64 {
    let (mut r, mut h) = (Rng(4), 0);
    for _ in 0..2000 {
        let (c, s) = cos_sin(r.any());
        h = mix(h, c.into());
        h = mix(h, s.into());
    }
    h
}

fn sum_offset() -> u64 {
    let (mut r, mut h) = (Rng(5), 0);
    for _ in 0..2000 {
        let (x, y) = (r.range(-60000, 60000), r.range(-60000, 60000));
        let (a, d) = (r.any(), r.range(-40000, 40000));
        let p = offset(x, y, a, d);
        h = mix(h, p.0.into());
        h = mix(h, p.1.into());
    }
    h
}

fn sum_lead() -> u64 {
    let (mut r, mut h) = (Rng(6), 0);
    for _ in 0..600 {
        let from = (r.range(0, 60000), r.range(0, 60000));
        let target = (r.range(0, 60000), r.range(0, 60000));
        let vel = (r.range(-700, 700), r.range(-700, 700));
        let (speed, windup) = (r.range(0, 4000), r.range(0, 5));
        let p = lead(from, target, vel, speed, windup);
        h = mix(h, p.0.into());
        h = mix(h, p.1.into());
    }
    h
}

fn sum_dist() -> u64 {
    let (mut r, mut h) = (Rng(7), 0);
    for _ in 0..1000 {
        let (ax, ay) = (r.range(-60000, 60000), r.range(-60000, 60000));
        let (bx, by) = (r.range(-60000, 60000), r.range(-60000, 60000));
        h = mix(h, dist((ax, ay), (bx, by)).into());
        h = mix(h, dist2((ax, ay), (bx, by)));
        h = mix(h, len(ax, by).into());
        let t = tile_of(ax, ay);
        h = mix(h, t.0.into());
        h = mix(h, t.1.into());
        let c = tile_centre(t.0, t.1);
        h = mix(h, c.0.into());
        h = mix(h, c.1.into());
    }
    h
}

/// Extreme inputs, where `len`, `dist`, `dist2` and `lead` saturate: any i32 coordinates and velocities,
/// slow bullets and any windup.
fn sum_saturating() -> u64 {
    let (mut r, mut h) = (Rng(8), 0);
    for _ in 0..1000 {
        let a = (r.any(), r.any());
        let b = (r.any(), r.any());
        h = mix(h, dist(a, b).into());
        h = mix(h, dist2(a, b));
        h = mix(h, len(a.0, b.1).into());
        let vel = (r.any(), r.any());
        let speed = r.range(-2, 30);
        let windup = r.any();
        let p = lead(a, b, vel, speed, windup);
        h = mix(h, p.0.into());
        h = mix(h, p.1.into());
        let walk = (r.range(-700, 700), r.range(-700, 700));
        let windup = r.range(0, 5);
        let p = lead(a, b, walk, speed, windup);
        h = mix(h, p.0.into());
        h = mix(h, p.1.into());
    }
    h
}

#[test]
fn checksums_match_the_cpp_port() {
    let got = [
        ("angle_to", sum_angle_to()),
        ("angle_diff", sum_angle_diff()),
        ("isqrt", sum_isqrt()),
        ("cos_sin", sum_cos_sin()),
        ("offset", sum_offset()),
        ("lead", sum_lead()),
        ("dist", sum_dist()),
        ("saturating", sum_saturating()),
    ];
    let want = [
        11710184062872645683,
        6282051905982044937,
        8501403630845804455,
        17938614175399592509,
        11064399518345077816,
        7056640344810707269,
        7256156328770612097,
        11519087152713774159,
    ];
    for ((name, got), want) in got.into_iter().zip(want) {
        assert_eq!(
            got, want,
            "sum_{name}: update geom_test.cpp along with this file"
        );
    }
}
