//! The shop: `A_BUY` during the buy phase. Purchases apply immediately. Checks run in a fixed order: unknown
//! item (`ERR_BAD_ARG`), cannot own it (`ERR_NOT_ALLOWED`), cannot pay (`ERR_INSUFFICIENT_FUNDS`). For ability
//! charges the abilities module's own checks (such as `max_charges`) come last, after the funds check: its
//! `add_charge` seam both validates and applies, so it can only be called once the price is covered.
//!
//! Trade-in: replacing a weapon or armour that was itself bought in this buy phase refunds `refund_pct` of what
//! was paid for it, so a unit can change its mind until the phase ends. Carried-over gear refunds nothing.

use super::{give, Paid, MAX_TRACKED};
use crate::world::World;
use ibc_abi::*;

/// Where unit `u`'s purchases are tracked: (team, slot in team), if it is within `MAX_TRACKED`.
fn tracked(w: &World, u: usize) -> Option<(usize, usize)> {
    let team = w.units[u].team;
    let slot = u as i32 - team * w.team_size;
    ((0..2).contains(&team) && (0..MAX_TRACKED as i32).contains(&slot))
        .then_some((team as usize, slot as usize))
}

fn paid(w: &World, u: usize) -> Paid {
    tracked(w, u)
        .map(|(t, s)| w.econ[t].paid[s])
        .unwrap_or_default()
}

fn paid_mut(w: &mut World, u: usize) -> Option<&mut Paid> {
    tracked(w, u).map(|(t, s)| &mut w.econ[t].paid[s])
}

/// Take `price` minus the trade-in for `was_paid` from the unit, or report that it cannot pay.
fn charge(w: &mut World, u: usize, price: i32, was_paid: i32) -> i32 {
    let cost = price - (was_paid as i64 * w.rules.flow.refund_pct as i64 / 100) as i32;
    if w.units[u].credits < cost {
        return ERR_INSUFFICIENT_FUNDS;
    }
    give(w, u, -cost);
    OK
}

/// SEAM: A_BUY in the buy phase. Applied immediately.
pub fn buy(w: &mut World, u: usize, item: i32) -> i32 {
    let it = match usize::try_from(item)
        .ok()
        .and_then(|i| w.rules.items.get(i))
    {
        Some(it) if it.price >= 0 => *it,
        _ => return ERR_BAD_ARG,
    };
    match it.kind {
        ITEM_KIND_WEAPON => {
            let wr = match usize::try_from(it.value)
                .ok()
                .and_then(|i| w.rules.weapons.get(i))
            {
                Some(wr) => *wr,
                None => return ERR_BAD_ARG,
            };
            let s = wr.slot as usize;
            if w.units[u].weapons[s] == wr.id {
                return ERR_NOT_ALLOWED;
            }
            let r = charge(w, u, it.price, paid(w, u).weapon[s]);
            if r != OK {
                return r;
            }
            let unit = &mut w.units[u];
            unit.weapons[s] = wr.id;
            unit.ammo[s] = wr.magazine;
            unit.slot = if unit.weapons[0] >= 0 { 0 } else { 1 };
            if let Some(p) = paid_mut(w, u) {
                p.weapon[s] = it.price;
            }
        }
        ITEM_KIND_ARMOR => {
            if w.units[u].armor >= it.value {
                return ERR_NOT_ALLOWED;
            }
            let r = charge(w, u, it.price, paid(w, u).armor);
            if r != OK {
                return r;
            }
            w.units[u].armor = it.value;
            if let Some(p) = paid_mut(w, u) {
                p.armor = it.price;
            }
        }
        ITEM_KIND_DEFUSE_KIT => {
            if w.units[u].has_kit || w.side_of(w.units[u].team) != SIDE_DEFEND {
                return ERR_NOT_ALLOWED;
            }
            let r = charge(w, u, it.price, 0);
            if r != OK {
                return r;
            }
            w.units[u].has_kit = true;
        }
        ITEM_KIND_ABILITY_CHARGE => {
            // One item per ability slot. A charge costs the item's price plus the price of whichever ability
            // the unit's kit has in that slot, so new abilities need no new shop lines.
            let ability = match ability_in_slot(w, u, it.value) {
                Some(a) => a,
                None => return ERR_NOT_ALLOWED,
            };
            let price = it.price + ability.price.max(0);
            if w.units[u].credits < price {
                return ERR_INSUFFICIENT_FUNDS;
            }
            let r = crate::abilities::add_charge(w, u, it.value);
            if r != OK {
                return r;
            }
            give(w, u, -price);
        }
        _ => return ERR_UNSUPPORTED,
    }
    OK
}

fn ability_in_slot(w: &World, u: usize, slot: i32) -> Option<AbilityRules> {
    let kit = w.rules.kits.get(usize::try_from(w.units[u].kit).ok()?)?;
    let id = *kit.abilities.get(usize::try_from(slot).ok()?)?;
    w.rules.abilities.get(usize::try_from(id).ok()?).copied()
}
