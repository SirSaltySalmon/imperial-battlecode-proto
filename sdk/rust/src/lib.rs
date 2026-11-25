//! `ibc`: the Rust bot SDK for Imperial Battlecode.
//!
//! The same bot code runs as a `wasm32-wasip1` binary (host calls are wasm imports from module `"ibc"`)
//! and natively inside the simulator (`ibc::native::with_host`). See `README.md` for a quickstart.
//!
//! Every constant and record of the frozen ABI (`ibc-abi`) is re-exported at the crate root.

pub mod geom;
pub mod nav;
mod sys;
pub mod tactics;

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

pub use geom::{angle_diff, angle_to, cos_sin, dist, dist2, isqrt, lead, len, norm_angle, offset};
pub use ibc_abi as abi;
pub use ibc_abi::*;
pub use nav::{tile_centre, tile_of, Grid, Navigator, Pathfinder};

use core::marker::PhantomData;

// ---- bot entry ----

/// A bot: one instance per unit, alive for the whole match. `turn` is called once per tick.
pub trait Bot {
    fn new() -> Self
    where
        Self: Sized;
    fn turn(&mut self);
}

/// Defines `main` for a bot binary: `ibc::bot!(MyBot);` in `src/main.rs`.
#[macro_export]
macro_rules! bot {
    ($bot:ty) => {
        #[cfg(target_arch = "wasm32")]
        fn main() {
            let mut bot = <$bot as $crate::Bot>::new();
            loop {
                $crate::Bot::turn(&mut bot);
                $crate::end_turn();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        fn main() {
            // Bots only run as wasm or inside the native simulator; keep the type checked here.
            let _ = <$bot as $crate::Bot>::new;
            eprintln!("this is an Imperial Battlecode bot: build it with --target wasm32-wasip1");
        }
    };
}

/// Finish this tick. Called for you by `bot!`; a no-op natively.
pub fn end_turn() {
    sys::sys_end_turn()
}

/// Fuel left in this tick's budget (`i64::MAX` natively).
pub fn fuel_remaining() -> i64 {
    sys::sys_fuel_remaining()
}

// ---- errors ----

/// A positive ABI error code (`ERR_*`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Error(pub i32);

impl Error {
    pub const BAD_ARG: Error = Error(ERR_BAD_ARG);
    pub const BUFFER_TOO_SMALL: Error = Error(ERR_BUFFER_TOO_SMALL);
    pub const UNSUPPORTED: Error = Error(ERR_UNSUPPORTED);
    pub const NOT_ALLOWED: Error = Error(ERR_NOT_ALLOWED);
    pub const NOT_READY: Error = Error(ERR_NOT_READY);
    pub const OUT_OF_RANGE: Error = Error(ERR_OUT_OF_RANGE);
    pub const INSUFFICIENT_FUNDS: Error = Error(ERR_INSUFFICIENT_FUNDS);
    pub const BAD_PTR: Error = Error(ERR_BAD_PTR);

    pub fn code(self) -> i32 {
        self.0
    }

    pub fn name(self) -> &'static str {
        match self.0 {
            ERR_BAD_ARG => "bad argument",
            ERR_BUFFER_TOO_SMALL => "buffer too small",
            ERR_UNSUPPORTED => "unsupported",
            ERR_NOT_ALLOWED => "not allowed",
            ERR_NOT_READY => "not ready",
            ERR_OUT_OF_RANGE => "out of range",
            ERR_INSUFFICIENT_FUNDS => "insufficient funds",
            ERR_BAD_PTR => "bad pointer",
            _ => "unknown error",
        }
    }
}

impl core::fmt::Debug for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Error({}: {})", self.0, self.name())
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for Error {}

#[inline]
fn check(code: i32) -> Result<(), Error> {
    if code == OK {
        Ok(())
    } else {
        Err(Error(code.abs()))
    }
}

// ---- tables ----

mod sealed {
    /// ABI records: `repr(C)`, all `i32`, any bit pattern valid.
    pub trait Record: Copy + Default {}
}
use sealed::Record;

macro_rules! records { ($($t:ty),*) => { $(impl Record for $t {})* } }
records!(UnitInfo, SoundInfo, ProjectileInfo, EffectInfo, MatchInfo, MapInfo, SiteInfo, DropInfo, EventInfo);
records!(CoreRules, WeaponRules, AbilityRules, ItemRules, KitRules, RosterInfo, RoundInfo);

fn read<T: Record>(table: u32, index: u32) -> Option<T> {
    let mut v = T::default();
    // SAFETY: `Record` types are plain i32 records, so any bytes the host writes are a valid value.
    let buf = unsafe { core::slice::from_raw_parts_mut(&mut v as *mut T as *mut u8, core::mem::size_of::<T>()) };
    (sys::sys_get(table, index, buf) >= 0).then_some(v)
}

/// Zero-allocation iterator over the rows of a host table. One host call per row, made lazily.
#[derive(Clone)]
pub struct Rows<T> {
    table: u32,
    next: u32,
    len: u32,
    _row: PhantomData<T>,
}

fn rows<T: Record>(table: u32) -> Rows<T> {
    Rows { table, next: 0, len: sys::sys_count(table).max(0) as u32, _row: PhantomData }
}

impl<T: Record> Iterator for Rows<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        while self.next < self.len {
            self.next += 1;
            if let Some(v) = read(self.table, self.next - 1) {
                return Some(v);
            }
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some((self.len - self.next) as usize))
    }
}

