// `ibc.h`: the header-only C++20 bot SDK for Imperial Battlecode. Mirrors `sdk/rust`.
//
//   #include "ibc.h"
//   struct MyBot { void turn() { ... } };
//   IBC_BOT(MyBot)
//
// One bot instance runs per unit and lives for the whole match; `turn` is called once per tick.
// Nothing here throws, and nothing allocates after the first call that needs a buffer (the map grid and
// up to `Pathfinder::CACHE` distance fields are `malloc`ed once and reused). See README.md.
#pragma once

#include "ibc_abi.h"

#include <array>
#include <bit>
#include <cstdarg>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <optional>
#include <string_view>
#include <type_traits>
#include <utility>

namespace ibc {

// ---------------------------------------------------------------------------------------------------
// Geometry: deterministic and float-free. Bit-for-bit the same results as `sdk/rust/src/geom.rs`.
// ---------------------------------------------------------------------------------------------------

/// A world position or a vector, in fixed-point units (`TILE` per tile). Also used for tile coordinates.
struct Point {
    int32_t x = 0;
    int32_t y = 0;
    friend constexpr bool operator==(const Point&, const Point&) = default;
};

namespace detail {
/// atan(i/128) in angle units, i = 0..=128.
inline constexpr uint16_t ATAN[129] = {
    0,    81,   163,  244,  326,  407,  489,  570,  651,  732,  813,  894,  975,  1056, 1136, 1217, 1297, 1377, 1457,
    1537, 1617, 1696, 1775, 1854, 1933, 2012, 2090, 2168, 2246, 2324, 2401, 2478, 2555, 2632, 2708, 2784, 2860, 2935,
    3010, 3085, 3159, 3233, 3307, 3380, 3453, 3526, 3599, 3670, 3742, 3813, 3884, 3955, 4025, 4095, 4164, 4233, 4302,
    4370, 4438, 4505, 4572, 4639, 4705, 4771, 4836, 4901, 4966, 5030, 5094, 5157, 5220, 5282, 5344, 5406, 5467, 5528,
    5589, 5649, 5708, 5768, 5826, 5885, 5943, 6000, 6058, 6114, 6171, 6227, 6282, 6337, 6392, 6446, 6500, 6554, 6607,
    6660, 6712, 6764, 6815, 6867, 6917, 6968, 7018, 7068, 7117, 7166, 7214, 7262, 7310, 7358, 7405, 7451, 7498, 7544,
    7589, 7635, 7679, 7724, 7768, 7812, 7856, 7899, 7942, 7984, 8026, 8068, 8110, 8151, 8192,
};
/// sin(i/64 * quarter turn) * 32768, i = 0..=64.
inline constexpr uint16_t SIN[65] = {
    0,     804,   1608,  2411,  3212,  4011,  4808,  5602,  6393,  7180,  7962,  8740,  9512,
    10279, 11039, 11793, 12540, 13279, 14010, 14733, 15447, 16151, 16846, 17531, 18205, 18868,
    19520, 20160, 20788, 21403, 22006, 22595, 23170, 23732, 24279, 24812, 25330, 25833, 26320,
    26791, 27246, 27684, 28106, 28511, 28899, 29269, 29622, 29957, 30274, 30572, 30853, 31114,
    31357, 31581, 31786, 31972, 32138, 32286, 32413, 32522, 32610, 32679, 32729, 32758, 32768,
};

constexpr int32_t sin_q15(int32_t angle) {
    const int32_t a = angle & (FULL_TURN - 1);
    const int32_t quad = a >> 14;
    int32_t r = a & 16383;
    if (quad & 1) r = 16384 - r;
    const int32_t i = r >> 8, f = r & 255;
    const int32_t v = i >= 64 ? 32768 : SIN[i] + (((SIN[i + 1] - SIN[i]) * f + 128) >> 8);
    return quad >= 2 ? -v : v;
}

constexpr int32_t floor_div(int32_t a, int32_t b) { return a >= 0 ? a / b : -((-a + b - 1) / b); }
}  // namespace detail

/// Wrap any angle into 0..65535.
constexpr int32_t norm_angle(int32_t a) { return a & (FULL_TURN - 1); }

/// Integer atan2: the angle of the vector (dx, dy), 0..65535. `angle_to(0, 0)` is 0.
constexpr int32_t angle_to(int32_t dx, int32_t dy) {
    const int64_t ax = dx < 0 ? -int64_t{dx} : int64_t{dx};
    const int64_t ay = dy < 0 ? -int64_t{dy} : int64_t{dy};
    if (ax == 0 && ay == 0) return 0;
    const int64_t lo = ax >= ay ? ay : ax, hi = ax >= ay ? ax : ay;
    // ratio in 0..=128*256, then table lookup with linear interpolation
    const int32_t r = static_cast<int32_t>(lo * 32768 / hi);
    const int32_t i = r >> 8, f = r & 255;
    int32_t a = i >= 128 ? 8192 : detail::ATAN[i] + (((detail::ATAN[i + 1] - detail::ATAN[i]) * f + 128) >> 8);
    if (ax < ay) a = 16384 - a;
    if (dx < 0) a = 32768 - a;
    return norm_angle(dy < 0 ? -a : a);
}
constexpr int32_t angle_to(Point from, Point to) { return angle_to(to.x - from.x, to.y - from.y); }

/// Signed shortest turn from `a` to `b`, in -32768..=32767 (positive = towards +y / clockwise on screen).
constexpr int32_t angle_diff(int32_t a, int32_t b) {
    return static_cast<int16_t>(static_cast<uint16_t>(static_cast<uint32_t>(b) - static_cast<uint32_t>(a)));
}

/// Floor of the square root.
constexpr int64_t isqrt(int64_t n) {
    if (n <= 0) return 0;
    const int bits = 64 - std::countl_zero(static_cast<uint64_t>(n));
    int64_t x = int64_t{1} << ((bits + 1) / 2);
    for (;;) {
        const int64_t y = (x + n / x) >> 1;
        if (y >= x) return x;
        x = y;
    }
}

namespace detail {
/// `dx * dx + dy * dy` for components below 2^32 in magnitude, or nothing past `UINT64_MAX`.
constexpr std::optional<uint64_t> squares(int64_t dx, int64_t dy) {
    // Each square is below 2^64, so the wrapped unsigned product is exact.
    const uint64_t xx = static_cast<uint64_t>(dx) * static_cast<uint64_t>(dx);
    const uint64_t yy = static_cast<uint64_t>(dy) * static_cast<uint64_t>(dy);
    if (yy > UINT64_MAX - xx) return std::nullopt;
    return xx + yy;
}
/// Length of a vector whose components are differences of two `int32_t`s, saturating at `INT32_MAX`.
constexpr int32_t len_wide(int64_t dx, int64_t dy) {
    const std::optional<uint64_t> n = squares(dx, dy);
    // Below 2^62 the root is below 2^31; from there on it is past `INT32_MAX`.
    return n && *n < (uint64_t{1} << 62) ? static_cast<int32_t>(isqrt(static_cast<int64_t>(*n))) : INT32_MAX;
}
/// `x + v * t`, saturated to the `int32_t` range. Exact in 64 bits for any `t` in -2^32..2^32.
constexpr int32_t along(int32_t x, int32_t v, int64_t t) {
    const int64_t r = int64_t{x} + int64_t{v} * t;
    return static_cast<int32_t>(r < INT32_MIN ? INT32_MIN : r > INT32_MAX ? INT32_MAX : r);
}
}  // namespace detail

/// Length of a vector, rounded down, saturating at `INT32_MAX`.
constexpr int32_t len(int32_t dx, int32_t dy) { return detail::len_wide(dx, dy); }
/// Euclidean distance between two points, rounded down, saturating at `INT32_MAX`.
constexpr int32_t dist(Point a, Point b) { return detail::len_wide(int64_t{b.x} - a.x, int64_t{b.y} - a.y); }
/// Squared distance, saturating at `INT64_MAX` (only points about 3e9 units apart reach it).
constexpr int64_t dist2(Point a, Point b) {
    const std::optional<uint64_t> n = detail::squares(int64_t{b.x} - a.x, int64_t{b.y} - a.y);
    return n && *n <= static_cast<uint64_t>(INT64_MAX) ? static_cast<int64_t>(*n) : INT64_MAX;
}

struct CosSin {
    int32_t cos;
    int32_t sin;
};
/// (cos, sin) of an angle, scaled by 32768.
constexpr CosSin cos_sin(int32_t angle) {
    return {detail::sin_q15(static_cast<int32_t>(static_cast<uint32_t>(angle) + 16384u)), detail::sin_q15(angle)};
}

/// The point `d` units from (x, y) in direction `angle`.
constexpr Point offset(Point p, int32_t angle, int32_t d) {
    const CosSin cs = cos_sin(angle);
    return {p.x + static_cast<int32_t>((int64_t{cs.cos} * d) >> 15), p.y + static_cast<int32_t>((int64_t{cs.sin} * d) >> 15)};
}

/// Where to aim so a bullet of `bullet_speed` (units/tick), leaving after `windup` ticks,
/// meets a target at `target` moving with constant velocity `vel` (units/tick).
/// Each coordinate saturates at the `int32_t` range (a very slow bullet aimed at a fast target).
constexpr Point lead(Point from, Point target, Point vel, int32_t bullet_speed, int32_t windup) {
    const int64_t speed = bullet_speed > 1 ? bullet_speed : 1;
    Point p = target;
    for (int i = 0; i < 3; ++i) {
        // Ticks until impact: windup plus at most `dist`, so within -2^31..2^32.
        const int64_t t = windup + (dist(from, p) + speed - 1) / speed;
        p = {detail::along(target.x, vel.x, t), detail::along(target.y, vel.y, t)};
    }
    return p;
}

/// Tile containing a world position.
constexpr Point tile_of(Point p) { return {detail::floor_div(p.x, TILE), detail::floor_div(p.y, TILE)}; }
/// World position of a tile's centre.
constexpr Point tile_centre(int32_t tx, int32_t ty) { return {tx * TILE + TILE / 2, ty * TILE + TILE / 2}; }
constexpr Point tile_centre(Point t) { return tile_centre(t.x, t.y); }

// ---------------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------------

/// Result of an action or memory call: `Error::Ok` or a positive ABI error code (`ERR_*`).
enum class Error : int32_t {
    Ok = OK,
    BadArg = ERR_BAD_ARG,
    BufferTooSmall = ERR_BUFFER_TOO_SMALL,
    Unsupported = ERR_UNSUPPORTED,
    NotAllowed = ERR_NOT_ALLOWED,
    NotReady = ERR_NOT_READY,
    OutOfRange = ERR_OUT_OF_RANGE,
    InsufficientFunds = ERR_INSUFFICIENT_FUNDS,
    BadPtr = ERR_BAD_PTR,
};

constexpr bool ok(Error e) { return e == Error::Ok; }

constexpr const char* error_name(Error e) {
    switch (e) {
        case Error::Ok: return "ok";
        case Error::BadArg: return "bad argument";
        case Error::BufferTooSmall: return "buffer too small";
        case Error::Unsupported: return "unsupported";
        case Error::NotAllowed: return "not allowed";
        case Error::NotReady: return "not ready";
        case Error::OutOfRange: return "out of range";
        case Error::InsufficientFunds: return "insufficient funds";
        case Error::BadPtr: return "bad pointer";
    }
    return "unknown error";
}

namespace detail {
constexpr Error check(int32_t code) { return static_cast<Error>(code < 0 ? -code : code); }
}  // namespace detail

// ---------------------------------------------------------------------------------------------------
// Bot entry
// ---------------------------------------------------------------------------------------------------

/// Finish this tick. Called for you by `ibc::run` / `IBC_BOT`.
inline void end_turn() { ibc_sys_end_turn(); }
/// Fuel left in this tick's budget.
inline int64_t fuel_remaining() { return ibc_sys_fuel_remaining(); }

/// Construct `Bot` once, then call `bot.turn()` every tick, forever.
template <class Bot>
[[noreturn]] void run() {
    Bot bot;
    for (;;) {
        bot.turn();
        end_turn();
    }
}

/// Defines `main` for a bot: `IBC_BOT(MyBot)` at the end of one .cpp file.
#define IBC_BOT(Bot) \
    int main() { ::ibc::run<Bot>(); }

// ---------------------------------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------------------------------

/// ABI records: plain `int32_t` aggregates, valid for any bytes the host writes.
template <class T>
concept Record = std::is_trivially_copyable_v<T> && std::is_aggregate_v<T> && sizeof(T) % 4 == 0 && alignof(T) == 4;

/// One row of a host table, or nothing if the index is out of range.
template <Record T>
std::optional<T> read(uint32_t table, uint32_t index) {
    T v{};
    if (ibc_sys_get(table, index, &v, sizeof(T)) < 0) return std::nullopt;
    return v;
}

/// The rows of a host table as a lazy single-pass range: `for (const UnitInfo& e : ibc::enemies())`.
/// No allocation; one cheap `count` call up front and one `get` per row visited.
template <Record T>
class Rows {
  public:
    struct sentinel {};

