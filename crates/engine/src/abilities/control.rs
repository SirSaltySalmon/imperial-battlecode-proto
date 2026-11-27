//! Area denial and information: the trap, the sonar pulse and the concussion. Called from `effects::step` for
//! active effects only. Like every ability these do not pick sides: they act on teammates and on the user too.
//! Every unit one of them acts on learns where it was (`affected`).

use super::{affected, geom, open_between};
use crate::combat;
use crate::fixed;
use crate::world::{Effect, World};
use ibc_abi::*;

fn status_turns(r: &AbilityRules) -> i32 {
    // The core counts statuses down right after this step, so n turns are stored as n + 1.
    r.power.max(0) + 1
}

/// Trap: springs on every living unit whose centre came within `radius` of it this tick, along the straight
/// path from where the unit was at the end of the last tick to where it is now (so a runner or a dash cannot
/// step over it). Each one is slowed and revealed for `power` turns, and the trap is spent.
pub(super) fn spring(w: &mut World, i: usize, r: &AbilityRules) {
    let trap = w.effects[i];
    let at = (trap.info.x, trap.info.y);
    let r2 = trap.info.radius as i64 * trap.info.radius as i64;
    let caught: Vec<usize> = (0..w.units.len())
        .filter(|&j| {
            let u = &w.units[j];
            u.alive && geom::point_segment_dist2(at, u.abil.last_pos, u.pos) <= r2
        })
        .collect();
    if caught.is_empty() {
        return;
    }
    let turns = status_turns(r);
    for j in caught {
        let u = &mut w.units[j];
        u.slowed = u.slowed.max(turns);
        u.revealed = u.revealed.max(turns);
        affected(w, &trap, j);
    }
    w.effects[i].info.ticks_left = 0;
}

/// Sonar: one pulse that marks every living unit within `radius`, friend or foe and through walls, as
/// revealed for `power` turns. A revealed unit is visible to the whole other team, so the pulse gives away
/// the user's own teammates standing in it.
pub(super) fn sonar(w: &mut World, e: &Effect, r: &AbilityRules) {
    let at = (e.info.x, e.info.y);
    let r2 = e.info.radius as i64 * e.info.radius as i64;
    let turns = status_turns(r);
    for j in 0..w.units.len() {
        let u = &mut w.units[j];
        if u.alive && fixed::dist2(u.pos, at) <= r2 {
            u.revealed = u.revealed.max(turns);
            affected(w, e, j);
        }
    }
}

/// Concussion: one pop. Every living unit within `radius` that no wall or barrier shelters from it is slowed
/// for `power` turns and loses what it was doing: a plant or defuse channel, a weapon startup and its brace, a
/// reload, an ability use in its startup. Progress on any of them is lost.
pub(super) fn concuss(w: &mut World, e: &Effect, r: &AbilityRules) {
    let at = (e.info.x, e.info.y);
    let r2 = e.info.radius as i64 * e.info.radius as i64;
    let hit: Vec<usize> = (0..w.units.len())
        .filter(|&j| {
            let u = &w.units[j];
            u.alive && fixed::dist2(u.pos, at) <= r2 && open_between(w, at, u.pos)
        })
        .collect();
    let turns = status_turns(r);
    for j in hit {
        w.cancel_channel(j);
        combat::cancel(w, j);
        super::cast::cancel(w, j);
        let u = &mut w.units[j];
        u.slowed = u.slowed.max(turns);
        affected(w, e, j);
    }
}