impl<T> Rows<T> {
    /// Rows in the table (one cheap host call was already made for this).
    pub fn total(&self) -> usize {
        self.len as usize
    }
}

/// This unit, full detail.
pub fn me() -> UnitInfo {
    read(T_SELF, 0).unwrap_or_default()
}
/// Teammates (always known), excluding this unit. Includes dead ones: check `alive()`.
pub fn allies() -> Rows<UnitInfo> {
    rows(T_ALLIES)
}
/// Enemies THIS unit can see right now.
pub fn enemies() -> Rows<UnitInfo> {
    rows(T_ENEMIES)
}
/// Sounds heard during the previous tick.
pub fn sounds() -> Rows<SoundInfo> {
    rows(T_SOUNDS)
}
pub fn projectiles() -> Rows<ProjectileInfo> {
    rows(T_PROJECTILES)
}
pub fn effects() -> Rows<EffectInfo> {
    rows(T_EFFECTS)
}
pub fn drops() -> Rows<DropInfo> {
    rows(T_DROPS)
}
/// Public events of the previous tick.
pub fn events() -> Rows<EventInfo> {
    rows(T_EVENTS)
}
pub fn sites() -> Rows<SiteInfo> {
    rows(T_SITES)
}
/// Every unit of both teams, by id: kit, credits at the start of this round's buy phase, alive.
pub fn roster() -> Rows<RosterInfo> {
    rows(T_ROSTER)
}
/// Every finished round so far, oldest first: winner, how it ended, who attacked, score after it.
pub fn rounds() -> Rows<RoundInfo> {
    rows(T_ROUNDS)
}
pub fn match_info() -> MatchInfo {
    read(T_MATCH, 0).unwrap_or_default()
}
pub fn map_info() -> MapInfo {
    read(T_MAP, 0).unwrap_or_default()
}
/// Global numbers. Constant for the whole match: fetch once in `Bot::new`.
pub fn core_rules() -> CoreRules {
    read(T_RULES_CORE, 0).unwrap_or_default()
}
pub fn weapon(id: i32) -> Option<WeaponRules> {
    read(T_WEAPON, id as u32)
}
/// Rules of an ability id (the action is `use_ability`).
pub fn ability(id: i32) -> Option<AbilityRules> {
    read(T_ABILITY, id as u32)
}
pub fn item(id: i32) -> Option<ItemRules> {
    read(T_ITEM, id as u32)
}
pub fn kit(id: i32) -> Option<KitRules> {
    read(T_KIT, id as u32)
}
/// All shop items.
pub fn items() -> Rows<ItemRules> {
    rows(T_ITEM)
}
/// All weapons.
pub fn weapons() -> Rows<WeaponRules> {
    rows(T_WEAPON)
}
/// Copy row `y` of the tile grid (`width` bytes of `TILE_*`) into `out`; returns bytes written.
pub fn map_row(y: i32, out: &mut [u8]) -> Result<usize, Error> {
    let n = sys::sys_get(T_MAP_ROW, y as u32, out);
    if n >= 0 {
        Ok(n as usize)
    } else {
        Err(Error(-n))
    }
}

