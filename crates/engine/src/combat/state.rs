//! Weapon state of one unit and the rules for starting and stopping things: startup, brace, cooldown, reload,
//! recoil.
//!
//! Timeline of a burst with startup `s` (`steady::startup_ticks`) and cooldown `c`, `A_FIRE` queued on tick T
//! by a unit that is not braced:
//!   T .. T+s-1   starting up (enemies who see the unit get `S_WINDUP` and the aim angle)
//!   T+s          bullets leave (before anyone moves on that tick); the unit is now braced
//!   T+s+1 ..     `c` ticks on which no pull is accepted
//!   T+s+1+c      the next `A_FIRE` is accepted and, braced, its bullets leave on that same tick.
//! So a braced unit fires every `1 + c` ticks. The brace breaks on a move intent, on a tick the weapon is
//! ready and no `A_FIRE` was queued, and on reload, swap, cancel, plant and defuse; turning does not break it.
//! A move intent also cancels a startup in progress, and a pull queued on a tick the unit moves is refused:
//! startup and braced ticks are always standing ticks.

use super::{firing, steady};
use crate::world::World;
use crate::Fnv;
use ibc_abi::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct UnitCombat {
    /// Starting up (or, braced, releasing this tick).
    pub winding: bool,
    /// Ticks of startup left.
    pub windup: i32,
    pub aim: i32,
    pub cooldown: i32,
    pub reload: i32,
    pub recoil: i32,
    pub shots: i32,
    /// Startup done and the trigger still held: the next pull leaves on the tick it is made.
    pub braced: bool,
    /// Unsteadiness heat (`steady`).
    pub unsteady: i32,
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
            self.braced as i32,
            self.unsteady,
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

/// Has the unit queued a move this tick? A move cancels a startup and breaks the brace.
fn moving(w: &World, u: usize) -> bool {
    matches!(w.units[u].intent.mv, Some((_, speed)) if speed > 0)
}

/// Would a pull be accepted, ignoring this tick's move intent?
fn ready(w: &World, u: usize) -> bool {
    let unit = &w.units[u];
    let c = &unit.combat;
    unit.weapon() >= 0 && !c.winding && c.reload == 0 && c.cooldown == 0 && unit.ammo[unit.slot] > 0
}

/// SEAM: could `start_fire` succeed right now? Used by `act` to give bots an early error code (a move queued
/// after the pull in the same tick still refuses it when the tick resolves).
pub fn can_fire(w: &World, u: usize) -> i32 {
    if !ready(w, u) {
        return ERR_NOT_READY;
    }
    if moving(w, u) {
        return ERR_NOT_ALLOWED;
    }
    OK
}

/// SEAM: pull the trigger towards `angle`. Braced, the bullets leave this tick; otherwise a startup of
/// `steady::startup_ticks` begins.
pub fn start_fire(w: &mut World, u: usize, angle: i32) -> i32 {
    let r = can_fire(w, u);
    if r != OK {
        return r;
    }
    let wr = w.rules.weapons[w.units[u].weapon() as usize];
    let c = &mut w.units[u].combat;
    c.windup = if c.braced {
        0
    } else {
        steady::startup_ticks(&wr, c.unsteady)
    };
    c.winding = true;
    c.aim = angle & 0xFFFF;
    OK
}

/// End a startup in progress and the brace. A reload keeps running.
pub(super) fn unbrace(w: &mut World, u: usize) {
    let c = &mut w.units[u].combat;
    c.winding = false;
    c.windup = 0;
    c.braced = false;
}

/// SEAM: an ability use was accepted (it starts its startup or happens now): the hands leave the trigger, so
/// a weapon startup is abandoned and the brace breaks, as for a reload. A reload keeps running.
pub fn on_cast(w: &mut World, u: usize) {
    unbrace(w, u);
}

/// SEAM: start reloading the weapon in hand. Refused when the magazine is full or a reload is already running
/// (it does not restart). Reloading breaks the brace; during a startup it abandons the shot and the round stays
/// in the magazine.
pub fn reload(w: &mut World, u: usize) -> i32 {
    let unit = &w.units[u];
    if unit.weapon() < 0 {
        return ERR_NOT_READY;
    }
    let wr = w.rules.weapons[unit.weapon() as usize];
    if unit.ammo[unit.slot] >= wr.magazine || unit.combat.reload > 0 {
        return ERR_NOT_READY;
    }
    unbrace(w, u);
    w.units[u].combat.reload = wr.reload_ticks;
    OK
}

/// SEAM: switch weapon slot. Abandons a startup or reload of the weapon put away and breaks the brace. Swapping to the slot already
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

/// The weapon in hand changed (swap, drop, pickup): the startup or reload of the old one is abandoned and the
/// brace breaks.
/// Recoil and a cooldown still running carry over, so changing weapon never lets a unit shoot sooner.
/// (Changing weapon is the tick's primary action, so the new weapon cannot fire before the next tick.)
pub(super) fn changed_hands(w: &mut World, u: usize) {
    cancel(w, u);
}

/// SEAM: abort startup and reload, and break the brace. Costs nothing: no ammunition is used and the magazine
/// keeps what it had.
pub fn cancel(w: &mut World, u: usize) {
    unbrace(w, u);
    w.units[u].combat.reload = 0;
}

/// SEAM: called once per action tick after intents, before movement.
pub fn pre_step(w: &mut World) {
    for u in 0..w.units.len() {
        if !w.units[u].alive {
            continue;
        }
        // Before the timers run, so "ready" means what it meant when the unit acted. A ready, braced unit
        // that pulled is winding by now; one that did not has let go of the trigger.
        let braced_idle = w.units[u].combat.braced && ready(w, u);
        if moving(w, u) || braced_idle {
            unbrace(w, u);
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
