// Imperial Battlecode bot ABI, version 0: the C++ mirror of `crates/abi/src/lib.rs` (the source of truth).
//
// Do not edit by taste: every constant and record here is compared against the Rust crate by
// `cargo test -p ibc-cli abi_header` (crates/cli/src/build/cpp.rs), so drift fails CI. The test reads
// this file line by line; keep one `inline constexpr TYPE NAME = VALUE;` per line and one field per line.
//
// Units: positions are fixed-point, `TILE` (1024) per map tile, origin top-left, +x right, +y down.
// Angles are 0..65535 for a full turn, 0 = +x, increasing towards +y (clockwise on screen).
// Records are little-endian, made only of `int32_t`, so they have no padding.
#pragma once

#include <cstdint>

namespace ibc {

inline constexpr uint32_t ABI_VERSION = 0;
inline constexpr char IMPORT_MODULE[] = "ibc";

/// Fixed-point units per tile.
inline constexpr int32_t TILE = 1024;
/// Angle units per full turn.
inline constexpr int32_t FULL_TURN = 65536;

// ---- error codes (positive; `get`/`count` return them negated) ----
inline constexpr int32_t OK = 0;
inline constexpr int32_t ERR_BAD_ARG = 1;
inline constexpr int32_t ERR_BUFFER_TOO_SMALL = 2;
inline constexpr int32_t ERR_UNSUPPORTED = 3;
/// Wrong phase, dead unit, or the unit may not do this now.
inline constexpr int32_t ERR_NOT_ALLOWED = 4;
/// Cooldown, empty magazine, no charges.
inline constexpr int32_t ERR_NOT_READY = 5;
inline constexpr int32_t ERR_OUT_OF_RANGE = 6;
inline constexpr int32_t ERR_INSUFFICIENT_FUNDS = 7;
inline constexpr int32_t ERR_BAD_PTR = 8;

// ---- tables for `count` / `get` ----
/// One `UnitInfo`: this unit, full detail.
inline constexpr uint32_t T_SELF = 1;
/// `UnitInfo` per teammate (always known, like a radar), excluding self.
inline constexpr uint32_t T_ALLIES = 2;
/// `UnitInfo` per enemy currently visible to THIS unit.
inline constexpr uint32_t T_ENEMIES = 3;
/// `SoundInfo` heard by this unit during the previous tick.
inline constexpr uint32_t T_SOUNDS = 4;
/// `ProjectileInfo` visible to this unit.
inline constexpr uint32_t T_PROJECTILES = 5;
/// `EffectInfo` visible to this unit (smoke, fire, barriers, ...).
inline constexpr uint32_t T_EFFECTS = 6;
/// One `MatchInfo`.
inline constexpr uint32_t T_MATCH = 7;
/// One `MapInfo`.
inline constexpr uint32_t T_MAP = 8;
/// Row `index` of the tile grid: `width` bytes, one `TILE_*` value each.
inline constexpr uint32_t T_MAP_ROW = 9;
/// One `CoreRules`.
inline constexpr uint32_t T_RULES_CORE = 10;
/// `WeaponRules` per weapon id.
inline constexpr uint32_t T_WEAPON = 11;
/// `AbilityRules` per ability id.
inline constexpr uint32_t T_ABILITY = 12;
/// `ItemRules` per shop item id.
inline constexpr uint32_t T_ITEM = 13;
/// `SiteInfo` per plant site.
inline constexpr uint32_t T_SITES = 14;
/// `DropInfo` per dropped item visible to this unit (the charge is always visible to attackers).
inline constexpr uint32_t T_DROPS = 15;
/// `KitRules` per kit id.
inline constexpr uint32_t T_KIT = 16;
/// `EventInfo` per public event of the previous tick (kills, plant, defuse, round end).
inline constexpr uint32_t T_EVENTS = 17;

// ---- actions for `act(kind, a, b, c)` ----
/// a = angle, b = speed in units/tick (clamped to the unit's max). Speed above `walk_speed` is a run (noisy).
inline constexpr uint32_t A_MOVE = 1;
/// a = desired facing angle (turn rate limited).
inline constexpr uint32_t A_FACE = 2;
/// a = aim angle. Starts the weapon's wind-up; bullets leave when it ends.
inline constexpr uint32_t A_FIRE = 3;
inline constexpr uint32_t A_RELOAD = 4;
inline constexpr uint32_t A_PLANT = 5;
inline constexpr uint32_t A_DEFUSE = 6;
/// a = ability slot (0 or 1), b = target x, c = target y.
inline constexpr uint32_t A_ABILITY = 7;
/// a = item id. Buy phase only.
inline constexpr uint32_t A_BUY = 8;
/// a = weapon slot to switch to.
inline constexpr uint32_t A_SWAP = 9;
/// a = DROP_* kind.
inline constexpr uint32_t A_DROP = 10;
/// Pick up the nearest drop in reach.
inline constexpr uint32_t A_PICKUP = 11;
/// a = kit id. Draft phase only.
inline constexpr uint32_t A_DRAFT = 12;
/// Cancel the current channel (plant, defuse, reload, wind-up).
inline constexpr uint32_t A_CANCEL = 13;

// ---- enums ----
inline constexpr int32_t TEAM_A = 0;
inline constexpr int32_t TEAM_B = 1;
inline constexpr int32_t SIDE_ATTACK = 0;
inline constexpr int32_t SIDE_DEFEND = 1;

inline constexpr uint8_t TILE_FLOOR = 0;
/// Blocks movement, vision and bullets.
inline constexpr uint8_t TILE_WALL = 1;
/// Blocks movement only.
inline constexpr uint8_t TILE_LOW = 2;

inline constexpr int32_t PHASE_DRAFT = 0;
inline constexpr int32_t PHASE_BUY = 1;
inline constexpr int32_t PHASE_ACTION = 2;
/// The charge is planted and its fuse is running.
inline constexpr int32_t PHASE_PLANTED = 3;
inline constexpr int32_t PHASE_ROUND_END = 4;
inline constexpr int32_t PHASE_MATCH_END = 5;

// UnitInfo.state bit flags
inline constexpr int32_t S_ALIVE = 1 << 0;
inline constexpr int32_t S_WINDUP = 1 << 1;
inline constexpr int32_t S_RELOADING = 1 << 2;
inline constexpr int32_t S_PLANTING = 1 << 3;
inline constexpr int32_t S_DEFUSING = 1 << 4;
inline constexpr int32_t S_BLINDED = 1 << 5;
inline constexpr int32_t S_HAS_CHARGE = 1 << 6;
inline constexpr int32_t S_RUNNING = 1 << 7;
inline constexpr int32_t S_HAS_KIT = 1 << 8;
inline constexpr int32_t S_SLOWED = 1 << 9;
inline constexpr int32_t S_REVEALED = 1 << 10;
/// An ability use is in its startup (`AbilityRules.delay`): it happens when the startup ends unless `A_CANCEL`
/// or death stops it. Until then every primary action except `A_CANCEL` is refused with `ERR_NOT_READY`.
inline constexpr int32_t S_CASTING = 1 << 11;

inline constexpr int32_t SOUND_STEP = 1;
inline constexpr int32_t SOUND_SHOT = 2;
inline constexpr int32_t SOUND_PLANT = 3;
inline constexpr int32_t SOUND_DEFUSE = 4;
inline constexpr int32_t SOUND_ABILITY = 5;
inline constexpr int32_t SOUND_DECOY = 6;

inline constexpr int32_t EFFECT_SMOKE = 1;
inline constexpr int32_t EFFECT_FIRE = 2;
inline constexpr int32_t EFFECT_BARRIER = 3;
inline constexpr int32_t EFFECT_FLASH = 4;
inline constexpr int32_t EFFECT_RECON = 5;
inline constexpr int32_t EFFECT_HEAL = 6;
inline constexpr int32_t EFFECT_DECOY = 7;
inline constexpr int32_t EFFECT_DASH = 8;

inline constexpr int32_t DROP_CHARGE = 1;
inline constexpr int32_t DROP_WEAPON = 2;

inline constexpr int32_t EVENT_KILL = 1;
inline constexpr int32_t EVENT_PLANT = 2;
inline constexpr int32_t EVENT_DEFUSE = 3;
inline constexpr int32_t EVENT_DETONATE = 4;
inline constexpr int32_t EVENT_ROUND_END = 5;
inline constexpr int32_t EVENT_DAMAGE = 6;

inline constexpr int32_t ROUND_WIN_ELIMINATION = 1;
inline constexpr int32_t ROUND_WIN_DETONATION = 2;
inline constexpr int32_t ROUND_WIN_DEFUSE = 3;
inline constexpr int32_t ROUND_WIN_TIMEOUT = 4;

// mem scopes
/// Team scratchpad, read/write all match. Later-running teammates see writes in the same tick.
inline constexpr uint32_t MEM_TEAM = 0;
/// Written in the draft phase, read-only afterwards.
inline constexpr uint32_t MEM_DRAFT = 1;

// debug_draw kinds
/// Line from (x0, y0) to (x1, y1).
inline constexpr uint32_t DEBUG_LINE = 1;
/// Dot at (x0, y0); x1 is the radius (0 = default).
inline constexpr uint32_t DEBUG_DOT = 2;

inline constexpr int32_t ITEM_KIND_WEAPON = 1;
inline constexpr int32_t ITEM_KIND_ARMOR = 2;
inline constexpr int32_t ITEM_KIND_ABILITY_CHARGE = 3;
inline constexpr int32_t ITEM_KIND_DEFUSE_KIT = 4;

// ---- fuel charged for host calls (on top of the wasm instructions of the call itself) ----
inline constexpr uint64_t FUEL_GET = 40;
inline constexpr uint64_t FUEL_COUNT = 10;
inline constexpr uint64_t FUEL_ACT = 20;
inline constexpr uint64_t FUEL_LOS = 60;
inline constexpr uint64_t FUEL_RAYCAST = 80;
inline constexpr uint64_t FUEL_MEM = 20;
inline constexpr uint64_t FUEL_DEBUG = 0;

// ---- records ----

/// A unit as seen by the caller. For enemies, `credits`, `charges`, `cooldown`, `ammo`, `reserve` are 0.
struct UnitInfo {
    int32_t id;
    int32_t team;
    int32_t side;
    int32_t kit;
    int32_t state;
    int32_t x;
    int32_t y;
    int32_t facing;
    /// Velocity of the previous tick in units/tick.
    int32_t vx;
    int32_t vy;
    int32_t hp;
    int32_t armor;
    int32_t weapon;
    int32_t ammo;
    int32_t reserve;
    /// Ticks left in the current channel (wind-up, reload, plant, defuse); 0 if none.
    int32_t channel;
    /// Aim angle of the current wind-up.
    int32_t aim;
    int32_t credits;
    int32_t charges[2];
    int32_t cooldown[2];
    /// Recoil heat, 0 = settled.
    int32_t recoil;
    int32_t blind;
};
static_assert(sizeof(UnitInfo) == 96);

struct SoundInfo {
    int32_t kind;
    /// Approximate source position (snapped to tile centres).
    int32_t x;
    int32_t y;
    int32_t team;
};
static_assert(sizeof(SoundInfo) == 16);

struct ProjectileInfo {
    /// 0 = a bullet (`weapon` says whose); otherwise a thrown ability device, `kind` = ability id + 1 (`T_ABILITY`)
    /// and `weapon` = -1. A thrown device hits nobody: it lands at the first wall or barrier, or after `range`.
    int32_t kind;
    int32_t team;
    int32_t x;
    int32_t y;
    /// Velocity in units/tick.
    int32_t vx;
    int32_t vy;
    int32_t weapon;
    int32_t owner;
};
static_assert(sizeof(ProjectileInfo) == 32);

struct EffectInfo {
    int32_t kind;
    int32_t team;
    int32_t x;
    int32_t y;
    int32_t radius;
    int32_t ticks_left;
    /// Second endpoint for segment effects (barrier); otherwise 0.
    int32_t x2;
    int32_t y2;
};
static_assert(sizeof(EffectInfo) == 32);

struct MatchInfo {
    int32_t tick;
    int32_t round;
    int32_t phase;
    /// Ticks left in the current phase.
    int32_t phase_ticks_left;
    int32_t my_team;
    int32_t my_side;
    int32_t score_mine;
    int32_t score_theirs;
    int32_t team_size;
    int32_t allies_alive;
    int32_t enemies_alive;
    /// 0 = carried, 1 = dropped, 2 = planted.
    int32_t charge_state;
    /// Known to attackers always; to defenders once planted or seen.
    int32_t charge_x;
    int32_t charge_y;
    int32_t fuse_left;
    int32_t rounds_to_win;
    int32_t seed_lo;
    int32_t unit_slot;
};
static_assert(sizeof(MatchInfo) == 72);

struct MapInfo {
    int32_t width;
    int32_t height;
    int32_t sites;
    int32_t team_size;
    /// Spawn centre of the caller's current side, and of the other side.
    int32_t spawn_x;
    int32_t spawn_y;
    int32_t enemy_spawn_x;
    int32_t enemy_spawn_y;
};
static_assert(sizeof(MapInfo) == 32);

struct SiteInfo {
    int32_t id;
    /// Bounding box in tiles, max exclusive.
    int32_t min_tx;
    int32_t min_ty;
    int32_t max_tx;
    int32_t max_ty;
    int32_t centre_x;
    int32_t centre_y;
};
static_assert(sizeof(SiteInfo) == 28);

struct DropInfo {
    int32_t kind;
    int32_t x;
    int32_t y;
    int32_t item;
};
static_assert(sizeof(DropInfo) == 16);

struct EventInfo {
    int32_t kind;
    /// Acting unit id or -1.
    int32_t actor;
    /// Target unit id or -1.
    int32_t target;
    int32_t x;
    int32_t y;
    int32_t value;
};
static_assert(sizeof(EventInfo) == 24);

struct CoreRules {
    int32_t team_size;
    int32_t rounds_to_win;
    int32_t rounds_per_half;
    int32_t buy_ticks;
    int32_t round_ticks;
    int32_t fuse_ticks;
    int32_t plant_ticks;
    int32_t defuse_ticks;
    int32_t defuse_kit_ticks;
    int32_t unit_radius;
    int32_t walk_speed;
    int32_t run_speed;
    /// Max facing change per tick.
    int32_t turn_rate;
    /// Half-angle of the vision cone.
    int32_t fov_half;
    int32_t vision_range;
    /// All-round sensing radius.
    int32_t sense_radius;
    int32_t max_hp;
    int32_t step_sound_radius;
    int32_t shot_sound_radius;
    int32_t substeps;
    int32_t fuel_per_tick;
    int32_t fuel_first_tick_mult;
    int32_t fuel_buy_mult;
    int32_t team_mem_bytes;
    int32_t draft_mem_bytes;
    int32_t pickup_reach;
    int32_t interact_reach;
    int32_t weapons;
    int32_t abilities;
    int32_t items;
    int32_t kits;
    int32_t max_ticks;
};
static_assert(sizeof(CoreRules) == 128);

struct WeaponRules {
    int32_t id;
    int32_t price;
    int32_t damage;
    /// Bullets per trigger pull.
    int32_t pellets;
    /// Angle between adjacent pellets when standing still and settled.
    int32_t spread;
    /// Extra spread per unit of speed (per 1024 units/tick), and per recoil point.
    int32_t move_spread;
    int32_t recoil_spread;
    int32_t recoil_per_shot;
    int32_t recoil_recovery;
    /// Ticks between `A_FIRE` and the bullets leaving.
    int32_t windup;
    /// Ticks after a shot before the next wind-up may start.
    int32_t cooldown;
    int32_t magazine;
    int32_t reload_ticks;
    /// Units per tick.
    int32_t bullet_speed;
    int32_t range;
    /// Damage is full up to `falloff_start`, then scales linearly to `falloff_min_pct` at `range`.
    int32_t falloff_start;
    int32_t falloff_min_pct;
    /// Percent of max run speed while holding this weapon.
    int32_t move_pct;
    int32_t kill_reward;
    int32_t slot;
};
static_assert(sizeof(WeaponRules) == 80);

struct AbilityRules {
    int32_t id;
    int32_t effect;
    int32_t price;
    int32_t max_charges;
    int32_t cooldown;
    int32_t range;
    int32_t radius;
    int32_t duration;
    int32_t power;
    /// Startup: ticks between `A_ABILITY` and the ability happening (0 = in the tick it is used). See `S_CASTING`.
    int32_t delay;
    /// Units per tick a thrown device flies (a projectile in `T_PROJECTILES`); 0 = it appears where it lands at
    /// once. Only smoke, flash, incendiary, recon and decoy are thrown.
    int32_t speed;
};
static_assert(sizeof(AbilityRules) == 44);

struct ItemRules {
    int32_t id;
    int32_t kind;
    int32_t price;
    /// Weapon id, armour amount, or ability slot depending on `kind`.
    int32_t value;
};
static_assert(sizeof(ItemRules) == 16);

struct KitRules {
    int32_t id;
    int32_t abilities[2];
    int32_t hp;
    int32_t speed_pct;
};
static_assert(sizeof(KitRules) == 20);

}  // namespace ibc

