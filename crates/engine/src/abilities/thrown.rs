//! Thrown devices: the second user of the bullet step. A device whose ability has `speed` > 0 leaves the
//! thrower's centre as a `Projectile` (`kind` = ability id + 1, `weapon` = -1, `range` = how far it is thrown)
//! and the core moves it with the bullets, swept against walls and `blocks_segment`. It flies over units. It
//! lands at the end of its range, or `THROW_STANDOFF` short of the first wall or barrier, and its effect
//! appears there in the same sub-step.
//!
//! Distance flown is counted in whole `speed` per tick (split over the sub-steps exactly like the velocity),
//! not from the rounded geometry, so a device thrown `L` away always lands in flight tick `ceil(L / speed)`.

use super::{geom, place, THROW_STANDOFF};
use crate::fixed::{self, V2};
use crate::world::{Projectile, World};
use ibc_abi::AbilityRules;

/// Launch a device from unit `u` towards `aim` (already cut to the ability's range). Thrown at its own feet,
/// it lands at once.
pub(super) fn throw(w: &mut World, u: usize, rules: &AbilityRules, aim: V2) {
    let (owner, team, from) = (w.units[u].id, w.units[u].team, w.units[u].pos);
    let d = fixed::dist(from, aim) as i64;
    if d == 0 {
        place(w, rules, owner, team, (from, (0, 0)), rules.radius, 0);
        return;
    }
    let speed = rules.speed.max(1) as i64;
    let vel = (((aim.0 - from.0) as i64 * speed / d) as i32, ((aim.1 - from.1) as i64 * speed / d) as i32);
    w.projectiles.push(Projectile {
        owner,
        team,
        weapon: -1,
        kind: rules.id + 1,
        pos: from,
        vel,
        travelled: 0,
        range: d as i32,
    });
}

/// SEAM: move a thrown device along a->b, sub-step `s` of `sub`. `stop` is the Q14 fraction along a->b at which
/// the core's swept test met a wall or a barrier. Returns where it lands, or advances it and returns `None`.
pub fn fly(w: &World, p: &mut Projectile, a: V2, b: V2, stop: Option<i64>, (s, sub): (i32, i32)) -> Option<V2> {
    let speed = usize::try_from(p.kind - 1).ok().and_then(|id| w.rules.abilities.get(id)).map_or(1, |r| r.speed);
    let step = (speed as i64 * (s + 1) as i64 / sub as i64 - speed as i64 * s as i64 / sub as i64).max(1);
    let left = (p.range - p.travelled).max(0) as i64;
    if let Some(t) = stop {
        if step * t / fixed::ONE <= left {
            return Some(short_of(a, b, t, p.vel));
        }
    }
    if step >= left {
        return Some(geom::lerp(a, b, left, step));
    }
    p.pos = b;
    p.travelled += step as i32;
    None
}

/// `THROW_STANDOFF` back along the flight from the point at Q14 fraction `t` of a->b. The way back is clear: the
/// device came along it, and it started at a unit's centre, further than that from any wall or barrier.
fn short_of(a: V2, b: V2, t: i64, vel: V2) -> V2 {
    let hit = geom::lerp(a, b, t, fixed::ONE);
    let speed = fixed::dist((0, 0), vel).max(1) as i64;
    let back = |v: i32| (v as i64 * THROW_STANDOFF / speed) as i32;
    (hit.0 - back(vel.0), hit.1 - back(vel.1))
}

/// SEAM: the thrown devices that landed in one sub-step, each with where it landed: their effects appear.
pub fn on_landed(w: &mut World, landed: &[(Projectile, V2)]) {
    for (p, at) in landed {
        let rules = usize::try_from(p.kind - 1).ok().and_then(|id| w.rules.abilities.get(id).copied());
        if let Some(r) = rules {
            place(w, &r, p.owner, p.team, (*at, (0, 0)), r.radius, 0);
        }
    }
}
