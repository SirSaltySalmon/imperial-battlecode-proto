//! Magazines, cancel, swap, and weapons on the ground. Scripts follow `combat_firing` and `combat_drops`.

use crate::record::Catalog;
use crate::setup::{self, weapon_id};
use crate::take::{self, Take};
use ibc_engine::abi::*;

pub fn add(cat: &mut Catalog) {
    reload(cat);
    cancel_windup(cat);
    reload_abandons(cat);
    killed_winding(cat);
    swap_keeps_cooldown(cat);
    death_drops(cat);
    pickup_empty(cat);
    drop_at_feet(cat);
    handoff(cat);
}

fn reload(cat: &mut Catalog) {
    let id = weapon_id("shotgun");
    let mut t = Take::begin(setup::range(id));
    let slot = t.m.world.units[0].slot;
    let ticks = t.m.world.rules.weapons[id as usize].reload_ticks;
    let cool = t.m.world.rules.weapons[id as usize].cooldown;
    t.then(move |m| m.world.units[0].ammo[slot] = 1);
    let mut guard = 0;
    while t.shots(0) < 1 && guard < 12 {
        t.act(0, A_FIRE, 0, 0);
        t.step();
        guard += 1;
    }
    for _ in 0..cool + 1 {
        t.step();
    }
    t.act(0, A_RELOAD, 0, 0);
    for _ in 0..ticks {
        t.step();
    }
    let ammo = t.m.world.units[0].ammo[slot];
    t.finish(
        cat,
        take::meta(
            "handling",
            "shotgun-reload",
            "Shotgun reload",
            "combat_firing::magazine_runs_dry_and_reload_takes_reload_ticks. One round left, then a reload. The magazine fills on the tick the timer reaches zero, and a running reload does not restart.",
            &format!("magazine {ammo}"),
        ),
    );
}

fn cancel_windup(cat: &mut Catalog) {
    let id = weapon_id("marksman");
    let mut t = Take::begin(setup::range(id));
    let mag = t.m.world.rules.weapons[id as usize].magazine;
    t.act(0, A_FIRE, 0, 0);
    t.step();
    t.act(0, A_CANCEL, 0, 0);
    t.step();
    for _ in 0..4 {
        t.step();
    }
    let shots = t.shots(0);
    t.finish(
        cat,
        take::meta(
            "handling",
            "marksman-cancel",
            "Cancel a wind-up",
            "combat_firing::cancel_and_reload_abandon_a_wind_up_without_spending_ammo. Cancelling a wind-up spends nothing: no round, no recoil, no cooldown. It can be used as a feint.",
            &format!("shots {shots}, magazine {mag}"),
        ),
    );
}

fn reload_abandons(cat: &mut Catalog) {
    let id = weapon_id("marksman");
    let mut t = Take::begin(setup::range(id));
    let ticks = t.m.world.rules.weapons[id as usize].reload_ticks;
    t.then(|m| m.world.units[0].ammo[0] = 2);
    t.act(0, A_FIRE, 0, 0);
    t.step();
    t.act(0, A_RELOAD, 0, 0);
    t.step();
    for _ in 0..ticks {
        t.step();
    }
    let (shots, ammo) = (t.shots(0), t.m.world.units[0].ammo[0]);
    t.finish(
        cat,
        take::meta(
            "handling",
            "marksman-reload-abandons",
            "Reload abandons a wind-up",
            "combat_firing::cancel_and_reload_abandon_a_wind_up_without_spending_ammo. Reloading during the wind-up keeps the round in the magazine and starts the reload.",
            &format!("shots {shots}, magazine {ammo}"),
        ),
    );
}

fn killed_winding(cat: &mut Catalog) {
    let id = weapon_id("marksman");
    let mut t = Take::begin(setup::range(id));
    t.act(0, A_FIRE, 0, 0);
    t.step();
    t.then(|m| {
        m.world.damage(0, 1000, None, -1);
    });
    for _ in 0..4 {
        t.step();
    }
    let shots = t.shots(0);
    t.finish(
        cat,
        take::meta(
            "handling",
            "killed-windup",
            "Killed during the wind-up",
            "combat_firing::a_unit_killed_during_its_wind_up_does_not_fire. Dying cancels the wind-up. Bullets already in the air would keep flying; these had not left yet.",
            &format!("shots {shots}"),
        ),
    );
}

fn swap_keeps_cooldown(cat: &mut Catalog) {
    let id = weapon_id("marksman");
    let mut t = Take::begin(setup::range(id));
    let mut guard = 0;
    while t.shots(0) < 1 && guard < 12 {
        t.act(0, A_FACE, 0, 0);
        t.act(0, A_FIRE, 0, 0);
        t.step();
        guard += 1;
    }
    t.act(0, A_SWAP, 1, 0);
    t.step();
    for _ in 0..6 {
        t.act(0, A_FIRE, 0, 0);
        t.step();
    }
    let shots = t.shots(0);
    t.finish(
        cat,
        take::meta(
            "handling",
            "swap-cooldown",
            "Swap keeps the cooldown",
            "combat_firing::swapping_abandons_reload_and_wind_up_and_keeps_cooldown. The marksman fires, then the pistol comes out. The pistol cannot shoot until the marksman's cooldown has finished, so a swap is never a faster gun.",
            &format!("shots {shots}"),
        ),
    );
}