    class iterator {
      public:
        using value_type = T;
        using difference_type = std::ptrdiff_t;

        iterator(uint32_t table, uint32_t len) : table_(table), len_(len) { advance(); }
        const T& operator*() const { return row_; }
        const T* operator->() const { return &row_; }
        iterator& operator++() {
            advance();
            return *this;
        }
        void operator++(int) { advance(); }
        bool operator==(sentinel) const { return !live_; }

      private:
        void advance() {
            live_ = false;
            while (next_ < len_) {
                if (ibc_sys_get(table_, next_++, &row_, sizeof(T)) >= 0) {
                    live_ = true;
                    return;
                }
            }
        }

        uint32_t table_;
        uint32_t next_ = 0;
        uint32_t len_;
        bool live_ = false;
        T row_{};
    };

    explicit Rows(uint32_t table) : table_(table) {
        const int32_t n = ibc_sys_count(table);
        len_ = n > 0 ? static_cast<uint32_t>(n) : 0;
    }
    iterator begin() const { return iterator(table_, len_); }
    sentinel end() const { return {}; }
    /// Rows in the table.
    uint32_t total() const { return len_; }
    bool empty() const { return len_ == 0; }
    /// Row `i`, or nothing if out of range.
    std::optional<T> at(uint32_t i) const { return i < len_ ? read<T>(table_, i) : std::nullopt; }

