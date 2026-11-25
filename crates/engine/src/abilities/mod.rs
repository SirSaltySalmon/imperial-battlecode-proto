//! Abilities and kits seam. Foundation: kits exist (stats only), no abilities. The core calls the SEAM functions.

use crate::fixed::V2;
use crate::world::World;
use crate::Fnv;
use ibc_abi::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct UnitAbilities {
    pub charges: [i32; 2],
    pub cooldown: [i32; 2],
}

impl UnitAbilities {
    pub fn hash(&self, h: &mut Fnv) {
        for v in [self.charges[0], self.charges[1], self.cooldown[0], self.cooldown[1]] {
            h.i32(v);
        }
    }
}

/// SEAM: A_DRAFT in the draft phase.
pub fn draft(w: &mut World, u: usize, kit: i32) -> i32 {
    if kit < 0 || kit as usize >= w.rules.kits.len() {
        return ERR_BAD_ARG;
    }
    w.units[u].kit = kit;
    OK
}

/// SEAM: early validation for `act`.
pub fn can_use(_w: &World, _u: usize, _slot: i32, _target: V2) -> i32 {
    ERR_UNSUPPORTED
}

/// SEAM: A_ABILITY resolved at the start of the tick.
pub fn use_ability(_w: &mut World, _u: usize, _slot: i32, _target: V2) -> i32 {
    ERR_UNSUPPORTED
}

/// SEAM: buying an ability charge (ITEM_KIND_ABILITY_CHARGE). The economy module has already checked credits.
pub fn add_charge(_w: &mut World, _u: usize, _slot: i32) -> i32 {
    ERR_UNSUPPORTED
}

/// SEAM: once per action tick after movement: tick effects, apply area damage, expire.
pub fn step(w: &mut World) {
    for e in w.effects.iter_mut() {
        e.info.ticks_left -= 1;
    }
    w.effects.retain(|e| e.info.ticks_left > 0);
}

/// SEAM: does a dynamic effect block sight between two points?
pub fn blocks_vision(_w: &World, _a: V2, _b: V2) -> bool {
    false
}

/// SEAM: does a dynamic effect block a circle at `p`?
pub fn blocks_move(_w: &World, _p: V2, _r: i32) -> bool {
    false
}

/// SEAM: does a dynamic effect stop a projectile on a->b? Returns the Q14 fraction along the segment.
pub fn blocks_segment(_w: &World, _a: V2, _b: V2) -> Option<i64> {
    None
}

/// SEAM
pub fn on_round_start(w: &mut World) {
    for u in w.units.iter_mut() {
        u.abil.cooldown = [0, 0];
    }
}
