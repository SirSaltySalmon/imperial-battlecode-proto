//! Imperial Battlecode bot ABI, version 0. FROZEN: additive changes only (new ids at the end,
//! new fields at the end of a record). See docs/ABI.md.
//!
//! Wasm bots import these functions from module `"ibc"`:
//!
//! ```text
//! end_turn()                                            yield until the next tick
//! fuel_remaining() -> i64                               fuel left in this tick's budget
//! count(table: u32) -> i32                              rows in a table (>= 0) or -error
//! get(table: u32, index: u32, out: *mut u8, cap: u32) -> i32   bytes written (>= 0, at most cap) or -error
//! act(kind: u32, a: i32, b: i32, c: i32) -> i32         0 or error code (positive)
//! los(x0: i32, y0: i32, x1: i32, y1: i32) -> i32        1 if no wall between the points
//! raycast(x: i32, y: i32, angle: i32, max: i32) -> i32  distance to the first wall, capped at max
//! mem_read(scope: u32, off: u32, ptr: *mut u8, len: u32) -> i32     0 or error code (positive), like `act`
//! mem_write(scope: u32, off: u32, ptr: *const u8, len: u32) -> i32
//! debug_draw(kind: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> i32
//! log(ptr: *const u8, len: u32) -> i32
//! ```
//!
//! Units: positions are fixed-point, `TILE` (1024) per map tile, origin top-left, +x right, +y down.
//! Angles are 0..65535 for a full turn, 0 = +x, increasing towards +y (clockwise on screen).
//! Records are `#[repr(C)]`, little-endian, made only of `i32`, so they have no padding.
#![cfg_attr(not(feature = "host"), no_std)]

pub const ABI_VERSION: u32 = 0;
pub const IMPORT_MODULE: &str = "ibc";

/// Fixed-point units per tile.
pub const TILE: i32 = 1024;
/// Angle units per full turn.
pub const FULL_TURN: i32 = 65536;

// ---- error codes (positive; `get`/`count` return them negated) ----
pub const OK: i32 = 0;
pub const ERR_BAD_ARG: i32 = 1;
pub const ERR_BUFFER_TOO_SMALL: i32 = 2;
pub const ERR_UNSUPPORTED: i32 = 3;
/// Wrong phase, dead unit, or the unit may not do this now.
pub const ERR_NOT_ALLOWED: i32 = 4;
/// Cooldown, empty magazine, no charges.
pub const ERR_NOT_READY: i32 = 5;
pub const ERR_OUT_OF_RANGE: i32 = 6;
pub const ERR_INSUFFICIENT_FUNDS: i32 = 7;
pub const ERR_BAD_PTR: i32 = 8;

// ---- tables for `count` / `get` ----
/// One `UnitInfo`: this unit, full detail.
pub const T_SELF: u32 = 1;
/// `UnitInfo` per teammate (always known, like a radar), excluding self.
pub const T_ALLIES: u32 = 2;
/// `UnitInfo` per living enemy currently seen by THIS unit's team (vision is shared by the team). Enemy rows
/// hide hp, armour, credits, ammo, charges, cooldowns and the defuse kit; the weapon in hand is shown.
pub const T_ENEMIES: u32 = 3;
/// `SoundInfo` heard by any living unit of THIS unit's team during the previous tick (shared by the team).
pub const T_SOUNDS: u32 = 4;
/// `ProjectileInfo` seen by this unit's team (own team's bullets always).
pub const T_PROJECTILES: u32 = 5;
/// `EffectInfo` seen by this unit's team (smoke, fire, barriers, ...; own team's effects always).
pub const T_EFFECTS: u32 = 6;
/// One `MatchInfo`.
pub const T_MATCH: u32 = 7;
/// One `MapInfo`.
pub const T_MAP: u32 = 8;
/// Row `index` of the tile grid: `width` bytes, one `TILE_*` value each.
pub const T_MAP_ROW: u32 = 9;
/// One `CoreRules`.
pub const T_RULES_CORE: u32 = 10;
/// `WeaponRules` per weapon id.
pub const T_WEAPON: u32 = 11;
/// `AbilityRules` per ability id.
pub const T_ABILITY: u32 = 12;
/// `ItemRules` per shop item id.
pub const T_ITEM: u32 = 13;
/// `SiteInfo` per plant site.
pub const T_SITES: u32 = 14;
/// `DropInfo` per dropped item seen by this unit's team (the charge is always visible to attackers).
pub const T_DROPS: u32 = 15;
/// `KitRules` per kit id.
pub const T_KIT: u32 = 16;
/// `EventInfo` per event of the previous tick that this unit's team receives. Events follow the team's sight;
/// kills, plant, defuse, detonation and round end always arrive, with `x`/`y` set to `POS_HIDDEN` when the
/// team does not know the position. See docs/ABI.md for each kind.
pub const T_EVENTS: u32 = 17;
/// `RosterInfo` per unit of both teams, in unit id order. Readable in every phase.
pub const T_ROSTER: u32 = 18;
/// `RoundInfo` per finished round of the match, oldest first.
pub const T_ROUNDS: u32 = 19;