  private:
    uint32_t table_;
    uint32_t len_;
};

/// This unit, full detail. Fetch once per tick and pass it around.
inline UnitInfo me() { return read<UnitInfo>(T_SELF, 0).value_or(UnitInfo{}); }
/// Teammates (always known), excluding this unit. Includes dead ones: check `alive()`.
inline Rows<UnitInfo> allies() { return Rows<UnitInfo>(T_ALLIES); }
/// Living enemies THIS unit's team sees right now (vision is shared). One may be behind a wall from here:
/// check `los` before shooting.
inline Rows<UnitInfo> enemies() { return Rows<UnitInfo>(T_ENEMIES); }
/// Sounds the team heard during the previous tick (hearing is shared).
inline Rows<SoundInfo> sounds() { return Rows<SoundInfo>(T_SOUNDS); }
inline Rows<ProjectileInfo> projectiles() { return Rows<ProjectileInfo>(T_PROJECTILES); }
inline Rows<EffectInfo> effects() { return Rows<EffectInfo>(T_EFFECTS); }
inline Rows<DropInfo> drops() { return Rows<DropInfo>(T_DROPS); }
/// Events of the previous tick that this team receives. Kills always arrive; their position is `POS_HIDDEN`
/// when the team did not see it. Damage arrives only when seen.
inline Rows<EventInfo> events() { return Rows<EventInfo>(T_EVENTS); }
inline Rows<SiteInfo> sites() { return Rows<SiteInfo>(T_SITES); }
/// Every unit of both teams, by id: kit, credits at the start of this round's buy phase, alive.
inline Rows<RosterInfo> roster() { return Rows<RosterInfo>(T_ROSTER); }
/// Every finished round so far, oldest first: winner, how it ended, who attacked, score after it.
inline Rows<RoundInfo> rounds() { return Rows<RoundInfo>(T_ROUNDS); }
/// All shop items.
inline Rows<ItemRules> items() { return Rows<ItemRules>(T_ITEM); }
/// All weapons.
inline Rows<WeaponRules> weapons() { return Rows<WeaponRules>(T_WEAPON); }
inline MatchInfo match_info() { return read<MatchInfo>(T_MATCH, 0).value_or(MatchInfo{}); }
inline MapInfo map_info() { return read<MapInfo>(T_MAP, 0).value_or(MapInfo{}); }
/// Global numbers. Constant for the whole match: fetch once in the bot's constructor.
inline CoreRules core_rules() { return read<CoreRules>(T_RULES_CORE, 0).value_or(CoreRules{}); }
inline std::optional<WeaponRules> weapon(int32_t id) { return read<WeaponRules>(T_WEAPON, static_cast<uint32_t>(id)); }
inline std::optional<AbilityRules> ability(int32_t id) { return read<AbilityRules>(T_ABILITY, static_cast<uint32_t>(id)); }
inline std::optional<ItemRules> item(int32_t id) { return read<ItemRules>(T_ITEM, static_cast<uint32_t>(id)); }
inline std::optional<KitRules> kit(int32_t id) { return read<KitRules>(T_KIT, static_cast<uint32_t>(id)); }

/// Copy row `y` of the tile grid (`width` bytes of `TILE_*`) into `out`; returns bytes written or -error.
inline int32_t map_row(int32_t y, uint8_t* out, uint32_t cap) { return ibc_sys_get(T_MAP_ROW, static_cast<uint32_t>(y), out, cap); }

// Convenience predicates on `UnitInfo`.
constexpr bool has(const UnitInfo& u, int32_t state_flags) { return (u.state & state_flags) != 0; }
constexpr bool alive(const UnitInfo& u) { return has(u, S_ALIVE); }
constexpr Point pos(const UnitInfo& u) { return {u.x, u.y}; }
constexpr Point vel(const UnitInfo& u) { return {u.vx, u.vy}; }
/// Winding up, reloading, planting or defusing.
constexpr bool busy(const UnitInfo& u) { return has(u, S_WINDUP | S_RELOADING | S_PLANTING | S_DEFUSING); }

// ---------------------------------------------------------------------------------------------------
// Actions. One move, one facing and one "hands" action per tick; a later call of a kind replaces the earlier.
// ---------------------------------------------------------------------------------------------------

/// Raw `act` call for action kinds without a wrapper.
inline Error act(uint32_t kind, int32_t a = 0, int32_t b = 0, int32_t c = 0) { return detail::check(ibc_sys_act(kind, a, b, c)); }
/// Move in direction `angle` at `speed` units/tick (clamped). Above `walk_speed` is a noisy run.
inline Error move_dir(int32_t angle, int32_t speed) { return act(A_MOVE, norm_angle(angle), speed); }
/// Move straight towards a point (no pathfinding; see `Navigator`).
inline Error move_towards(const UnitInfo& me, Point p, int32_t speed) {
    const int32_t d = dist(pos(me), p);
    return move_dir(angle_to(pos(me), p), speed < d ? speed : d);
}
/// Turn towards `angle` (turn-rate limited).
inline Error face(int32_t angle) { return act(A_FACE, norm_angle(angle)); }
inline Error face_point(const UnitInfo& me, Point p) { return face(angle_to(pos(me), p)); }
/// Start the wind-up of a shot aimed at `angle`.
inline Error fire(int32_t angle) { return act(A_FIRE, norm_angle(angle)); }
inline Error fire_at(const UnitInfo& me, Point p) { return fire(angle_to(pos(me), p)); }
inline Error reload() { return act(A_RELOAD); }
/// Attacker carrying the charge, standing on a site.
inline Error plant() { return act(A_PLANT); }
/// Defender within `interact_reach` of the planted charge.
inline Error defuse() { return act(A_DEFUSE); }
/// Pick up the nearest drop within `pickup_reach`.
inline Error pickup() { return act(A_PICKUP); }
/// Buy a shop item (buy phase only).
inline Error buy(int32_t item_id) { return act(A_BUY, item_id); }
/// Buy a shop item for the teammate in `slot` (`MatchInfo.unit_slot`); this unit pays. Buy phase only.
inline Error buy_for(int32_t item_id, int32_t slot) { return act(A_BUY_FOR, item_id, slot); }
/// Use the ability in `slot` (0 or 1) at a target point.
inline Error use_ability(int32_t slot, Point target) { return act(A_ABILITY, slot, target.x, target.y); }
inline Error swap(int32_t slot) { return act(A_SWAP, slot); }
/// Drop something (`DROP_*`).
inline Error drop_item(int32_t kind) { return act(A_DROP, kind); }
/// Pick a kit (draft phase only). Kits other than 0 are unique within a team: `Error::NotAllowed` if taken.
inline Error draft(int32_t kit_id) { return act(A_DRAFT, kit_id); }
/// Pick a kit for the teammate in `slot` (draft phase only). Same uniqueness rule as `draft`.
inline Error draft_for(int32_t kit_id, int32_t slot) { return act(A_DRAFT_FOR, kit_id, slot); }
/// Cancel the current channel (plant, defuse, reload, wind-up).
inline Error cancel() { return act(A_CANCEL); }

// ---- world queries ----

/// True if no wall lies between the two points.
inline bool los(Point a, Point b) { return ibc_sys_los(a.x, a.y, b.x, b.y) == 1; }
/// Distance from `p` along `angle` to the first wall, capped at `max`.
inline int32_t raycast(Point p, int32_t angle, int32_t max) { return ibc_sys_raycast(p.x, p.y, norm_angle(angle), max); }

// ---------------------------------------------------------------------------------------------------
// Memory, logging, debug drawing
// ---------------------------------------------------------------------------------------------------

/// Fill `out` from a memory scope (`MEM_*`) starting at byte `off`.
inline Error mem_read(uint32_t scope, uint32_t off, void* out, uint32_t len) { return detail::check(ibc_sys_mem_read(scope, off, out, len)); }
inline Error mem_write(uint32_t scope, uint32_t off, const void* data, uint32_t len) {
    return detail::check(ibc_sys_mem_write(scope, off, data, len));
}
/// Fill `out` from the team scratchpad starting at byte `off`.
inline Error team_mem_read(uint32_t off, void* out, uint32_t len) { return mem_read(MEM_TEAM, off, out, len); }
/// Write to the team scratchpad. Teammates running later this tick already see it.
inline Error team_mem_write(uint32_t off, const void* data, uint32_t len) { return mem_write(MEM_TEAM, off, data, len); }
/// Read `N` little-endian int32s from a memory scope (`MEM_*`; zeros if the read fails).
template <size_t N>
std::array<int32_t, N> mem_get(uint32_t scope, uint32_t off) {
    std::array<int32_t, N> v{};
    if (!ok(mem_read(scope, off, v.data(), static_cast<uint32_t>(N * 4)))) v = {};
    return v;
}
/// Write `N` little-endian int32s to a memory scope (`MEM_*`).
template <size_t N>
Error mem_set(uint32_t scope, uint32_t off, const std::array<int32_t, N>& v) {
    return mem_write(scope, off, v.data(), static_cast<uint32_t>(N * 4));
}
/// Read `N` little-endian int32s from team memory (zeros if the read fails).
template <size_t N>
std::array<int32_t, N> team_mem_get(uint32_t off) { return mem_get<N>(MEM_TEAM, off); }
/// Write `N` little-endian int32s to team memory (action phases only).
template <size_t N>
Error team_mem_set(uint32_t off, const std::array<int32_t, N>& v) { return mem_set<N>(MEM_TEAM, off, v); }

/// Write a line to the match log (shown in the viewer; ignored in tournaments).
inline void log(std::string_view msg) { ibc_sys_log(msg.data(), static_cast<uint32_t>(msg.size())); }
/// printf-style log line (at most 255 bytes). Formatting costs fuel and pulls printf into the wasm:
/// keep it out of hot paths and out of release bots.
__attribute__((format(printf, 1, 2))) inline void logf(const char* fmt, ...) {
    char buf[256];
    va_list ap;
    va_start(ap, fmt);
    const int n = std::vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    if (n > 0) log({buf, static_cast<size_t>(n) < sizeof buf ? static_cast<size_t>(n) : sizeof buf - 1});
}
/// Draw a line in the viewer (`color` is 0xRRGGBB). Free.
inline void debug_line(Point a, Point b, uint32_t color) { ibc_sys_debug_draw(DEBUG_LINE, a.x, a.y, b.x, b.y, color); }
/// Draw a dot in the viewer (`color` is 0xRRGGBB). Free.
inline void debug_dot(Point p, uint32_t color) { ibc_sys_debug_draw(DEBUG_DOT, p.x, p.y, 0, 0, color); }

// ---------------------------------------------------------------------------------------------------
// Navigation: wall grid, cached distance-field pathfinder and a stuck-proof `Navigator`.
// ---------------------------------------------------------------------------------------------------

namespace detail {
/// A fixed-size heap buffer: allocated once, never resized. Out of memory traps.
template <class T>
class Buf {
  public:
    Buf() = default;
    explicit Buf(uint32_t n) : p_(n ? static_cast<T*>(std::malloc(n * sizeof(T))) : nullptr), n_(n) {
        if (n && !p_) __builtin_trap();
    }
    Buf(Buf&& o) noexcept : p_(std::exchange(o.p_, nullptr)), n_(std::exchange(o.n_, 0)) {}
    Buf& operator=(Buf&& o) noexcept {
        if (this != &o) {
            std::free(p_);
            p_ = std::exchange(o.p_, nullptr);
            n_ = std::exchange(o.n_, 0);
        }
        return *this;
    }
    Buf(const Buf&) = delete;
    Buf& operator=(const Buf&) = delete;
    ~Buf() { std::free(p_); }