fn death_drops(cat: &mut Catalog) {
    let rifle = weapon_id("rifle");
    let pistol = weapon_id("pistol");
    let mut m = setup::room();
    setup::arm(&mut m, 2, rifle);
    let mut t = Take::begin(m);
    t.then(move |m| {
        m.world.damage(2, 1000, Some(0), pistol);
    });
    t.step();
    t.then(move |m| {
        m.world.damage(1, 1000, Some(3), pistol);
    });
    t.step();
    let drops = weapon_drops(&t.m);
    t.finish(
        cat,
        take::meta(
            "drops",
            "death-drops",
            "Death drops bought guns, not the sidearm",
            "combat_drops::death_drops_bought_weapons_but_not_the_free_sidearm. The defender holding a rifle drops it and keeps nothing. The attacker holding only the free pistol drops nothing.",
            &format!("{drops} weapon drop(s)"),
        ),
    );
}

fn pickup_empty(cat: &mut Catalog) {
    let rifle = weapon_id("rifle");
    let pistol = weapon_id("pistol");
    let mut m = setup::room();
    setup::arm(&mut m, 2, rifle);
    let mut t = Take::begin(m);
    t.then(move |m| {
        m.world.damage(2, 1000, Some(0), pistol);
    });
    t.step();
    t.act(3, A_PICKUP, 0, 0);
    t.step();
    let reach = t.m.world.rules.core.pickup_reach;
    t.then(move |m| {
        let body = m.world.units[2].pos;
        m.world.units[3].pos = (body.0, body.1 + reach - 10);
    });
    t.act(3, A_PICKUP, 0, 0);
    t.step();
    let ammo = t.m.world.units[3].ammo[0];
    t.finish(
        cat,
        take::meta(
            "drops",
            "pickup-empty",
            "A picked-up rifle has an empty magazine",
            "combat_drops::a_picked_up_weapon_goes_to_its_slot_with_an_empty_magazine. The first pickup is out of reach and does nothing. Stepping into pickup_reach takes the rifle into its own slot, empty, and the unit keeps the pistol in hand.",
            &format!("rifle ammo {ammo}"),
        ),
    );
}

fn drop_at_feet(cat: &mut Catalog) {
    let rifle = weapon_id("rifle");
    let mut m = setup::room();
    setup::arm(&mut m, 0, rifle);
    let mut t = Take::begin(m);
    t.act(0, A_DROP, DROP_WEAPON, 0);
    t.step();
    t.act(0, A_DROP, DROP_WEAPON, 0);
    t.step();
    t.act(0, A_PICKUP, 0, 0);
    t.step();
    let held = t.m.world.units[0].weapon();
    let ammo = t.m.world.units[0].ammo[t.m.world.units[0].slot];
    t.finish(
        cat,
        take::meta(
            "drops",
            "drop-feet",
            "Drop both guns, pick the rifle back up",
            "combat_drops::a_drop_puts_the_weapon_in_hand_at_the_units_feet. Dropping puts the gun in hand at your feet and switches to the other one. With empty hands the next pickup takes the nearest gun (the rifle, dropped first) straight into that slot. A gun on the ground holds no ammunition.",
            &format!("holding weapon {held}, ammo {ammo}"),
        ),
    );
}

fn handoff(cat: &mut Catalog) {
    let lmg = weapon_id("lmg");
    let mut m = setup::room();
    setup::arm(&mut m, 0, lmg);
    let mut t = Take::begin(m);
    t.act(0, A_DROP, DROP_WEAPON, 0);
    t.step();
    t.then(|m| m.world.units[1].pos = m.world.units[0].pos);
    t.act(1, A_PICKUP, 0, 0);
    t.step();
    let ammo = t.m.world.units[1].ammo[0];
    t.finish(
        cat,
        take::meta(
            "drops",
            "handoff",
            "Hand a teammate the LMG",
            "combat_drops::a_teammate_can_be_handed_a_weapon_and_keeps_it_next_round. The teammate is placed on the drop. The LMG arrives with an empty magazine, so handing a gun costs them a reload. Dropping and picking up is never a faster reload.",
            &format!("lmg ammo {ammo}"),
        ),
    );
}

fn weapon_drops(m: &ibc_engine::Match) -> usize {
    m.world
        .drops
        .iter()
        .filter(|d| d.kind == DROP_WEAPON)
        .count()
}