// ---- raw host imports (wasm module "ibc") ----
// Prefixed `ibc_sys_` on the C++ side so `log` does not collide with <cmath>; the wasm import names are the ABI's.
#define IBC_IMPORT(name) __attribute__((import_module("ibc"), import_name(#name)))

extern "C" {
/// Yield until this unit's next tick.
IBC_IMPORT(end_turn) void ibc_sys_end_turn(void);
/// Fuel left in this tick's budget.
IBC_IMPORT(fuel_remaining) int64_t ibc_sys_fuel_remaining(void);
/// Rows in a table (>= 0) or -error.
IBC_IMPORT(count) int32_t ibc_sys_count(uint32_t table);
/// Copy one row; bytes written (>= 0) or -error.
IBC_IMPORT(get) int32_t ibc_sys_get(uint32_t table, uint32_t index, void* out, uint32_t cap);
/// Queue an intent; 0 or an error code (positive).
IBC_IMPORT(act) int32_t ibc_sys_act(uint32_t kind, int32_t a, int32_t b, int32_t c);
/// 1 if no wall lies between the points (static map only).
IBC_IMPORT(los) int32_t ibc_sys_los(int32_t x0, int32_t y0, int32_t x1, int32_t y1);
/// Distance to the first wall, capped at `max`.
IBC_IMPORT(raycast) int32_t ibc_sys_raycast(int32_t x, int32_t y, int32_t angle, int32_t max);
/// 0 or an error code (positive), like `act`.
IBC_IMPORT(mem_read) int32_t ibc_sys_mem_read(uint32_t scope, uint32_t off, void* ptr, uint32_t len);
IBC_IMPORT(mem_write) int32_t ibc_sys_mem_write(uint32_t scope, uint32_t off, const void* ptr, uint32_t len);
IBC_IMPORT(debug_draw)
int32_t ibc_sys_debug_draw(uint32_t kind, int32_t x0, int32_t y0, int32_t x1, int32_t y1, uint32_t color);
IBC_IMPORT(log) int32_t ibc_sys_log(const void* ptr, uint32_t len);
}

#undef IBC_IMPORT