/// Convenience predicates on `UnitInfo`.
pub trait UnitExt {
    fn has(&self, state_flag: i32) -> bool;
    fn alive(&self) -> bool;
    fn pos(&self) -> (i32, i32);
    /// Winding up, reloading, planting or defusing.
    fn busy(&self) -> bool;
}

impl UnitExt for UnitInfo {
    #[inline]
    fn has(&self, state_flag: i32) -> bool {
        self.state & state_flag != 0
    }
    #[inline]
    fn alive(&self) -> bool {
        self.has(S_ALIVE)
    }
    #[inline]
    fn pos(&self) -> (i32, i32) {
        (self.x, self.y)
    }
    #[inline]
    fn busy(&self) -> bool {
        self.has(S_WINDUP | S_RELOADING | S_PLANTING | S_DEFUSING)
    }
}

// ---- actions ----

/// Raw `act` call for action kinds without a wrapper.
pub fn act(kind: u32, a: i32, b: i32, c: i32) -> Result<(), Error> {
    check(sys::sys_act(kind, a, b, c))
}
/// Move in direction `angle` at `speed` units/tick (clamped). Above `walk_speed` is a noisy run.
pub fn move_dir(angle: i32, speed: i32) -> Result<(), Error> {
    act(A_MOVE, norm_angle(angle), speed, 0)
}
/// Move straight towards a point (no pathfinding; see `Navigator`). Costs one extra `me()` call.
pub fn move_towards(x: i32, y: i32, speed: i32) -> Result<(), Error> {
    let m = me();
    move_dir(angle_to(x - m.x, y - m.y), speed.min(dist((m.x, m.y), (x, y))))
}
/// Turn towards `angle` (turn-rate limited).
pub fn face(angle: i32) -> Result<(), Error> {
    act(A_FACE, norm_angle(angle), 0, 0)
}
/// Turn towards a point. Costs one extra `me()` call.
pub fn face_point(x: i32, y: i32) -> Result<(), Error> {
    let m = me();
    face(angle_to(x - m.x, y - m.y))
}
/// Start the wind-up of a shot aimed at `angle`.
pub fn fire(angle: i32) -> Result<(), Error> {
    act(A_FIRE, norm_angle(angle), 0, 0)
}
/// Start the wind-up of a shot aimed at a point. Costs one extra `me()` call.
pub fn fire_at(x: i32, y: i32) -> Result<(), Error> {
    let m = me();
    fire(angle_to(x - m.x, y - m.y))
}
pub fn reload() -> Result<(), Error> {
    act(A_RELOAD, 0, 0, 0)
}
/// Attacker carrying the charge, standing on a site.
pub fn plant() -> Result<(), Error> {
    act(A_PLANT, 0, 0, 0)
}
/// Defender within `interact_reach` of the planted charge.
pub fn defuse() -> Result<(), Error> {
    act(A_DEFUSE, 0, 0, 0)
}
/// Pick up the nearest drop within `pickup_reach`.
pub fn pickup() -> Result<(), Error> {
    act(A_PICKUP, 0, 0, 0)
}
/// Buy a shop item (buy phase only).
pub fn buy(item: i32) -> Result<(), Error> {
    act(A_BUY, item, 0, 0)
}
/// Buy a shop item for the teammate in `slot` (`MatchInfo::unit_slot`); this unit pays. Buy phase only.
pub fn buy_for(item: i32, slot: i32) -> Result<(), Error> {
    act(A_BUY_FOR, item, slot, 0)
}
/// Use the ability in `slot` (0 or 1) at a target point.
pub fn use_ability(slot: i32, x: i32, y: i32) -> Result<(), Error> {
    act(A_ABILITY, slot, x, y)
}
pub fn swap(slot: i32) -> Result<(), Error> {
    act(A_SWAP, slot, 0, 0)
}
/// Drop something (`DROP_*`).
pub fn drop_item(kind: i32) -> Result<(), Error> {
    act(A_DROP, kind, 0, 0)
}
/// Pick a kit (draft phase only). Kits other than 0 are unique within a team: `Error::NOT_ALLOWED` if taken.
pub fn draft(kit: i32) -> Result<(), Error> {
    act(A_DRAFT, kit, 0, 0)
}
/// Pick a kit for the teammate in `slot` (draft phase only). Same uniqueness rule as `draft`.
pub fn draft_for(kit: i32, slot: i32) -> Result<(), Error> {
    act(A_DRAFT_FOR, kit, slot, 0)
}
/// Cancel the current channel (plant, defuse, reload, wind-up).
pub fn cancel() -> Result<(), Error> {
    act(A_CANCEL, 0, 0, 0)
}

