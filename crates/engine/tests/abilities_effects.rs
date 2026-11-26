//! One scenario per ability, driven through the same `Host` calls bots use. Units are placed by hand and
//! bought charges are set directly (shop items belong to the economy unit).

use ibc_engine::abi::*;
use ibc_engine::{abilities, Map, Match, Ruleset};

/// 20x9, two per team, one wall tile at (9,5).
const ARENA: &str = "name: arena\nteam_size: 2\n---\n\
####################\n\
#a................d#\n\
#a................d#\n\
#..................#\n\
#..................#\n\
#........#.........#\n\
#..................#\n\
#................11#\n\
####################\n";

const EAST: i32 = 0;
const SOUTH: i32 = 16384;
const WEST: i32 = 32768;
const NORTH: i32 = 49152;

fn c(tx: i32, ty: i32) -> (i32, i32) {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

fn kit(name: &str) -> i32 {
    Ruleset::builtin().kit_names.iter().position(|n| n == name).expect("kit exists") as i32
}

fn ability(name: &str) -> AbilityRules {
    let r = Ruleset::builtin();
    r.abilities[r.ability_names.iter().position(|n| n == name).expect("ability exists")]
}

/// A match in its first action tick: unit 0 (team A) drafted `caster_kit`, everyone else plays kit 0.
/// Units 1 and 3 are parked in corners looking at the wall.
fn setup(caster_kit: &str) -> Match {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(ARENA).unwrap(), 1);
    assert_eq!(m.act(0, A_DRAFT, kit(caster_kit), 0, 0), OK);
    while m.world.phase != PHASE_ACTION {
        m.step();
    }
    place(&mut m, 1, c(1, 7), SOUTH);
    place(&mut m, 3, c(18, 1), NORTH);
    m
}

fn place(m: &mut Match, unit: usize, pos: (i32, i32), facing: i32) {
    m.world.units[unit].pos = pos;
    m.world.units[unit].facing = facing;
}

fn steps(m: &mut Match, n: usize) {
    for _ in 0..n {
        m.step();
    }
}

fn row<T: Copy + Default>(m: &Match, unit: u32, table: u32, index: u32) -> T {
    let mut buf = Vec::new();
    assert!(m.get(unit, table, index, &mut buf) >= 0);
    from_bytes(&buf).unwrap()
}

fn sees(m: &Match, unit: u32, enemy: i32) -> bool {
    (0..m.count(unit, T_ENEMIES)).any(|i| row::<UnitInfo>(m, unit, T_ENEMIES, i as u32).id == enemy)
}

fn sounds(m: &Match, unit: u32) -> Vec<SoundInfo> {
    (0..m.count(unit, T_SOUNDS)).map(|i| row(m, unit, T_SOUNDS, i as u32)).collect()
}

fn use_at(m: &mut Match, unit: u32, slot: i32, at: (i32, i32)) -> i32 {
    m.act(unit, A_ABILITY, slot, at.0, at.1)
}

#[test]
fn smoke_blocks_sight_both_ways_but_not_bullets() {
    let mut m = setup("shroud");
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, c(12, 3), WEST);
    m.world.units[0].abil.charges[1] = 1;
    m.step();
    assert!(sees(&m, 0, 2) && sees(&m, 2, 0));

    assert_eq!(use_at(&mut m, 0, 1, c(7, 3)), OK);
    m.step();
    assert!(!sees(&m, 0, 2) && !sees(&m, 2, 0), "the cloud is between them");
    assert_eq!(m.count(2, T_EFFECTS), 1, "the enemy can see the cloud itself");
    let e: EffectInfo = row(&m, 2, T_EFFECTS, 0);
    assert_eq!((e.kind, e.x, e.y, e.radius), (EFFECT_SMOKE, c(7, 3).0, c(7, 3).1, ability("smoke").radius));
    assert_eq!(row::<UnitInfo>(&m, 0, T_SELF, 0).charges, [1, 0], "the free decoy is left, the smoke is spent");

    // Shooting through it works: bullets ignore smoke.
    for _ in 0..4 {
        m.act(0, A_FIRE, EAST, 0, 0);
        m.step();
        assert!(!sees(&m, 0, 2));
    }
    assert!(m.world.units[2].hp < 100, "bullets pass through smoke");

    steps(&mut m, ability("smoke").duration as usize);
    assert!(m.world.effects.is_empty());
    assert!(sees(&m, 0, 2) && sees(&m, 2, 0));
}

