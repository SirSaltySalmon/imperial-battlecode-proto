//! The buy phase. Weapons are chosen by what they are (longest range, biggest magazine, most pellets,
//! cheapest), not by name or id, so the logic survives a change to `rules/weapons.toml`.
use crate::kit::Kit;
use crate::{Buy, Style};
use ibc::*;

/// A primary weapon the shop sells.
#[derive(Clone, Copy)]
struct Offer {
    item: i32,
    price: i32,
    rules: WeaponRules,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arm {
    Rifle,
    Marksman,
    MachineGun,
    Shotgun,
    Smg,
}

fn arm_for(style: &Style, me: &UnitInfo, info: &MatchInfo) -> Arm {
    let slot = info.unit_slot.max(0);
    if style.buy == Buy::Force {
        return if slot % 2 == 0 { Arm::Smg } else { Arm::Shotgun };
    }
    if !style.mixed_arms {
        return Arm::Rifle;
    }
    match slot % 5 {
        2 => Arm::MachineGun,
        3 => Arm::Marksman,
        4 if me.side == SIDE_DEFEND => Arm::Shotgun,
        4 => Arm::Smg,
        _ => Arm::Rifle,
    }
}

fn find(offers: &[Offer], arm: Arm) -> Option<Offer> {
    let marksman = offers.iter().max_by_key(|o| (o.rules.range, o.price)).copied();
    match arm {
        Arm::Marksman => marksman,
        Arm::MachineGun => offers.iter().max_by_key(|o| (o.rules.magazine, o.price)).copied(),
        Arm::Shotgun => offers.iter().max_by_key(|o| (o.rules.pellets, o.price)).copied(),
        Arm::Smg => offers.iter().min_by_key(|o| o.price).copied(),
        Arm::Rifle => {
            offers.iter().filter(|o| Some(o.item) != marksman.map(|m| m.item)).max_by_key(|o| o.price).copied()
        }
    }
}

/// Buy everything this unit wants this round. Safe to call on every buy tick: it only buys what is missing.
pub(crate) fn shop(style: &Style, me: &UnitInfo, info: &MatchInfo, core: &CoreRules, kit: &Kit) {
    // The free sidearm has no price, so this is 0 unless a primary is in hand.
    let held_price = weapon(me.weapon).map_or(0, |w| w.price);
    let offers: Vec<Offer> = items()
        .filter(|it| it.kind == ITEM_KIND_WEAPON && it.price > 0)
        .filter_map(|it| weapon(it.value).map(|rules| Offer { item: it.id, price: it.price, rules }))
        .collect();
    let armour: Vec<ItemRules> = items().filter(|it| it.kind == ITEM_KIND_ARMOR).collect();
    let want = find(&offers, arm_for(style, me, info));
    let mut credits = me.credits;

    if style.buy == Buy::Eco && held_price == 0 && saves(me, info, core, want, &armour) {
        return;
    }

    // Weapon: the one this unit wants, else the most expensive it can afford that beats what it holds.
    let pick = want
        .filter(|w| w.price <= credits)
        .or_else(|| offers.iter().filter(|o| o.price <= credits).max_by_key(|o| o.price).copied())
        .filter(|w| w.price > held_price && w.rules.id != me.weapon);
    if let Some(w) = pick {
        if buy(w.item).is_ok() {
            credits -= w.price;
        }
    }

    // Armour: the best affordable, or the cheapest for a force buy.
    let better = armour.iter().filter(|it| it.value > me.armor && it.price <= credits);
    let plate =
        if style.buy == Buy::Force { better.min_by_key(|it| it.price) } else { better.max_by_key(|it| it.value) };
    if let Some(it) = plate {
        if buy(it.id).is_ok() {
            credits -= it.price;
        }
    }

    if me.side == SIDE_DEFEND && !me.has(S_HAS_KIT) {
        if let Some(it) = items().find(|it| it.kind == ITEM_KIND_DEFUSE_KIT && it.price <= credits) {
            if buy(it.id).is_ok() {
                credits -= it.price;
            }
        }
    }

    // Ability charges that have a price (free ones refill by themselves).
    for it in items().filter(|it| it.kind == ITEM_KIND_ABILITY_CHARGE) {
        let slot = it.value.clamp(0, 1) as usize;
        if let Some(a) = kit.abilities[slot].filter(|a| a.price > 0 && me.charges[slot] < a.max_charges) {
            if it.price + a.price <= credits && buy(it.id).is_ok() {
                credits -= it.price + a.price;
            }
        }
    }
}

/// An eco team keeps its money when it cannot afford its weapon with some armour, except in the first
/// round of a half (nothing to save for), the last round of a half (credits reset) or when the next
/// lost round loses the match.
fn saves(me: &UnitInfo, info: &MatchInfo, core: &CoreRules, want: Option<Offer>, armour: &[ItemRules]) -> bool {
    let half = core.rounds_per_half.max(1);
    let opener = info.round == 1 || info.round == half + 1;
    let closer = info.round == half || info.round >= 2 * half;
    let must_win = info.score_theirs >= info.rounds_to_win - 1;
    let plate = armour.iter().map(|it| it.price).min().unwrap_or(0);
    let full = want.map_or(i32::MAX, |w| w.price.saturating_add(plate));
    !opener && !closer && !must_win && me.credits < full
}
