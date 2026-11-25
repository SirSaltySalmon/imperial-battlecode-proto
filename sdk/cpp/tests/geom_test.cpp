// Compile-time test: the C++ geometry helpers give the same results as `sdk/rust/src/geom.rs`.
//
// Everything is a `static_assert`, so compiling this file (`-fsyntax-only`) is the whole test; it is run by
// `cargo test -p ibc-cli` when wasi-sdk is available. The checksums were produced by the Rust SDK from the
// same pseudo-random inputs (same generator, same seeds, same order of draws): if you change a helper on
// one side, change the other and regenerate them.
#include "ibc.h"

using namespace ibc;

namespace {

struct Rng {
    uint64_t s;
    constexpr uint32_t next() {
        s = s * 6364136223846793005ull + 1442695040888963407ull;
        return static_cast<uint32_t>(s >> 33);
    }
    constexpr int32_t range(int32_t lo, int32_t hi) { return lo + static_cast<int32_t>(next() % static_cast<uint32_t>(hi - lo + 1)); }
    constexpr int32_t any() {
        const uint32_t a = next();
        const uint32_t b = next();
        return static_cast<int32_t>((a << 1) ^ b);
    }
};

constexpr uint64_t mix(uint64_t h, int64_t v) { return h * 1000003ull + static_cast<uint64_t>(v); }

constexpr uint64_t sum_angle_to() {
    Rng r{1};
    uint64_t h = 0;
    for (int i = 0; i < 2000; ++i) {
        const int32_t dx = r.range(-60000, 60000);
        const int32_t dy = r.range(-60000, 60000);
        h = mix(h, angle_to(dx, dy));
    }
    return h;
}

constexpr uint64_t sum_angle_diff() {
    Rng r{2};
    uint64_t h = 0;
    for (int i = 0; i < 2000; ++i) {
        const int32_t a = r.any();
        const int32_t b = r.any();
        h = mix(h, angle_diff(a, b));
        h = mix(h, norm_angle(a));
    }
    return h;
}

constexpr uint64_t sum_isqrt() {
    Rng r{3};
    uint64_t h = 0;
    for (int i = 0; i < 400; ++i) {
        const int64_t hi = r.next();
        const int64_t lo = r.next();
        h = mix(h, isqrt((hi << 31) | lo));
        h = mix(h, isqrt(r.range(0, 100000)));
    }
    return h;
}

constexpr uint64_t sum_cos_sin() {
    Rng r{4};
    uint64_t h = 0;
    for (int i = 0; i < 2000; ++i) {
        const CosSin cs = cos_sin(r.any());
        h = mix(h, cs.cos);
        h = mix(h, cs.sin);
    }
    return h;
}

constexpr uint64_t sum_offset() {
    Rng r{5};
    uint64_t h = 0;
    for (int i = 0; i < 2000; ++i) {
        const int32_t x = r.range(-60000, 60000);
        const int32_t y = r.range(-60000, 60000);
        const int32_t a = r.any();
        const int32_t d = r.range(-40000, 40000);
        const Point p = offset({x, y}, a, d);
        h = mix(h, p.x);
        h = mix(h, p.y);
    }
    return h;
}

constexpr uint64_t sum_lead() {
    Rng r{6};
    uint64_t h = 0;
    for (int i = 0; i < 600; ++i) {
        const int32_t fx = r.range(0, 60000);
        const int32_t fy = r.range(0, 60000);
        const int32_t tx = r.range(0, 60000);
        const int32_t ty = r.range(0, 60000);
        const int32_t vx = r.range(-700, 700);
        const int32_t vy = r.range(-700, 700);
        const int32_t speed = r.range(0, 4000);
        const int32_t windup = r.range(0, 5);
        const Point p = lead({fx, fy}, {tx, ty}, {vx, vy}, speed, windup);
        h = mix(h, p.x);
        h = mix(h, p.y);
    }
    return h;
}

constexpr uint64_t sum_dist() {
    Rng r{7};
    uint64_t h = 0;
    for (int i = 0; i < 1000; ++i) {
        const int32_t ax = r.range(-60000, 60000);
        const int32_t ay = r.range(-60000, 60000);
        const int32_t bx = r.range(-60000, 60000);
        const int32_t by = r.range(-60000, 60000);
        h = mix(h, dist({ax, ay}, {bx, by}));
        h = mix(h, dist2({ax, ay}, {bx, by}));
        h = mix(h, len(ax, by));
        const Point t = tile_of({ax, ay});
        h = mix(h, t.x);
        h = mix(h, t.y);
        const Point c = tile_centre(t);
        h = mix(h, c.x);
        h = mix(h, c.y);
    }
    return h;
}

static_assert(sum_angle_to() == 11710184062872645683ull);
static_assert(sum_angle_diff() == 6282051905982044937ull);
static_assert(sum_isqrt() == 8501403630845804455ull);
static_assert(sum_cos_sin() == 17938614175399592509ull);
static_assert(sum_offset() == 11064399518345077816ull);
static_assert(sum_lead() == 16454461919611396669ull);
static_assert(sum_dist() == 7256156328770612097ull);

// The spot checks of the Rust unit tests.
static_assert(angle_to(10, 0) == 0 && angle_to(0, 10) == 16384 && angle_to(-10, 0) == 32768 && angle_to(0, -10) == 49152);
static_assert(angle_to(7, 7) == 8192 && angle_to(-7, 7) == 24576 && angle_to(-7, -7) == 40960 && angle_to(7, -7) == 57344);
static_assert(angle_to(0, 0) == 0);
static_assert(angle_diff(65000, 100) == 636 && angle_diff(100, 65000) == -636 && angle_diff(0, 32768) == -32768);
static_assert(isqrt(0) == 0 && isqrt(15) == 3 && isqrt(16) == 4 && isqrt(17) == 4 && isqrt(INT64_MAX) == 3037000499ll);
static_assert(dist({0, 0}, {3000, 4000}) == 5000);
static_assert(tile_of({-1, 1024}) == Point{-1, 1} && tile_of({-1024, -1025}) == Point{-1, -2});
static_assert(error_name(Error::NotReady)[0] == 'n' && ok(Error::Ok) && !ok(Error::BadArg));

}  // namespace

int main() { return 0; }
