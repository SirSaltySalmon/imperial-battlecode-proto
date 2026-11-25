//! Weapon state of one unit and the rules for starting and stopping things: wind-up, cooldown, reload, recoil.
//!
//! Timeline of one trigger pull with wind-up `w` and cooldown `c`, `A_FIRE` queued on tick T:
//!   T .. T+w-1   winding (enemies who see the unit get `S_WINDUP` and the aim angle)
//!   T+w          bullets leave (before anyone moves on that tick)
//!   T+w+1 ..     `c` ticks on which a new wind-up cannot start
//!   T+w+1+c      the next `A_FIRE` is accepted, so one pull takes `w + 1 + c` ticks.

use super::firing;
use crate::world::World;
use crate::Fnv;
use ibc_abi::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct UnitCombat {
    pub winding: bool,
    pub windup: i32,
    pub aim: i32,
    pub cooldown: i32,
    pub reload: i32,
    pub recoil: i32,
    pub shots: i32,
}

impl UnitCombat {
    pub fn hash(&self, h: &mut Fnv) {
        for v in [
            self.winding as i32,
            self.windup,
            self.aim,
            self.cooldown,
            self.reload,
            self.recoil,
            self.shots,
        ] {
            h.i32(v);
        }
    }
    /// Ticks left in the current weapon channel, for UnitInfo.channel.
    pub fn channel(&self) -> i32 {
        if self.winding {
            self.windup + 1
        } else {
            self.reload
        }
    }
}

/// SEAM: could `start_fire` succeed right now? Used by `act` to give bots an early error code.
pub fn can_fire(w: &World, u: usize) -> i32 {
    let unit = &w.units[u];
    let c = &unit.combat;
    if unit.weapon() < 0 || c.winding || c.reload > 0 || c.cooldown > 0 || unit.ammo[unit.slot] <= 0
    {
        return ERR_NOT_READY;
    }
    OK
}

/// SEAM: begin a wind-up towards `angle`.
pub fn start_fire(w: &mut World, u: usize, angle: i32) -> i32 {
    let r = can_fire(w, u);
    if r != OK {
        return r;
    }
    let wid = w.units[u].weapon() as usize;
    let windup = w.rules.weapons[wid].windup;
    let c = &mut w.units[u].combat;
    c.winding = true;
    c.windup = windup;
    c.aim = angle & 0xFFFF;
    OK
}

/// SEAM: start reloading the weapon in hand. Refused when the magazine is full or a reload is already running
/// (it does not restart). Reloading during a wind-up abandons the shot; the round stays in the magazine.
pub fn reload(w: &mut World, u: usize) -> i32 {
    let unit = &w.units[u];
    if unit.weapon() < 0 {
        return ERR_NOT_READY;
    }
    let wr = w.rules.weapons[unit.weapon() as usize];
    if unit.ammo[unit.slot] >= wr.magazine || unit.combat.reload > 0 {
        return ERR_NOT_READY;
    }
    let c = &mut w.units[u].combat;
    c.winding = false;
    c.windup = 0;
    c.reload = wr.reload_ticks;
    OK
}

/// SEAM: switch weapon slot. Abandons a wind-up or reload of the weapon put away. Swapping to the slot already
/// in hand does nothing.
pub fn swap(w: &mut World, u: usize, slot: i32) -> i32 {
    if !(0..2).contains(&slot) || w.units[u].weapons[slot as usize] < 0 {
        return ERR_BAD_ARG;
    }
    if w.units[u].slot == slot as usize {
        return OK;
    }
    w.units[u].slot = slot as usize;
    changed_hands(w, u);
    OK
}

/// The weapon in hand changed (swap, drop, pickup): the wind-up or reload of the old one is abandoned.
/// Recoil and a cooldown still running carry over, so changing weapon never lets a unit shoot sooner.
/// (Changing weapon is the tick's primary action, so the new weapon cannot fire before the next tick.)
pub(super) fn changed_hands(w: &mut World, u: usize) {
    cancel(w, u);
}

/// SEAM: abort wind-up and reload. Costs nothing: no ammunition is used and the magazine keeps what it had.
pub fn cancel(w: &mut World, u: usize) {
    let c = &mut w.units[u].combat;
    c.winding = false;
    c.windup = 0;
    c.reload = 0;
}

/// SEAM: called once per action tick after intents, before movement.
pub fn pre_step(w: &mut World) {
    for u in 0..w.units.len() {
        if !w.units[u].alive {
            continue;
        }
        tick_timers(w, u);
        let mut fired = false;
        if w.units[u].combat.winding {
            if w.units[u].combat.windup <= 0 {
                firing::release(w, u);
                fired = true;
            } else {
                w.units[u].combat.windup -= 1;
            }
        }
        if !fired && w.units[u].weapon() >= 0 {
            let rec = w.rules.weapons[w.units[u].weapon() as usize].recoil_recovery;
            let c = &mut w.units[u].combat;
            c.recoil = (c.recoil - rec).max(0);
        }
    }
}

/// Reload and cooldown count down; a reload that reaches zero fills the magazine of the weapon in hand.
/// (Anything that changes the weapon in hand cancels the reload, so it is always the weapon that started it.)
fn tick_timers(w: &mut World, u: usize) {
    let unit = &mut w.units[u];
    if unit.combat.reload > 0 {
        unit.combat.reload -= 1;
        if unit.combat.reload == 0 && unit.weapon() >= 0 {
            unit.ammo[unit.slot] = w.rules.weapons[unit.weapon() as usize].magazine;
        }
    }
    if unit.combat.cooldown > 0 {
        unit.combat.cooldown -= 1;
    }
}

/// SEAM: reset per-round weapon state (ammo and loadout are the economy module's job).
pub fn on_round_start(w: &mut World) {
    for u in w.units.iter_mut() {
        u.combat = UnitCombat::default();
    }
}