#[test]
fn smoke_hides_a_unit_standing_on_its_centre() {
    let mut m = setup("shroud");
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, c(12, 3), WEST);
    m.world.units[0].abil.charges[1] = 1;
    assert_eq!(use_at(&mut m, 0, 1, c(2, 3)), OK);
    m.step();
    assert_eq!((m.world.effects[0].info.x, m.world.effects[0].info.y), c(2, 3));
    assert!(!sees(&m, 2, 0) && !sees(&m, 0, 2));
}

#[test]
fn barrier_cut_short_by_a_wall_is_still_visible_to_enemies() {
    let mut m = setup("bulwark");
    place(&mut m, 0, c(3, 6), EAST);
    place(&mut m, 2, c(9, 6), WEST);
    m.world.units[0].abil.charges[1] = 1;
    assert_eq!(use_at(&mut m, 0, 1, c(5, 6)), OK);
    m.step();
    let e = m.world.effects[0].info;
    assert_eq!((e.x, e.y, e.y2), (c(5, 6).0, 8 * TILE - 1, 5 * TILE), "one end stops at the bottom wall");
    assert_eq!(m.count(2, T_EFFECTS), 1);
    assert!(!sees(&m, 2, 0));
}

#[test]
fn flash_blinds_by_how_directly_it_is_faced_and_spares_units_facing_away() {
    let mut m = setup("lancer");
    let flash = ability("flash");
    place(&mut m, 0, c(2, 3), EAST); // the thrower, looking straight at its own flash
    place(&mut m, 1, c(2, 5), EAST); // a teammate with the flash off-centre in its cone
    place(&mut m, 2, c(12, 3), WEST); // an enemy facing it head-on
    place(&mut m, 3, c(12, 5), EAST); // an enemy facing away
    m.world.units[0].abil.charges[1] = 1;
    m.step();
    assert!(sees(&m, 2, 0));

    assert_eq!(use_at(&mut m, 0, 1, c(8, 3)), OK);
    m.step();
    assert!(m.world.units.iter().all(|u| u.blind == 0), "nothing happens before the delay is over");
    assert_eq!(m.count(2, T_EFFECTS), 1, "the enemy sees it coming");
    m.step();
    let blind: Vec<i32> = m.world.units.iter().map(|u| u.blind).collect();
    assert_eq!(blind[2], flash.duration, "head-on: the full duration");
    assert_eq!(blind[0], flash.duration, "the thrower is not exempt");
    assert!(blind[1] > 0 && blind[1] < blind[2], "off-centre: shorter ({blind:?})");
    assert_eq!(blind[3], 0, "facing away: not affected");
    assert!(!sees(&m, 2, 0), "a blinded unit sees nothing beyond its sense radius");
    assert!(row::<UnitInfo>(&m, 2, T_SELF, 0).state & S_BLINDED != 0);
    assert!(row::<UnitInfo>(&m, 3, T_SELF, 0).state & S_BLINDED == 0);

    steps(&mut m, flash.duration as usize);
    assert_eq!(m.world.units[2].blind, 0);
    assert!(sees(&m, 2, 0));
}

#[test]
fn flash_behind_smoke_blinds_nobody() {
    let mut m = setup("lancer");
    place(&mut m, 0, c(2, 3), NORTH);
    place(&mut m, 2, c(14, 3), WEST);
    m.world.units[0].abil.charges[1] = 1;
    m.world.units[2].kit = kit("shroud");
    m.world.units[2].abil.charges[1] = 1;
    assert_eq!(use_at(&mut m, 2, 1, c(11, 3)), OK);
    steps(&mut m, 2);
    assert_eq!(use_at(&mut m, 0, 1, c(8, 3)), OK);
    steps(&mut m, 2);
    assert_eq!(m.world.units[2].blind, 0, "smoke between the unit and the pop");
}

