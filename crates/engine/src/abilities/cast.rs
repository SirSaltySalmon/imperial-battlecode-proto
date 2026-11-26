//! Startup: a use waits `AbilityRules.delay` ticks before the ability happens. Meanwhile the unit may move and
//! turn, but its hands are busy: the host refuses every other primary action (`S_CASTING`), `A_CANCEL`
//! abandons the use, and so does death. Nothing is spent until the ability happens, and the use is checked
//! again then, from where the unit stands at that moment.
//!
//! A use queued on tick `T` with startup `d` happens on tick `T + d`, after that tick's intents and before
//! movement: exactly where a use without startup queued on `T + d` would have happened. Bots see `S_CASTING` on
//! the `d` turns in between.

use super::{perform, plan};
use crate::fixed::V2;
use crate::world::World;

/// Begin the startup of a use that `plan` accepted. `cast_left` counts the `pre_step`s still to come, the one
/// of this tick included.
pub(super) fn start(w: &mut World, u: usize, slot: i32, target: V2, ticks: i32) {
    let a = &mut w.units[u].abil;
    a.cast_left = ticks + 1;
    a.cast_slot = slot;
    a.cast_target = target;
}

/// SEAM: is the unit in the startup of an ability? The host refuses its primary actions except `A_CANCEL`.
pub fn casting(w: &World, u: usize) -> bool {
    w.units[u].abil.cast_left > 0
}

/// SEAM: `A_CANCEL`: abandon a use in its startup. It costs nothing.
pub fn cancel(w: &mut World, u: usize) {
    w.units[u].abil.cast_left = 0;
}

/// SEAM: once per action tick after intents, before weapons and movement: startups count down, in unit order,
/// and those that end happen now. A dead unit's use is dropped, and so is a use that is no longer possible.
pub fn pre_step(w: &mut World) {
    for u in 0..w.units.len() {
        let a = w.units[u].abil;
        if a.cast_left <= 0 {
            continue;
        }
        let alive = w.units[u].alive;
        let left = if alive { a.cast_left - 1 } else { 0 };
        w.units[u].abil.cast_left = left;
        if left > 0 || !alive {
            continue;
        }
        if let Ok((rules, p)) = plan(w, u, a.cast_slot, a.cast_target) {
            perform(w, u, a.cast_slot, &rules, p);
        }
    }
}
