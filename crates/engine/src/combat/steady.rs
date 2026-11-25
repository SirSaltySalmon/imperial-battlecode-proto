//! How steady a unit is: unsteadiness heat, which builds while the unit moves, decays while it stands, and
//! lengthens the startup of its next trigger pull. It works like recoil heat, so stopping for one tick before
//! a pull does not erase a run. Everything here is a pure function of values a bot can read (`T_WEAPON`,
//! `UnitInfo.unsteadiness`, `UnitInfo.vx`/`vy`), so a bot can predict its own startup exactly.

use super::state;
use crate::fixed;
use crate::world::World;
use ibc_abi::*;

/// Unsteadiness after one tick in which the unit actually covered `moved` units, holding `wr`:
/// `settled = max(heat - unsteady_decay, 0)`, then `min(settled + unsteady_gain * moved / 1024,
/// max(unsteady_max, settled))`.
///
/// It always decays and moving adds to it, so a steady pace settles where the two balance: a pace that adds
/// no more than the decay holds the heat at what one tick adds, and a faster one climbs to `unsteady_max`.
/// The cap only limits the gain: heat brought from a weapon with a higher cap just decays, so swapping never
/// sheds unsteadiness.
pub fn unsteadiness_after(wr: &WeaponRules, heat: i32, moved: i32) -> i32 {
    let gain = wr.unsteady_gain as i64 * moved.max(0) as i64 / 1024;
    let settled = (heat - wr.unsteady_decay).max(0) as i64;
    (settled + gain).min((wr.unsteady_max as i64).max(settled)) as i32
}

/// Ticks from a pull to the bullets leaving for a unit that is not braced:
/// `windup + round(unsteadiness * startup_per_point / 100)`, halves rounded up.
pub fn startup_ticks(wr: &WeaponRules, heat: i32) -> i32 {
    let extra = (heat.max(0) as i64 * wr.startup_per_point as i64 + 50) / 100;
    wr.windup + extra as i32
}

/// SEAM: called once per action tick after units moved; `unit.vel` is the distance each one actually covered
/// this tick (a dash included). A unit that was displaced at all also loses its startup and brace, so a
/// startup or braced tick is always a standing tick even when no move intent was queued.
pub fn after_move(w: &mut World) {
    for u in 0..w.units.len() {
        if !w.units[u].alive {
            continue;
        }
        let moved = fixed::dist((0, 0), w.units[u].vel);
        if moved > 0 {
            state::unbrace(w, u);
        }
        let weapon = w.units[u].weapon();
        // Empty hands keep the heat as it is: there are no numbers to move it by.
        if let Some(wr) = w
            .rules
            .weapons
            .get(weapon.max(0) as usize)
            .filter(|_| weapon >= 0)
        {
            let heat = w.units[u].combat.unsteady;
            w.units[u].combat.unsteady = unsteadiness_after(wr, heat, moved);
        }
    }
}