    T& operator[](uint32_t i) { return p_[i]; }
    const T& operator[](uint32_t i) const { return p_[i]; }
    T* data() { return p_; }
    uint32_t size() const { return n_; }

  private:
    T* p_ = nullptr;
    uint32_t n_ = 0;
};

inline constexpr uint32_t ORTH = 5;
inline constexpr uint32_t DIAG = 7;
struct Step {
    int32_t dx, dy;
    uint32_t cost;
};
/// The 8 neighbours. Orthogonal first.
inline constexpr Step NEIGHBOURS[8] = {
    {1, 0, ORTH}, {0, 1, ORTH}, {-1, 0, ORTH}, {0, -1, ORTH}, {1, 1, DIAG}, {-1, 1, DIAG}, {-1, -1, DIAG}, {1, -1, DIAG},
};
inline constexpr uint8_t ORTH_BITS = 0b0000'1111;
inline constexpr uint32_t UNREACHABLE = UINT32_MAX;
inline constexpr uint32_t NONE = UINT32_MAX;
}  // namespace detail

/// The whole tile grid, fetched once (`height` host calls). Walls never change during a match.
class Grid {
  public:
    int32_t width = 0;
    int32_t height = 0;

    Grid() = default;

    /// Load every map row from the host.
    static Grid load() {
        const MapInfo info = map_info();
        Grid g(info.width > 0 ? info.width : 0, info.height > 0 ? info.height : 0);
        std::memset(g.tiles_.data(), TILE_WALL, g.tiles_.size());
        for (int32_t y = 0; y < g.height; ++y) {
            map_row(y, g.tiles_.data() + y * g.width, static_cast<uint32_t>(g.width));
        }
        g.index_steps();
        return g;
    }

