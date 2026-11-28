//! Weapons on the ground: dropped on death, `A_DROP`, `A_PICKUP`.

mod combat_common;

use combat_common::*;
use ibc_engine::abi::*;
use ibc_engine::combat;
use ibc_engine::{Match, Ruleset};

/// 2 v 2 in an open room. Units 0, 1 attack (0 carries the charge); 2, 3 defend.
fn room() -> (Match, i32, i32, i32) {
    let mut m = start(&arena(24, 11, 2), 1);
    let r = Ruleset::builtin();
    place(&mut m, 0, at(4, 3), 0);
    place(&mut m, 1, at(4, 7), 0);
    place(&mut m, 2, at(18, 3), 32768);
    place(&mut m, 3, at(18, 7), 32768);
    (
        m,
        weapon_id(&r, "pistol"),
        weapon_id(&r, "rifle"),
        weapon_id(&r, "lmg"),
    )
}

/// A weapon on the ground with `ammo` rounds in its magazine.
fn drop_of(item: i32, pos: (i32, i32), ammo: i32) -> DropInfo {
    DropInfo {
        kind: DROP_WEAPON,
        x: pos.0,
        y: pos.1,
        item,
        ammo,
    }
}

#[test]
fn death_drops_bought_weapons_but_not_the_free_sidearm() {
    let (mut m, pistol, rifle, _) = room();
    arm(&mut m, 2, rifle);
    assert_eq!(m.world.units[2].weapons, [rifle, pistol]);
    let pos = m.world.units[2].pos;
    m.world.units[2].ammo[0] = 7;
    m.world.damage(2, 1000, Some(0), pistol);
    assert_eq!(
        weapon_drops(&m),
        vec![drop_of(rifle, pos, 7)],
        "the rifle falls with the rounds left in it"
    );
    let dead = &m.world.units[2];
    assert_eq!(
        (dead.weapons, dead.ammo[0]),
        ([-1, pistol], 0),
        "the body keeps nothing that was dropped"
    );

    // A unit with only the sidearm leaves nothing.
    m.world.damage(1, 1000, Some(3), pistol);
    assert_eq!(weapon_drops(&m).len(), 1);
    m.step();
    assert_eq!(weapon_drops(&m).len(), 1, "drops stay on the ground");
}

#[test]
fn a_picked_up_weapon_goes_to_its_slot_with_the_magazine_it_fell_with() {
    let (mut m, pistol, rifle, _) = room();
    arm(&mut m, 2, rifle);
    m.world.units[2].ammo[0] = 1;
    m.world.damage(2, 1000, Some(0), pistol);
    let at_body = m.world.units[2].pos;

    // Too far: nothing happens.
    assert_eq!(m.act(3, A_PICKUP, 0, 0, 0), OK);
    m.step();
    assert_eq!(
        (weapon_drops(&m).len(), m.world.units[3].weapons),
        (1, [-1, pistol])
    );

    // In reach (defenders can pick up weapons, just not the charge).
    let reach = m.world.rules.core.pickup_reach;
    m.world.units[3].pos = (at_body.0, at_body.1 + reach - 10);
    m.act(3, A_PICKUP, 0, 0, 0);
    m.step();
    let u = &m.world.units[3];
    assert!(weapon_drops(&m).is_empty());
    assert_eq!((u.weapons, u.ammo[0]), ([rifle, pistol], 1));
    assert_eq!(u.slot, 1, "it stays in the hand it was using");

    // It fires the one round it came with, then has to be loaded.
    assert_eq!(m.act(3, A_SWAP, 0, 0, 0), OK);
    m.step();
    assert_eq!(m.act(3, A_FIRE, 32768, 0, 0), OK);
    for _ in 0..10 {
        m.step();
    }
    assert_eq!((shots(&m, 3), m.world.units[3].ammo[0]), (1, 0));
    assert_eq!(m.act(3, A_FIRE, 32768, 0, 0), ERR_NOT_READY);
    assert_eq!(combat::reload(&mut m.world, 3), OK);
    for _ in 0..weapon(&m, rifle).reload_ticks {
        m.step();
    }
    assert_eq!(m.world.units[3].ammo[0], weapon(&m, rifle).magazine);
    assert_eq!(m.act(3, A_FIRE, 32768, 0, 0), OK);
}

