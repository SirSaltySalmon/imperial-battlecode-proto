//! Weapons on the ground. A dropped weapon is a `DropInfo { kind: DROP_WEAPON, item: weapon id }` in
//! `World.drops` (which the core hashes and clears at round start). A drop carries no ammunition: a weapon
//! picked up has an empty magazine and must be reloaded, so dropping and picking up is never a faster reload.
//! Weapons are never created here, and only a dead unit's default sidearm disappears: every pickup that
//! displaces a weapon puts it on the ground.

use super::state;
use crate::fixed::V2;
use crate::world::World;
use ibc_abi::*;

fn put_down(w: &mut World, pos: V2, weapon: i32) {
    w.drops.push(DropInfo {
        kind: DROP_WEAPON,
        x: pos.0,
        y: pos.1,
        item: weapon,
    });
}

/// Take the weapon out of `slot`, if any.
fn take(w: &mut World, u: usize, slot: usize) -> Option<i32> {
    let unit = &mut w.units[u];
    let id = unit.weapons[slot];
    if id < 0 {
        return None;
    }
    unit.weapons[slot] = -1;
    unit.ammo[slot] = 0;
    Some(id)
}

/// SEAM: a unit died. Its startup, brace and reload end, and its weapons fall where it died, except the default
/// sidearm (everyone is handed one each round, so it is not worth leaving on the ground).
pub fn on_death(w: &mut World, u: usize) {
    state::cancel(w, u);
    let pos = w.units[u].pos;
    let free = w.rules.econ.default_sidearm;
    for slot in 0..2 {
        if w.units[u].weapons[slot] == free {
            continue;
        }
        if let Some(id) = take(w, u, slot) {
            put_down(w, pos, id);
        }
    }
}

/// SEAM: a non-charge drop was picked up (the core has already removed it from `World.drops`). The weapon
/// goes into its own slot with an empty magazine; a weapon already in that slot is put down in its place.
pub fn on_pickup(w: &mut World, u: usize, drop: DropInfo) {
    let wr = match w.rules.weapons.get(drop.item.max(0) as usize) {
        Some(wr) if drop.kind == DROP_WEAPON && drop.item >= 0 => *wr,
        _ => {
            w.drops.push(drop); // not something a unit can hold: leave it where it was
            return;
        }
    };
    let slot = wr.slot as usize;
    let pos = w.units[u].pos;
    let hand_was_empty = w.units[u].weapon() < 0;
    if let Some(old) = take(w, u, slot) {
        put_down(w, pos, old);
    }
    let unit = &mut w.units[u];
    unit.weapons[slot] = wr.id;
    unit.ammo[slot] = 0;
    if hand_was_empty {
        unit.slot = slot;
    }
    if w.units[u].slot == slot {
        state::changed_hands(w, u);
    }
}

/// SEAM: A_DROP with DROP_WEAPON. The weapon in hand falls at the unit's feet and the unit switches to its
/// other weapon, if it has one. Does nothing with empty hands.
pub fn on_drop(w: &mut World, u: usize) {
    let slot = w.units[u].slot;
    let Some(id) = take(w, u, slot) else {
        return;
    };
    let pos = w.units[u].pos;
    put_down(w, pos, id);
    if w.units[u].weapons[1 - slot] >= 0 {
        w.units[u].slot = 1 - slot;
    }
    state::changed_hands(w, u);
}