#[test]
fn incendiary_burns_everyone_inside_each_tick() {
    let mut m = setup("seeker");
    let fire = ability("incendiary");
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, c(8, 3), WEST);
    place(&mut m, 1, (c(8, 3).0 + 500, c(8, 3).1), WEST); // a careless teammate
    m.world.units[0].abil.charges[1] = 1;
    let (hp_enemy, hp_ally) = (m.world.units[2].hp, m.world.units[1].hp);

    assert_eq!(use_at(&mut m, 0, 1, c(8, 3)), OK);
    steps(&mut m, fire.delay as usize);
    assert_eq!(m.world.units[2].hp, hp_enemy, "arming");
    m.step();
    assert_eq!(m.world.units[2].hp, hp_enemy - fire.power);
    steps(&mut m, fire.duration as usize - 1);
    let total = fire.power * fire.duration;
    assert_eq!(m.world.units[2].hp, hp_enemy - total);
    assert_eq!(m.world.units[1].hp, hp_ally - total, "fire does not pick sides");
    assert_eq!(m.world.units[0].stats.damage, total, "only enemy damage is credited");
    assert!(m.world.effects.is_empty());
    m.step();
    assert_eq!(m.world.units[2].hp, hp_enemy - total, "burnt out");
}

#[test]
fn recon_reveals_enemies_the_beacon_can_see() {
    let mut m = setup("seeker");
    let recon = ability("recon");
    place(&mut m, 0, c(2, 5), WEST); // looking away from everyone
    place(&mut m, 2, c(7, 2), WEST); // in the open, 3 tiles from the beacon
    place(&mut m, 3, c(11, 5), WEST); // behind the wall tile at (9,5)
    m.step();
    assert_eq!(m.world.units[0].abil.charges[0], recon.max_charges, "free abilities are filled at round start");
    assert!(!sees(&m, 0, 2) && !sees(&m, 0, 3));

    assert_eq!(use_at(&mut m, 0, 0, c(7, 5)), OK);
    steps(&mut m, recon.delay as usize + 1);
    assert!(sees(&m, 0, 2) && sees(&m, 1, 2), "revealed to the whole team");
    assert!(!sees(&m, 0, 3), "the wall shelters unit 3 from the beacon");
    let seen: UnitInfo = row(&m, 0, T_ENEMIES, 0);
    assert!(seen.state & S_REVEALED != 0);

    steps(&mut m, (recon.duration + recon.power) as usize);
    assert!(!sees(&m, 0, 2), "the mark wears off after the beacon dies");
    assert_eq!(use_at(&mut m, 0, 0, c(7, 5)), ERR_NOT_READY, "one charge per round");
}

/// Unit 0 (bulwark) at (2,3) with a barrier two tiles in front of it; unit 2 faces it from (8,3).
fn behind_barrier() -> Match {
    let mut m = setup("bulwark");
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, c(8, 3), WEST);
    m.world.units[0].abil.charges[1] = 1;
    m.step();
    assert!(sees(&m, 2, 0));
    assert_eq!(use_at(&mut m, 0, 1, c(4, 3)), OK);
    m.step();
    m
}

#[test]
fn barrier_blocks_sight_and_movement_then_expires() {
    let mut m = behind_barrier();
    let barrier = ability("barrier");
    assert!(!sees(&m, 2, 0) && !sees(&m, 0, 2));
    let e: EffectInfo = row(&m, 2, T_EFFECTS, 0);
    assert_eq!((e.kind, e.x, e.x2), (EFFECT_BARRIER, c(4, 3).0, c(4, 3).0), "a segment across the line of fire");
    assert_eq!((e.y - e.y2).abs(), 2 * barrier.radius);

    // A teammate walking east is stopped a unit radius short of it.
    place(&mut m, 1, c(3, 3), EAST);
    for _ in 0..6 {
        m.act(1, A_MOVE, EAST, 700, 0);
        m.step();
    }
    let x = m.world.units[1].pos.0;
    assert!(x > c(3, 3).0 + 300 && x <= e.x - m.world.rules.core.unit_radius, "stopped at x = {x}");

    steps(&mut m, barrier.duration as usize - 7);
    assert!(m.world.effects.is_empty(), "expired after its duration");
    for _ in 0..3 {
        m.act(1, A_MOVE, EAST, 700, 0);
        m.step();
    }
    assert!(m.world.units[1].pos.0 > e.x, "the way is open again");
    assert!(sees(&m, 2, 0));
}

