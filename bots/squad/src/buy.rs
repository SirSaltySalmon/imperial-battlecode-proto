//! The buy phase. Weapons are chosen by what they are (one bullet or a fan, longest range, biggest magazine,
//! most pellets, cheapest), not by name or id, so the logic survives a change to `rules/weapons.toml`.
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

/// A primary weapon's role, resolved from what the weapons are (see `find`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
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

/// The weapon for `arm`. Single-bullet guns (the marksman rifle and the cheap long gun) and fans (everything
/// else) are told apart first, so a cheap long gun never passes for the SMG and a dear fan for the marksman.
fn find(offers: &[Offer], arm: Arm) -> Option<Offer> {
    let fans = || offers.iter().filter(|o| o.rules.pellets > 1);
    match arm {
        Arm::Marksman => marksman(offers),
        Arm::MachineGun => fans().max_by_key(|o| (o.rules.magazine, o.price)).copied(),
        Arm::Shotgun => fans().max_by_key(|o| (o.rules.pellets, o.price)).copied(),
        Arm::Smg => fans().min_by_key(|o| o.price).copied(),
        Arm::Rifle => fans().max_by_key(|o| o.price).copied(),
    }
}

/// The longest-reaching single-bullet gun.
fn marksman(offers: &[Offer]) -> Option<Offer> {
    offers.iter().filter(|o| o.rules.pellets == 1).max_by_key(|o| (o.rules.range, o.price)).copied()
}

/// The cheap long gun: the cheapest single-bullet gun other than the marksman rifle.
fn long_eco(offers: &[Offer]) -> Option<Offer> {
    let top = marksman(offers).map(|m| m.item);
    offers.iter().filter(|o| o.rules.pellets == 1 && Some(o.item) != top).min_by_key(|o| o.price).copied()
}

/// The weapon `Style::loadout` forces on this unit's slot, if any.
fn forced(style: &Style, info: &MatchInfo, offers: &[Offer]) -> Option<Offer> {
    let slot = info.unit_slot.max(0);
    style.loadout.filter(|l| l.slot == slot).and_then(|l| find(offers, l.arm))
}

/// Buy everything this unit wants this round. Safe to call on every buy tick: it only buys what is missing.
pub(crate) fn shop(style: &Style, me: &UnitInfo, info: &MatchInfo, core: &CoreRules, kit: &Kit) {
    // The free sidearm has no price, so this is 0 unless a primary is in hand.
    let held_price = weapon(me.weapon).map_or(0, |w| w.price);
    let offers: Vec<Offer> = items()
        .filter(|it| it.kind == ITEM_KIND_WEAPON && it.price > 0)
        .filter_map(|it| weapon(it.value).map(|rules| Offer { item: it.id, price: it.price, rules }))
        .filter(|o| o.rules.slot == 0)
        .collect();
    let armour: Vec<ItemRules> = items().filter(|it| it.kind == ITEM_KIND_ARMOR).collect();
    let arm = arm_for(style, me, info);
    let long = arm == Arm::Marksman;
    let want = find(&offers, arm);
    let mut credits = me.credits;
    let beats = |o: &Offer, credits: i32| o.price <= credits && o.price > held_price && o.rules.id != me.weapon;

    // A saving team keeps its money, except that the long slot takes the cheap long gun for its angles. Whether
    // to save is the usual weapon's question, so a loadout override never moves an eco round.
    let cheap_long = long_eco(&offers);
    let saving_kit = held_price == 0 || cheap_long.is_some_and(|w| w.rules.id == me.weapon);
    if style.buy == Buy::Eco && saving_kit && saves(me, info, core, want, &armour) {
        if let Some(w) = cheap_long.filter(|w| long && beats(w, credits)) {
            let _ = buy(w.item);
        }
        return;
    }

    // Weapon: a forced one it can afford, even over a dearer one it holds (else the A/B would measure the
    // weapon a survivor kept). Otherwise the one this unit wants (nothing, if it already holds it). Short of
    // it: for the long slot the cheap long gun, for the same angles; else the most expensive it can afford that
    // beats what it holds (a fan, unless this is the long slot).
    let fallback = || {
        let fits = |o: &&Offer| beats(o, credits) && (long || o.rules.pellets > 1);
        offers.iter().filter(fits).max_by_key(|o| o.price).copied()
    };
    let pick = match forced(style, info, &offers).filter(|w| w.price <= credits) {
        Some(w) => Some(w).filter(|w| w.rules.id != me.weapon),
        None => match want.filter(|w| w.price <= credits) {
            Some(w) => Some(w).filter(|w| beats(w, credits)),
            None => cheap_long.filter(|w| long && beats(w, credits)).or_else(fallback),
        },
    };
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