#[test]
fn picking_up_over_a_held_weapon_swaps_them_on_the_ground() {
    let (mut m, pistol, rifle, lmg) = room();
    arm(&mut m, 1, rifle);
    let pos = m.world.units[1].pos;
    m.world.drops.push(drop_of(lmg, (pos.0 + 200, pos.1), 5));
    let full = weapon(&m, rifle).magazine;

    // Mid wind-up with the rifle: the shot is abandoned along with the weapon.
    m.act(1, A_FIRE, 0, 0, 0);
    m.step();
    assert!(m.world.units[1].combat.winding);
    m.act(1, A_PICKUP, 0, 0, 0);
    m.step();
    let u = &m.world.units[1];
    assert_eq!((u.weapons, u.ammo[0], u.slot), ([lmg, pistol], 5, 0));
    assert_eq!((shots(&m, 1), u.combat.winding), (0, false));
    assert_eq!(
        weapon_drops(&m),
        vec![drop_of(rifle, pos, full)],
        "the old weapon lies at the unit's feet, as loaded as it was"
    );

    // And back again: the nearest drop is the rifle; each gun keeps its own magazine.
    m.act(1, A_PICKUP, 0, 0, 0);
    m.step();
    assert_eq!(
        (m.world.units[1].weapons, m.world.units[1].ammo[0]),
        ([rifle, pistol], full)
    );
    assert_eq!(weapon_drops(&m), vec![drop_of(lmg, pos, 5)]);

    // Picking up a sidearm while holding the primary does not touch the primary's reload.
    m.world.drops.clear();
    m.world.drops.push(drop_of(pistol, pos, 2));
    m.world.units[1].ammo[0] = 0;
    let held = m.world.units[1].ammo[1];
    m.act(1, A_RELOAD, 0, 0, 0);
    m.step();
    m.act(1, A_PICKUP, 0, 0, 0);
    m.step();
    assert_eq!(
        m.world.units[1].combat.reload,
        weapon(&m, rifle).reload_ticks - 2
    );
    assert_eq!(
        (m.world.units[1].ammo[1], weapon_drops(&m)),
        (2, vec![drop_of(pistol, pos, held)])
    );
}

#[test]
fn a_drop_puts_the_weapon_in_hand_at_the_units_feet() {
    let (mut m, pistol, rifle, _) = room();
    arm(&mut m, 0, rifle);
    let pos = m.world.units[0].pos;

    assert_eq!(m.act(0, A_DROP, DROP_WEAPON, 0, 0), OK);
    m.step();
    let u = &m.world.units[0];
    assert_eq!(
        (u.weapons, u.slot, u.ammo),
        ([-1, pistol], 1, [0, weapon(&m, pistol).magazine])
    );
    assert_eq!(
        weapon_drops(&m),
        vec![drop_of(rifle, pos, weapon(&m, rifle).magazine)]
    );
    assert!(
        m.world.units[0].has_charge,
        "dropping a weapon is not dropping the charge"
    );
    assert_eq!(
        m.act(0, A_FIRE, 0, 0, 0),
        OK,
        "the sidearm is ready the tick after"
    );

    // Drop the sidearm too: empty hands cannot fire, reload or drop.
    m.act(0, A_DROP, DROP_WEAPON, 0, 0);
    m.step();
    assert_eq!(
        (m.world.units[0].weapons, weapon_drops(&m).len()),
        ([-1, -1], 2)
    );
    assert_eq!(m.act(0, A_FIRE, 0, 0, 0), ERR_NOT_READY);
    assert_eq!(combat::reload(&mut m.world, 0), ERR_NOT_READY);
    m.act(0, A_DROP, DROP_WEAPON, 0, 0);
    m.step();
    assert_eq!(weapon_drops(&m).len(), 2);
    assert_eq!(
        m.world.max_speed(0),
        m.world.rules.core.run_speed,
        "nothing in hand, nothing slowing you"
    );

    // Empty-handed pickup takes the weapon straight into the hands. The rifle was dropped first, so it wins
    // the tie on distance.
    m.act(0, A_PICKUP, 0, 0, 0);
    m.step();
    assert_eq!(
        (m.world.units[0].weapons, m.world.units[0].slot),
        ([rifle, -1], 0)
    );
    assert_eq!(m.world.units[0].ammo[0], weapon(&m, rifle).magazine);
}

#[test]
fn dropping_cancels_the_shot_and_is_never_a_reload() {
    let (mut m, _, rifle, _) = room();
    arm(&mut m, 0, rifle);
    m.world.units[0].ammo[0] = 3;
    m.act(0, A_FIRE, 0, 0, 0);
    m.step();
    m.act(0, A_DROP, DROP_WEAPON, 0, 0);
    m.step();
    assert_eq!((shots(&m, 0), m.world.projectiles.len()), (0, 0));
    m.act(0, A_PICKUP, 0, 0, 0);
    m.step();
    assert_eq!(
        (m.world.units[0].weapons[0], m.world.units[0].ammo[0]),
        (rifle, 3),
        "the same three rounds come back"
    );
}

#[test]
fn a_teammate_can_be_handed_a_weapon_and_keeps_it_next_round() {
    let (mut m, pistol, _, lmg) = room();
    arm(&mut m, 0, lmg);
    m.world.units[0].ammo[0] = 9;
    m.act(0, A_DROP, DROP_WEAPON, 0, 0);
    m.step();
    m.world.units[1].pos = m.world.units[0].pos;
    m.act(1, A_PICKUP, 0, 0, 0);
    m.step();
    assert_eq!(m.world.units[1].weapons, [lmg, pistol]);
    assert_eq!(
        m.world.units[1].ammo[0], 9,
        "handed over as loaded as it was"
    );
    // End the round with unit 1 alive: survivors keep their loadout (the economy's rule), refilled.
    m.world.damage(2, 1000, Some(1), pistol);
    m.world.damage(3, 1000, Some(1), pistol);
    let round = m.world.round;
    while m.world.round == round {
        m.step();
    }
    assert!(
        m.world.drops.is_empty(),
        "the ground is cleared between rounds"
    );
    assert_eq!(m.world.units[1].weapons, [lmg, pistol]);
    assert_eq!(m.world.units[1].ammo[0], weapon(&m, lmg).magazine);
}