#[test]
fn barrier_stops_bullets_until_its_hit_points_run_out() {
    let mut m = behind_barrier();
    let full = m.world.units[0].hp;
    let mut broke_at = None;
    for tick in 0..ability("barrier").duration - 2 {
        m.act(2, A_FIRE, WEST, 0, 0);
        m.step();
        if m.world.effects.is_empty() {
            broke_at = Some(tick);
            break;
        }
        assert_eq!(m.world.units[0].hp, full, "the barrier takes the bullets");
    }
    assert!(broke_at.is_some(), "shot down before it would have expired");
    assert_eq!(m.world.units[0].hp, full);
    assert!(m.world.units[2].stats.shots >= 5, "it took 120 hit points of pistol fire");
    for _ in 0..4 {
        m.act(2, A_FIRE, WEST, 0, 0);
        m.step();
    }
    assert!(m.world.units[0].hp < full, "with the barrier gone the bullets land");
}

#[test]
fn barrier_cannot_be_placed_on_a_unit_and_fails_if_one_stands_in_it_when_it_arms() {
    let mut m = setup("bulwark");
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, c(4, 4), WEST);
    m.world.units[0].abil.charges[1] = 1;
    assert_eq!(use_at(&mut m, 0, 1, c(4, 3)), ERR_NOT_ALLOWED);
    assert_eq!(use_at(&mut m, 0, 1, c(2, 3)), ERR_OUT_OF_RANGE, "no direction");

    // With a delay, a unit that steps into the line before it arms makes it fail.
    let id = ability("barrier").id as usize;
    m.world.rules.abilities[id].delay = 2;
    place(&mut m, 2, c(8, 3), WEST);
    assert_eq!(use_at(&mut m, 0, 1, c(4, 3)), OK);
    m.step();
    assert_eq!(m.world.effects.len(), 1);
    place(&mut m, 2, c(4, 4), WEST);
    m.step();
    assert!(m.world.effects.is_empty());
    assert_eq!(m.world.units[0].abil.charges[1], 0, "the charge is spent all the same");
}

#[test]
fn dash_moves_instantly_and_stops_at_walls() {
    let mut m = setup("lancer");
    let dash = ability("dash");
    place(&mut m, 0, c(2, 3), EAST);
    assert_eq!(m.world.units[0].abil.charges[0], dash.max_charges);

    assert_eq!(use_at(&mut m, 0, 0, c(12, 3)), OK);
    m.step();
    assert_eq!(m.world.units[0].pos, (c(2, 3).0 + dash.range, c(2, 3).1), "the full range along the line");
    let me: UnitInfo = row(&m, 0, T_SELF, 0);
    assert_eq!((me.charges[0], me.cooldown[0]), (dash.max_charges - 1, dash.cooldown));
    m.world.units[0].abil.charges[0] = 1; // so the refusals below are the cooldown, not an empty slot
    assert_eq!(use_at(&mut m, 0, 0, c(12, 3)), ERR_NOT_READY, "cooling down");
    steps(&mut m, dash.cooldown as usize - 1);
    assert_eq!(use_at(&mut m, 0, 0, c(12, 3)), ERR_NOT_READY, "the last tick of the cooldown");
    m.step();

    // Towards the wall tile at (9,5): it ends flush against the wall, not inside it.
    place(&mut m, 0, c(7, 5), EAST);
    assert_eq!(use_at(&mut m, 0, 0, c(12, 5)), OK);
    m.step();
    let x = m.world.units[0].pos.0;
    let limit = 9 * TILE - m.world.rules.core.unit_radius;
    assert!(x > limit - 200 && x <= limit, "stopped at x = {x}");
    assert_eq!(m.world.units[0].pos.1, c(7, 5).1);

    // A short dash goes exactly to the target point.
    m.world.units[0].abil = abilities::UnitAbilities { charges: [1, 0], cooldown: [0, 0] };
    place(&mut m, 0, c(2, 3), EAST);
    assert_eq!(use_at(&mut m, 0, 0, (c(2, 3).0 + 1000, c(2, 3).1 + 1000)), OK);
    m.step();
    assert_eq!(m.world.units[0].pos, (c(2, 3).0 + 1000, c(2, 3).1 + 1000));
}