    /// Build from `width * height` raw `TILE_*` values, row by row.
    static Grid from_tiles(int32_t width, int32_t height, const uint8_t* tiles) {
        Grid g(width, height);
        std::memcpy(g.tiles_.data(), tiles, g.tiles_.size());
        g.index_steps();
        return g;
    }

    uint32_t size() const { return tiles_.size(); }
    bool inside(int32_t tx, int32_t ty) const { return tx >= 0 && ty >= 0 && tx < width && ty < height; }
    uint32_t index(int32_t tx, int32_t ty) const { return static_cast<uint32_t>(ty * width + tx); }
    /// `TILE_*` value of a tile; anything outside the map is a wall.
    uint8_t tile(int32_t tx, int32_t ty) const { return inside(tx, ty) ? tiles_[index(tx, ty)] : TILE_WALL; }
    /// `TILE_*` value under a world position.
    uint8_t tile_at(Point p) const {
        const Point t = tile_of(p);
        return tile(t.x, t.y);
    }
    /// Blocks vision and bullets (full wall).
    bool is_wall(int32_t tx, int32_t ty) const { return tile(tx, ty) == TILE_WALL; }
    /// Blocks movement (wall or low cover).
    bool is_blocked(int32_t tx, int32_t ty) const { return tile(tx, ty) != TILE_FLOOR; }
    /// Can a unit step from tile (tx, ty) to the neighbour at (dx, dy)? Diagonals may not cut corners.
    bool can_step(int32_t tx, int32_t ty, int32_t dx, int32_t dy) const {
        return !is_blocked(tx + dx, ty + dy) && (dx == 0 || dy == 0 || (!is_blocked(tx + dx, ty) && !is_blocked(tx, ty + dy)));
    }
    /// Bit `d` is set if a unit may step from tile index `i` to `detail::NEIGHBOURS[d]`.
    uint8_t steps(uint32_t i) const { return steps_[i]; }
    uint8_t tile_index(uint32_t i) const { return tiles_[i]; }