// ---- actions for `act(kind, a, b, c)` ----
/// a = angle, b = speed in units/tick (clamped to the unit's max). Speed above `walk_speed` is a run (noisy).
pub const A_MOVE: u32 = 1;
/// a = desired facing angle (turn rate limited).
pub const A_FACE: u32 = 2;
/// a = aim angle. Starts the weapon's startup (`windup` plus the unit's unsteadiness); bullets leave when it
/// ends and the unit is then braced: while it keeps firing and stays put, each pull leaves on the tick it is
/// made. Refused with `ERR_NOT_ALLOWED` on a tick the unit also moves: a move cancels a startup and the brace.
pub const A_FIRE: u32 = 3;
pub const A_RELOAD: u32 = 4;
pub const A_PLANT: u32 = 5;
pub const A_DEFUSE: u32 = 6;
/// a = ability slot (0 or 1), b = target x, c = target y.
pub const A_ABILITY: u32 = 7;
/// a = item id. Buy phase only.
pub const A_BUY: u32 = 8;
/// a = weapon slot to switch to.
pub const A_SWAP: u32 = 9;
/// a = DROP_* kind.
pub const A_DROP: u32 = 10;
/// Pick up the nearest drop in reach.
pub const A_PICKUP: u32 = 11;
/// a = kit id. Draft phase only.
pub const A_DRAFT: u32 = 12;
/// Cancel the current channel (plant, defuse, reload, wind-up).
pub const A_CANCEL: u32 = 13;
/// a = item id, b = slot of the teammate who gets it (`MatchInfo.unit_slot`; your own slot works too).
/// Buy phase only. The caller pays.
pub const A_BUY_FOR: u32 = 14;
/// a = kit id, b = slot of the teammate it is drafted for. Draft phase only.
pub const A_DRAFT_FOR: u32 = 15;

// ---- enums ----
pub const TEAM_A: i32 = 0;
pub const TEAM_B: i32 = 1;
pub const SIDE_ATTACK: i32 = 0;
pub const SIDE_DEFEND: i32 = 1;

pub const TILE_FLOOR: u8 = 0;
/// Blocks movement, vision and bullets.
pub const TILE_WALL: u8 = 1;
/// Blocks movement only.
pub const TILE_LOW: u8 = 2;

pub const PHASE_DRAFT: i32 = 0;
pub const PHASE_BUY: i32 = 1;
pub const PHASE_ACTION: i32 = 2;
/// The charge is planted and its fuse is running.
pub const PHASE_PLANTED: i32 = 3;
pub const PHASE_ROUND_END: i32 = 4;
pub const PHASE_MATCH_END: i32 = 5;

// UnitInfo.state bit flags
pub const S_ALIVE: i32 = 1 << 0;
pub const S_WINDUP: i32 = 1 << 1;
pub const S_RELOADING: i32 = 1 << 2;
pub const S_PLANTING: i32 = 1 << 3;
pub const S_DEFUSING: i32 = 1 << 4;
pub const S_BLINDED: i32 = 1 << 5;
pub const S_HAS_CHARGE: i32 = 1 << 6;
pub const S_RUNNING: i32 = 1 << 7;
pub const S_HAS_KIT: i32 = 1 << 8;
pub const S_SLOWED: i32 = 1 << 9;
pub const S_REVEALED: i32 = 1 << 10;
/// Startup done and still firing: the next pull leaves on the tick it is made.
pub const S_BRACED: i32 = 1 << 11;

pub const SOUND_STEP: i32 = 1;
pub const SOUND_SHOT: i32 = 2;
pub const SOUND_PLANT: i32 = 3;
pub const SOUND_DEFUSE: i32 = 4;
pub const SOUND_ABILITY: i32 = 5;
pub const SOUND_DECOY: i32 = 6;