#[test]
fn decoy_sounds_real_from_afar_and_fake_up_close() {
    let mut m = setup("shroud");
    let decoy = ability("decoy");
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, c(14, 3), WEST); // 4 tiles from the decoy
    place(&mut m, 3, c(11, 3), WEST); // 1 tile from it
    assert_eq!(use_at(&mut m, 0, 0, c(10, 3)), OK);
    let mut shots = 0;
    for _ in 0..decoy.duration {
        m.step();
        let far = sounds(&m, 2);
        assert!(far.iter().all(|s| (s.x, s.y, s.team) == (c(10, 3).0, c(10, 3).1, TEAM_A)));
        assert_eq!(far.iter().filter(|s| s.kind == SOUND_STEP).count(), 1, "a footstep every tick");
        assert!(far.iter().all(|s| s.kind == SOUND_STEP || s.kind == SOUND_SHOT), "indistinguishable from afar");
        shots += far.iter().filter(|s| s.kind == SOUND_SHOT).count() as i32;
        let near = sounds(&m, 3);
        assert_eq!(near.iter().map(|s| s.kind).collect::<Vec<_>>(), vec![SOUND_DECOY], "recognised up close");
    }
    assert_eq!(shots, decoy.duration / decoy.power);
    m.step();
    assert!(sounds(&m, 2).is_empty() && m.world.effects.is_empty());
}

#[test]
fn heal_restores_an_ally_over_time_up_to_its_maximum() {
    let mut m = setup("mender");
    let heal = ability("heal");
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 1, c(4, 3), EAST);
    assert_eq!(use_at(&mut m, 0, 0, c(4, 3)), ERR_NOT_ALLOWED, "nothing to heal");
    assert_eq!(use_at(&mut m, 0, 0, c(8, 3)), ERR_OUT_OF_RANGE, "nobody near the target point");

    m.world.units[1].hp = 40;
    assert_eq!(use_at(&mut m, 0, 0, c(4, 3)), OK);
    m.step();
    assert_eq!(m.world.units[1].hp, 40 + heal.power);
    assert_eq!(m.world.effects[0].info.kind, EFFECT_HEAL);
    steps(&mut m, heal.duration as usize - 1);
    assert_eq!(m.world.units[1].hp, 40 + heal.power * heal.duration);
    assert!(m.world.effects.is_empty());

    // Self-heal, capped at the kit's hit points.
    let cap = m.world.rules.kits[kit("mender") as usize].hp;
    m.world.units[0].hp = cap - 3;
    m.world.units[0].abil.charges[0] = 1; // so the refusal below is the cooldown, not an empty slot
    assert_eq!(use_at(&mut m, 0, 0, c(2, 3)), ERR_NOT_READY, "the last tick of the cooldown");
    m.step();
    assert_eq!(use_at(&mut m, 0, 0, c(2, 3)), OK);
    steps(&mut m, 3);
    assert_eq!(m.world.units[0].hp, cap);
}

#[test]
fn thrown_devices_land_within_range_and_short_of_walls() {
    let mut m = setup("shroud");
    let smoke = ability("smoke");
    m.world.units[0].abil.charges[1] = 2;
    place(&mut m, 0, c(1, 3), EAST);
    assert_eq!(use_at(&mut m, 0, 1, c(18, 3)), OK);
    m.step();
    assert_eq!((m.world.effects[0].info.x, m.world.effects[0].info.y), (c(1, 3).0 + smoke.range, c(1, 3).1));

    // Thrown at a point behind the wall tile at (9,5), it lands just in front of the wall.
    m.world.units[0].abil.cooldown[1] = 0;
    place(&mut m, 0, c(6, 5), EAST);
    assert_eq!(use_at(&mut m, 0, 1, c(11, 5)), OK);
    m.step();
    let x = m.world.effects[1].info.x;
    assert!(x < 9 * TILE && x > 9 * TILE - 200, "landed at x = {x}");
}