  private:
    Grid(int32_t w, int32_t h) : width(w), height(h), tiles_(static_cast<uint32_t>(w * h)), steps_(static_cast<uint32_t>(w * h)) {}

    void index_steps() {
        for (int32_t ty = 0; ty < height; ++ty) {
            for (int32_t tx = 0; tx < width; ++tx) {
                uint8_t bits = 0;
                for (int d = 0; d < 8; ++d) {
                    bits |= static_cast<uint8_t>(can_step(tx, ty, detail::NEIGHBOURS[d].dx, detail::NEIGHBOURS[d].dy) << d);
                }
                steps_[index(tx, ty)] = bits;
            }
        }
    }

    detail::Buf<uint8_t> tiles_;
    detail::Buf<uint8_t> steps_;
};

/// 8-connected shortest paths over the wall grid, without cutting corners. Each distinct goal tile
/// gets a cached distance field that is grown lazily from the goal until it covers the asking unit,
/// so a query for a known goal costs a handful of array reads and a new goal costs at most
/// `EXPAND_PER_CALL` settled tiles per `next_waypoint` call. Buffers are allocated once per cache slot.
class Pathfinder {
  public:
    /// Distance fields kept; the oldest is evicted first.
    static constexpr uint32_t CACHE = 6;
    /// Tiles a single `next_waypoint` call may settle.
    static constexpr uint32_t EXPAND_PER_CALL = 160;