pub const EFFECT_SMOKE: i32 = 1;
pub const EFFECT_FIRE: i32 = 2;
pub const EFFECT_BARRIER: i32 = 3;
pub const EFFECT_FLASH: i32 = 4;
pub const EFFECT_RECON: i32 = 5;
pub const EFFECT_HEAL: i32 = 6;
pub const EFFECT_DECOY: i32 = 7;
/// Ability effect id only (an instant move); never appears in `T_EFFECTS`.
pub const EFFECT_DASH: i32 = 8;

pub const DROP_CHARGE: i32 = 1;
pub const DROP_WEAPON: i32 = 2;

/// actor = killer, or -1 for a death nobody caused (the detonation blast); target = victim; value = weapon id,
/// or -1 when no weapon did it.
pub const EVENT_KILL: i32 = 1;
pub const EVENT_PLANT: i32 = 2;
pub const EVENT_DEFUSE: i32 = 3;
pub const EVENT_DETONATE: i32 = 4;
pub const EVENT_ROUND_END: i32 = 5;
pub const EVENT_DAMAGE: i32 = 6;

/// `EventInfo.x` and `.y` of an event whose position the receiving team does not know. Never a real
/// position: check for it before using the coordinates.
pub const POS_HIDDEN: i32 = -2147483648;

pub const ROUND_WIN_ELIMINATION: i32 = 1;
pub const ROUND_WIN_DETONATION: i32 = 2;
pub const ROUND_WIN_DEFUSE: i32 = 3;
pub const ROUND_WIN_TIMEOUT: i32 = 4;

// mem scopes
/// Team scratchpad, kept all match. Readable always, writable only in the action phases (`PHASE_ACTION`,
/// `PHASE_PLANTED`). Later-running teammates see writes in the same tick.
pub const MEM_TEAM: u32 = 0;
/// Written in the draft phase, read-only afterwards.
pub const MEM_DRAFT: u32 = 1;
/// Buy-phase team scratchpad: zeroed at every round start, writable only in the buy phase, readable always.
pub const MEM_BUY: u32 = 2;

// debug_draw kinds
/// Line from (x0, y0) to (x1, y1).
pub const DEBUG_LINE: u32 = 1;
/// Dot at (x0, y0); x1 is the radius (0 = default).
pub const DEBUG_DOT: u32 = 2;

pub const ITEM_KIND_WEAPON: i32 = 1;
pub const ITEM_KIND_ARMOR: i32 = 2;
pub const ITEM_KIND_ABILITY_CHARGE: i32 = 3;
pub const ITEM_KIND_DEFUSE_KIT: i32 = 4;

// ---- fuel charged for host calls (on top of the wasm instructions of the call itself) ----
pub const FUEL_GET: u64 = 40;
pub const FUEL_COUNT: u64 = 10;
pub const FUEL_ACT: u64 = 20;
pub const FUEL_LOS: u64 = 60;
pub const FUEL_RAYCAST: u64 = 80;
pub const FUEL_MEM: u64 = 20;
pub const FUEL_DEBUG: u64 = 0;

// ---- records ----

