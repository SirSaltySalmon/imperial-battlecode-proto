//! What a bullet does between the muzzle and the hit. The flight itself (swept against walls and unit circles,
//! per movement slice) is in the core's `step_projectiles`.

use crate::abilities;
use crate::fixed::{self, V2};
use crate::world::{Projectile, World};
use ibc_abi::*;

/// Where a bullet fired by a unit at `origin` along `angle` starts: on the unit's circle. `None` if a wall or
/// a bullet-blocking effect lies between the unit's centre and that point: the bullet is spent at once, so a
/// unit pressed against cover can never fire from the far side of it.
pub(super) fn muzzle(w: &World, origin: V2, angle: i32) -> Option<V2> {
    let off = fixed::polar(angle, w.rules.core.unit_radius);
    let pos = (origin.0 + off.0, origin.1 + off.1);
    if w.map.segment_wall(origin, pos).is_some()
        || abilities::blocks_segment(w, origin, pos).is_some()
    {
        return None;
    }
    Some(pos)
}

/// Damage of one bullet of `wr` that hits after flying `travelled` units: full up to `falloff_start`, then
/// linear down to `falloff_min_pct` percent at `range`.
pub fn damage_at(wr: &WeaponRules, travelled: i32) -> i32 {
    if travelled <= wr.falloff_start || wr.range <= wr.falloff_start {
        return wr.damage;
    }
    let span = (wr.range - wr.falloff_start) as i64;
    let over = ((travelled - wr.falloff_start) as i64).min(span);
    let pct = 100 - (100 - wr.falloff_min_pct as i64) * over / span;
    (wr.damage as i64 * pct / 100) as i32
}

/// SEAM: damage of a bullet that hit after travelling `p.travelled`. The core removes a bullet at the end of
/// the movement slice in which it passes `range`, so a hit can come slightly late: past `range` it does nothing.
/// That makes a weapon's reach exactly `range` in every direction, whatever the slice length.
pub fn bullet_damage(w: &World, p: &Projectile) -> i32 {
    match w.rules.weapons.get(p.weapon.max(0) as usize) {
        Some(wr) if p.weapon >= 0 && p.travelled <= wr.range => damage_at(wr, p.travelled),
        _ => 0,
    }
}