    /// Path length in world units from `from` to `goal` (tile resolution), or nothing if unreachable.
    /// Always exact, so on a goal that is new and far away it does the whole search in this call:
    /// use it in the buy phase, not mid-fight.
    std::optional<int32_t> path_len(const Grid& grid, Point from, Point goal) {
        const Point f = tile_of(from), g = tile_of(goal);
        if (!grid.inside(g.x, g.y) || grid.is_blocked(f.x, f.y)) return std::nullopt;
        Field& field = field_for(grid, grid.index(g.x, g.y));
        const uint32_t tile = grid.index(f.x, f.y);
        field.settle(grid, tile, UINT32_MAX);
        const uint32_t c = field.cost[tile];
        if (c == detail::UNREACHABLE) return std::nullopt;
        return static_cast<int32_t>(int64_t{c} * TILE / detail::ORTH);
    }

    /// The point to walk towards right now to get from `from` to `goal`: the goal itself when it is in
    /// this tile or the next one, otherwise the centre of the next tile on a shortest path.
    /// Nothing if there is no path, or if the search for a new goal needs more calls to reach `from`
    /// (work per call is capped so a turn never blows its fuel; ask again next tick).
    std::optional<Point> next_waypoint(const Grid& grid, Point from, Point goal) {
        const Point f = tile_of(from), g = tile_of(goal);
        if (!grid.inside(g.x, g.y) || grid.is_blocked(f.x, f.y)) return std::nullopt;
        if (f == g) return goal;
        Field& field = field_for(grid, grid.index(g.x, g.y));
        const uint32_t tile = grid.index(f.x, f.y);
        if (!field.settle(grid, tile, EXPAND_PER_CALL) || field.cost[tile] == detail::UNREACHABLE) return std::nullopt;
        // Every neighbour that is cheaper than this tile is already final.
        const uint32_t here = field.cost[tile];
        uint32_t best_cost = here;
        Point best = f;
        for (int d = 0; d < 8; ++d) {
            if (!(grid.steps(tile) & (1 << d))) continue;
            const detail::Step& s = detail::NEIGHBOURS[d];
            const uint32_t c = field.cost[grid.index(f.x + s.dx, f.y + s.dy)];
            if (c != detail::UNREACHABLE && c + s.cost <= here && c < best_cost) {
                best_cost = c;
                best = {f.x + s.dx, f.y + s.dy};
            }
        }
        return best == g ? goal : tile_centre(best);
    }

  private:
    /// Shortest-path costs from every tile to one goal tile, grown outwards from the goal on demand:
    /// Dijkstra with a bucket queue (steps cost 5 or 7, so 8 buckets are enough). The buckets are
    /// intrusive doubly linked lists through `next`/`prev`, so the queue never allocates.
    struct Field {
        uint32_t goal = detail::NONE;
        detail::Buf<uint32_t> cost, next, prev;
        uint32_t head[8] = {};
        /// Cost level being drained; every tile with `cost <= level` is final.
        uint32_t level = 0;
        uint32_t queued = 0;

        void reset(const Grid& grid, uint32_t goal_tile) {
            if (cost.size() != grid.size()) {
                cost = detail::Buf<uint32_t>(grid.size());
                next = detail::Buf<uint32_t>(grid.size());
                prev = detail::Buf<uint32_t>(grid.size());
            }
            std::memset(cost.data(), 0xff, size_t{cost.size()} * 4);
            for (uint32_t& h : head) h = detail::NONE;
            goal = goal_tile;
            level = 0;
            queued = 1;
            cost[goal] = 0;
            link(goal);
        }

        void link(uint32_t t) {
            uint32_t& h = head[cost[t] & 7];
            next[t] = h;
            prev[t] = detail::NONE;
            if (h != detail::NONE) prev[h] = t;
            h = t;
        }

        void unlink(uint32_t t) {
            const uint32_t p = prev[t], n = next[t];
            if (p != detail::NONE) {
                next[p] = n;
            } else {
                head[cost[t] & 7] = n;
            }
            if (n != detail::NONE) prev[n] = p;
        }