/// A unit as seen by the caller. For enemies, `hp`, `armor`, `credits`, `charges`, `cooldown`, `ammo`, `reserve`,
/// `blind`, `other_weapon` and `fire_cooldown` are 0.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnitInfo {
    pub id: i32,
    pub team: i32,
    pub side: i32,
    pub kit: i32,
    pub state: i32,
    pub x: i32,
    pub y: i32,
    pub facing: i32,
    /// Velocity of the previous tick in units/tick: the distance actually moved, so a move into a wall reads 0.
    pub vx: i32,
    pub vy: i32,
    pub hp: i32,
    pub armor: i32,
    pub weapon: i32,
    pub ammo: i32,
    pub reserve: i32,
    /// Ticks left in the current channel (wind-up, reload, plant, defuse); 0 if none.
    pub channel: i32,
    /// Aim angle of the current wind-up.
    pub aim: i32,
    pub credits: i32,
    pub charges: [i32; 2],
    pub cooldown: [i32; 2],
    /// Recoil heat, 0 = settled.
    pub recoil: i32,
    pub blind: i32,
    /// Unsteadiness heat, 0 = steady: builds while moving, decays standing, lengthens the next startup.
    pub unsteadiness: i32,
    /// Weapon id in the slot not in hand, -1 if that slot is empty.
    pub other_weapon: i32,
    /// Ticks before the weapon in hand may start a new wind-up (0 = ready, unless reloading or empty).
    pub fire_cooldown: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoundInfo {
    pub kind: i32,
    /// Approximate source position (snapped to tile centres).
    pub x: i32,
    pub y: i32,
    pub team: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectileInfo {
    pub kind: i32,
    pub team: i32,
    pub x: i32,
    pub y: i32,
    /// Velocity in units/tick.
    pub vx: i32,
    pub vy: i32,
    pub weapon: i32,
    pub owner: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EffectInfo {
    pub kind: i32,
    pub team: i32,
    pub x: i32,
    pub y: i32,
    pub radius: i32,
    pub ticks_left: i32,
    /// Second endpoint for segment effects (barrier); otherwise 0.
    pub x2: i32,
    pub y2: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchInfo {
    pub tick: i32,
    pub round: i32,
    pub phase: i32,
    /// Ticks left in the current phase.
    pub phase_ticks_left: i32,
    pub my_team: i32,
    pub my_side: i32,
    pub score_mine: i32,
    pub score_theirs: i32,
    pub team_size: i32,
    pub allies_alive: i32,
    pub enemies_alive: i32,
    /// 0 = carried, 1 = dropped, 2 = planted.
    pub charge_state: i32,
    /// Known to attackers always; to defenders once planted or seen.
    pub charge_x: i32,
    pub charge_y: i32,
    pub fuse_left: i32,
    pub rounds_to_win: i32,
    pub seed_lo: i32,
    pub unit_slot: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapInfo {
    pub width: i32,
    pub height: i32,
    pub sites: i32,
    pub team_size: i32,
    /// Spawn centre of the caller's current side, and of the other side.
    pub spawn_x: i32,
    pub spawn_y: i32,
    pub enemy_spawn_x: i32,
    pub enemy_spawn_y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SiteInfo {
    pub id: i32,
    /// Bounding box in tiles, max exclusive.
    pub min_tx: i32,
    pub min_ty: i32,
    pub max_tx: i32,
    pub max_ty: i32,
    pub centre_x: i32,
    pub centre_y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropInfo {
    pub kind: i32,
    pub x: i32,
    pub y: i32,
    pub item: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventInfo {
    pub kind: i32,
    /// Acting unit id or -1.
    pub actor: i32,
    /// Target unit id or -1.
    pub target: i32,
    /// Where it happened, or `POS_HIDDEN` in both `x` and `y` when the receiving team does not know.
    pub x: i32,
    pub y: i32,
    pub value: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreRules {
    pub team_size: i32,
    pub rounds_to_win: i32,
    pub rounds_per_half: i32,
    pub buy_ticks: i32,
    pub round_ticks: i32,
    pub fuse_ticks: i32,
    pub plant_ticks: i32,
    pub defuse_ticks: i32,
    pub defuse_kit_ticks: i32,
    pub unit_radius: i32,
    pub walk_speed: i32,
    pub run_speed: i32,
    /// Max facing change per tick.
    pub turn_rate: i32,
    /// Half-angle of the vision cone.
    pub fov_half: i32,
    pub vision_range: i32,
    /// All-round sensing radius.
    pub sense_radius: i32,
    pub max_hp: i32,
    pub step_sound_radius: i32,
    pub shot_sound_radius: i32,
    pub substeps: i32,
    pub fuel_per_tick: i32,
    pub fuel_first_tick_mult: i32,
    pub fuel_buy_mult: i32,
    pub team_mem_bytes: i32,
    pub draft_mem_bytes: i32,
    pub pickup_reach: i32,
    pub interact_reach: i32,
    pub weapons: i32,
    pub abilities: i32,
    pub items: i32,
    pub kits: i32,
    pub max_ticks: i32,
    /// On detonation every living unit within this distance of the charge dies, through walls.
    pub blast_radius: i32,
    pub buy_mem_bytes: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeaponRules {
    pub id: i32,
    pub price: i32,
    pub damage: i32,
    /// Bullets per trigger pull.
    pub pellets: i32,
    /// Angle between adjacent pellets when standing still and settled.
    pub spread: i32,
    /// Reserved, always 0 (was `move_spread`: movement no longer adds spread).
    pub _reserved0: i32,
    /// Extra spread between pellets per recoil point.
    pub recoil_spread: i32,
    pub recoil_per_shot: i32,
    pub recoil_recovery: i32,
    /// Ticks between `A_FIRE` and the bullets leaving for a steady unit that is not braced.
    pub windup: i32,
    /// Ticks after a shot before the next pull is accepted.
    pub cooldown: i32,
    pub magazine: i32,
    pub reload_ticks: i32,
    /// Units per tick.
    pub bullet_speed: i32,
    pub range: i32,
    /// Damage is full up to `falloff_start`, then scales linearly to `falloff_min_pct` at `range`.
    pub falloff_start: i32,
    pub falloff_min_pct: i32,
    /// Percent of max run speed while holding this weapon.
    pub move_pct: i32,
    pub kill_reward: i32,
    pub slot: i32,
    /// Unsteadiness gained per tick moved, per 1024 units/tick of distance actually covered.
    pub unsteady_gain: i32,
    /// Unsteadiness lost per tick without moving.
    pub unsteady_decay: i32,
    /// Unsteadiness never exceeds this.
    pub unsteady_max: i32,
    /// Startup added per point of unsteadiness, in hundredths of a tick (rounded half up).
    pub startup_per_point: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AbilityRules {
    pub id: i32,
    pub effect: i32,
    pub price: i32,
    pub max_charges: i32,
    pub cooldown: i32,
    pub range: i32,
    pub radius: i32,
    pub duration: i32,
    pub power: i32,
    /// Ticks between use and the effect starting.
    pub delay: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemRules {
    pub id: i32,
    pub kind: i32,
    pub price: i32,
    /// Weapon id, armour amount, or ability slot depending on `kind`.
    pub value: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KitRules {
    pub id: i32,
    pub abilities: [i32; 2],
    pub hp: i32,
    pub speed_pct: i32,
}

/// One unit in `T_ROSTER`. Public to both teams: kits and round-start credits, not equipment.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RosterInfo {
    pub id: i32,
    pub team: i32,
    pub kit: i32,
    /// Credits when this round's buy phase began (0 during the draft).
    pub credits: i32,
    /// 1 if the unit is alive.
    pub alive: i32,
}

/// One finished round in `T_ROUNDS`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoundInfo {
    pub round: i32,
    /// Winning team (`TEAM_A` / `TEAM_B`).
    pub winner: i32,
    /// How it ended: `ROUND_WIN_*`.
    pub reason: i32,
    /// Team that attacked in this round.
    pub attackers: i32,
    /// Ticks from the start of the buy phase to the end of the round.
    pub ticks: i32,
    /// Score after this round.
    pub score_a: i32,
    pub score_b: i32,
}

/// Bytes of a record. Sound only for the `repr(C)` all-`i32` records in this crate.
pub fn bytes_of<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: records here are repr(C), contain only i32 and so have no padding or invalid bit patterns.
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
}

/// Read a record from bytes (unaligned). Returns `None` if `b` is too short.
pub fn from_bytes<T: Copy + Default>(b: &[u8]) -> Option<T> {
    if b.len() < core::mem::size_of::<T>() {
        return None;
    }
    // SAFETY: as in `bytes_of`; read_unaligned tolerates any alignment.
    Some(unsafe { core::ptr::read_unaligned(b.as_ptr() as *const T) })
}

/// The game as a host sees it. Implemented by the engine; consumed by the wasm runner and the native sim,
/// so both paths go through exactly the same calls. `unit` is the calling unit id.
#[cfg(feature = "host")]
pub trait Host {
    fn count(&self, unit: u32, table: u32) -> i32;
    /// Appends the row to `out` and returns bytes written, or a negative error.
    fn get(&self, unit: u32, table: u32, index: u32, out: &mut Vec<u8>) -> i32;
    fn act(&mut self, unit: u32, kind: u32, a: i32, b: i32, c: i32) -> i32;
    fn los(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> i32;
    fn raycast(&self, x: i32, y: i32, angle: i32, max: i32) -> i32;
    fn mem_read(&self, unit: u32, scope: u32, off: u32, out: &mut [u8]) -> i32;
    fn mem_write(&mut self, unit: u32, scope: u32, off: u32, data: &[u8]) -> i32;
    fn debug_draw(&mut self, unit: u32, kind: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> i32;
}