// ---- world queries ----

/// True if no wall lies between the two points.
pub fn los(a: (i32, i32), b: (i32, i32)) -> bool {
    sys::sys_los(a.0, a.1, b.0, b.1) == 1
}
/// Distance from (x, y) along `angle` to the first wall, capped at `max`.
pub fn raycast(x: i32, y: i32, angle: i32, max: i32) -> i32 {
    sys::sys_raycast(x, y, norm_angle(angle), max)
}

// ---- memory ----

/// Fill `out` from the team scratchpad starting at byte `off`.
pub fn team_mem_read(off: u32, out: &mut [u8]) -> Result<(), Error> {
    check(sys::sys_mem_read(MEM_TEAM, off, out))
}
/// Write to the team scratchpad (action phases only). Teammates running later this tick already see it.
pub fn team_mem_write(off: u32, data: &[u8]) -> Result<(), Error> {
    check(sys::sys_mem_write(MEM_TEAM, off, data))
}
/// Fill `out` from any memory scope (`MEM_*`).
pub fn mem_read(scope: u32, off: u32, out: &mut [u8]) -> Result<(), Error> {
    check(sys::sys_mem_read(scope, off, out))
}
pub fn mem_write(scope: u32, off: u32, data: &[u8]) -> Result<(), Error> {
    check(sys::sys_mem_write(scope, off, data))
}
/// Read `N` little-endian i32s from a memory scope (`MEM_*`; zeros if the read fails).
pub fn mem_get<const N: usize>(scope: u32, off: u32) -> [i32; N] {
    let mut v = [0i32; N];
    // SAFETY: an i32 array is valid for any bytes; length is exactly its size.
    let buf = unsafe { core::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, N * 4) };
    if sys::sys_mem_read(scope, off, buf) != OK {
        return [0; N];
    }
    v.map(i32::from_le)
}
/// Write `N` little-endian i32s to a memory scope (`MEM_*`).
pub fn mem_set<const N: usize>(scope: u32, off: u32, v: [i32; N]) -> Result<(), Error> {
    let v = v.map(i32::to_le);
    // SAFETY: as in `mem_get`.
    let buf = unsafe { core::slice::from_raw_parts(v.as_ptr() as *const u8, N * 4) };
    check(sys::sys_mem_write(scope, off, buf))
}
/// Read `N` little-endian i32s from team memory (zeros if the read fails).
pub fn team_mem_get<const N: usize>(off: u32) -> [i32; N] {
    mem_get(MEM_TEAM, off)
}
/// Write `N` little-endian i32s to team memory (action phases only).
pub fn team_mem_set<const N: usize>(off: u32, v: [i32; N]) -> Result<(), Error> {
    mem_set(MEM_TEAM, off, v)
}

// ---- debugging ----

/// `debug_draw` kinds.
pub const DEBUG_LINE: u32 = 1;
pub const DEBUG_DOT: u32 = 2;

/// Write a line to the match log (shown in the viewer; ignored in tournaments).
pub fn log(msg: &str) {
    sys::sys_log(msg.as_bytes());
}
/// `ibc::log!("hp {}", me.hp)`. Formatting costs fuel: keep it out of hot paths.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log(&::std::format!($($arg)*)) };
}
/// Draw a line in the viewer (`color` is 0xRRGGBB).
pub fn debug_line(a: (i32, i32), b: (i32, i32), color: u32) {
    sys::sys_debug_draw(DEBUG_LINE, a.0, a.1, b.0, b.1, color);
}
/// Draw a dot in the viewer (`color` is 0xRRGGBB).
pub fn debug_dot(p: (i32, i32), color: u32) {
    sys::sys_debug_draw(DEBUG_DOT, p.0, p.1, 0, 0, color);
}