        /// Grow the field until the cost of `tile` is final, settling at most `budget` tiles.
        /// Returns false if the budget ran out first.
        bool settle(const Grid& grid, uint32_t tile, uint32_t budget) {
            int32_t offsets[8];
            for (int d = 0; d < 8; ++d) offsets[d] = detail::NEIGHBOURS[d].dy * grid.width + detail::NEIGHBOURS[d].dx;
            for (;;) {
                if (cost[tile] <= level || queued == 0) return true;
                const uint32_t at = head[level & 7];
                if (at == detail::NONE) {
                    ++level;
                    continue;
                }
                if (budget == 0) return false;
                --budget;
                unlink(at);
                --queued;
                // Stepping is symmetric between floor tiles, so the goal-outwards search can use the forward
                // step masks. A blocked goal still seeds its orthogonal neighbours ("as close as possible").
                uint8_t bits = grid.steps(at);
                if (at == goal && grid.tile_index(at) != TILE_FLOOR) bits &= detail::ORTH_BITS;
                for (int d = 0; d < 8; ++d) {
                    if (!(bits & (1 << d))) continue;
                    const uint32_t n = static_cast<uint32_t>(static_cast<int32_t>(at) + offsets[d]);
                    const uint32_t c = level + detail::NEIGHBOURS[d].cost;
                    if (c < cost[n]) {
                        // A tile with a finite cost above `level` is still queued: move it to its new bucket.
                        if (cost[n] != detail::UNREACHABLE) {
                            unlink(n);
                        } else {
                            ++queued;
                        }
                        cost[n] = c;
                        link(n);
                    }
                }
            }
        }
    };

    Field& field_for(const Grid& grid, uint32_t goal) {
        for (uint32_t i = 0; i < used_; ++i) {
            if (fields_[i].goal == goal && fields_[i].cost.size() == grid.size()) return fields_[i];
        }
        uint32_t slot;
        if (used_ < CACHE) {
            slot = used_++;
        } else {
            slot = evict_;
            evict_ = (evict_ + 1) % CACHE;
        }
        fields_[slot].reset(grid, goal);
        return fields_[slot];
    }

    Field fields_[CACHE];
    uint32_t used_ = 0;
    uint32_t evict_ = 0;
};

/// Grid + pathfinder + stuck detection: call `go` every tick you want to travel somewhere.
class Navigator {
  public:
    Grid grid = Grid::load();
    Pathfinder paths;
    /// Direction of the last move issued by `go` (face it to look where you walk).
    int32_t heading = 0;
    /// Result of the last move issued by `go`.
    Error last_error = Error::Ok;

    /// Loads the map grid from the host.
    Navigator() : max_speed_(core_rules().run_speed) {}

    /// Walk towards `goal` around walls at `speed` (capped at the run speed). Returns true once there
    /// (no move issued), false while travelling. If no path exists it walks straight at the goal.
    bool go(const UnitInfo& me, Point goal, int32_t speed) {
        const Point at = pos(me);
        const int32_t left = dist(at, goal);
        if (left <= ARRIVE) {
            stuck_ = 0;
            return true;
        }
        // Slow down for the last step so we stop on the goal instead of orbiting it.
        const int32_t cap = max_speed_ > 1 ? max_speed_ : 1;
        speed = speed < 1 ? 1 : (speed > cap ? cap : speed);
        const int32_t step = speed < left ? speed : left;
        const int32_t moved_min = step / 8 > 2 ? step / 8 : 2;
        stuck_ = dist(at, last_) < moved_min ? stuck_ + 1 : 0;
        last_ = at;
        const Point way = paths.next_waypoint(grid, at, goal).value_or(goal);
        const int32_t towards = angle_to(at, way);
        if (stuck_ >= STUCK_TICKS && dodge_ == 0) {
            // Blocked by a teammate or a corner: slide sideways for a moment, alternating sides.
            ++flips_;
            const int32_t side = ((me.id + flips_) & 1) == 0 ? 16384 : -16384;
            dodge_angle_ = towards + side + (flips_ % 3 == 0 ? side / 2 : 0);
            dodge_ = DODGE_TICKS;
            stuck_ = 0;
        }
        heading = towards;
        if (dodge_ > 0) {
            --dodge_;
            last_error = move_dir(dodge_angle_, speed);
        } else {
            last_error = move_dir(towards, step);
        }
        return false;
    }

    /// Path length in world units, or nothing if unreachable.
    std::optional<int32_t> path_len(Point from, Point goal) { return paths.path_len(grid, from, goal); }

  private:
    /// Ticks without progress before side-stepping, and how long the side-step lasts.
    static constexpr int32_t STUCK_TICKS = 4;
    static constexpr int32_t DODGE_TICKS = 6;
    /// Closer than this to the goal counts as arrived.
    static constexpr int32_t ARRIVE = 48;

    int32_t max_speed_;
    Point last_{};
    int32_t stuck_ = 0;
    int32_t dodge_ = 0;
    int32_t dodge_angle_ = 0;
    int32_t flips_ = 0;
};

}  // namespace ibc