#[test]
fn charges_are_validated_bought_and_refilled() {
    let mut m = setup("lancer");
    let (dash, flash) = (ability("dash"), ability("flash"));
    place(&mut m, 0, c(2, 3), EAST);
    assert_eq!(use_at(&mut m, 0, 2, c(4, 3)), ERR_BAD_ARG, "no such slot");
    assert_eq!(use_at(&mut m, 0, 1, c(4, 3)), ERR_NOT_READY, "no flash bought");
    assert_eq!(use_at(&mut m, 0, 0, (i32::MAX, i32::MIN)), ERR_BAD_ARG, "target outside the map");
    assert_eq!(use_at(&mut m, 0, 0, (-1, 0)), ERR_BAD_ARG);
    assert_eq!(use_at(&mut m, 1, 0, c(4, 3)), ERR_UNSUPPORTED, "the standard kit has no abilities");

    let w = &mut m.world;
    w.units[0].credits = flash.price * flash.max_charges + flash.price - 1;
    // Charges are bought through the shop, which takes the credits; `add_charge` only checks and grants.
    let item = w.rules.items.iter().find(|it| it.kind == ITEM_KIND_ABILITY_CHARGE && it.value == 1).unwrap().id;
    for _ in 0..flash.max_charges {
        assert_eq!(ibc_engine::economy::buy(w, 0, item), OK);
    }
    assert_eq!(w.units[0].credits, flash.price - 1, "the ability's own price is charged exactly once");
    assert_eq!(abilities::add_charge(w, 0, 1), ERR_NOT_ALLOWED, "already at max_charges");
    w.units[0].abil.charges[1] -= 1;
    assert_eq!(abilities::add_charge(w, 0, 1), ERR_INSUFFICIENT_FUNDS);
    assert_eq!(abilities::add_charge(w, 0, 0), ERR_NOT_ALLOWED, "the free slot is always full at round start");
    assert_eq!(abilities::add_charge(w, 1, 0), ERR_UNSUPPORTED);
    assert_eq!(abilities::add_charge(w, 0, 7), ERR_BAD_ARG);

    // Spend the dashes; the next round refills them and keeps the bought flash.
    assert_eq!(use_at(&mut m, 0, 0, c(4, 3)), OK);
    m.step();
    assert_eq!(m.world.units[0].abil.charges, [dash.max_charges - 1, flash.max_charges - 1]);
    let round = m.world.round;
    while m.world.round == round {
        m.step();
    }
    assert_eq!(m.world.units[0].abil.charges, [dash.max_charges, flash.max_charges - 1]);
    assert_eq!(m.world.units[0].abil.cooldown, [0, 0]);
    assert_eq!(m.world.units[0].hp, m.world.rules.kits[kit("lancer") as usize].hp, "kit hit points apply");
}

#[test]
fn every_kit_has_two_abilities_and_original_stats() {
    let r = Ruleset::builtin();
    assert_eq!(r.abilities.len(), 8);
    let mut effects: Vec<i32> = r.abilities.iter().map(|a| a.effect).collect();
    effects.sort();
    assert_eq!(effects, vec![1, 2, 3, 4, 5, 6, 7, abilities::EFFECT_DASH]);
    assert_eq!(r.kits[0].abilities, [-1, -1], "kit 0 stays the plain kit undrafted bots play");
    assert!((6..=7).contains(&r.kits.len()));
    for k in &r.kits[1..] {
        let [free, bought] = k.abilities.map(|a| r.abilities[a as usize]);
        assert_eq!(free.price, 0, "slot 0 is the kit's free ability");
        assert!(bought.price > 0, "slot 1 is bought");
        assert!(k.hp != 100 || k.speed_pct != 100, "every kit trades something");
    }
}
