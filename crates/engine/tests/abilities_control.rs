//! Trap, sonar and concussion: one scenario per rule (trigger, fog, friendly fire, expiry), and a match with
//! the three kits that carry them, played twice and replayed from its inputs. A use waits out its startup
//! (`delay`, unit 5) and then appears where it lands, active at once.

use ibc_engine::abi::*;
use ibc_engine::{fixed, Input, Map, Match, Ruleset};

/// 20x9, two per team, one wall tile at (9,5), the site at (17..18, 7).
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

fn ability_id(name: &str) -> usize {
    Ruleset::builtin().ability_names.iter().position(|n| n == name).expect("ability exists")
}

fn ability(name: &str) -> AbilityRules {
    Ruleset::builtin().abilities[ability_id(name)]
}

/// A match in its first action tick with the given drafts; everyone else plays kit 0. Every unit is then put
/// somewhere harmless: units 1 and 3 in corners looking at the wall, 0 and 2 on row 1.
fn setup(drafts: &[(u32, &str)]) -> Match {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(ARENA).unwrap(), 1);
    for &(unit, name) in drafts {
        assert_eq!(m.act(unit, A_DRAFT, kit(name), 0, 0), OK);
    }
    while m.world.phase != PHASE_ACTION {
        m.step();
    }
    place(&mut m, 0, c(3, 1), NORTH);
    place(&mut m, 1, c(1, 7), SOUTH);
    place(&mut m, 2, c(15, 1), NORTH);
    place(&mut m, 3, c(18, 1), NORTH);
    m.step();
    m
}

fn place(m: &mut Match, unit: usize, pos: (i32, i32), facing: i32) {
    m.world.units[unit].pos = pos;
    m.world.units[unit].facing = facing;
}

fn steps(m: &mut Match, n: i32) {
    for _ in 0..n {
        m.step();
    }
}

fn row<T: Copy + Default>(m: &Match, unit: u32, table: u32, index: u32) -> T {
    let mut buf = Vec::new();
    assert!(m.get(unit, table, index, &mut buf) >= 0);
    from_bytes(&buf).unwrap()
}

fn me(m: &Match, unit: u32) -> UnitInfo {
    row(m, unit, T_SELF, 0)
}

fn sees(m: &Match, unit: u32, enemy: i32) -> bool {
    (0..m.count(unit, T_ENEMIES)).any(|i| row::<UnitInfo>(m, unit, T_ENEMIES, i as u32).id == enemy)
}

fn use_at(m: &mut Match, unit: u32, slot: i32, at: (i32, i32)) -> i32 {
    m.act(unit, A_ABILITY, slot, at.0, at.1)
}

fn kinds(m: &Match) -> Vec<i32> {
    m.world.effects.iter().map(|e| e.info.kind).collect()
}

// ---------------------------------------------------------------- trap

#[test]
fn trap_is_hidden_from_enemies_until_close_then_springs_on_them() {
    let trap = ability("trap");
    let mut m = setup(&[(0, "trapper")]);
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, c(12, 3), WEST);
    assert_eq!(use_at(&mut m, 0, 0, c(4, 3)), OK);
    steps(&mut m, trap.delay + 1);
    assert_eq!(kinds(&m), [EFFECT_TRAP]);
    assert_eq!(row::<EffectInfo>(&m, 1, T_EFFECTS, 0).kind, EFFECT_TRAP, "the owner's team always sees it");
    assert!(sees(&m, 2, 0), "the enemy looks straight at the trap and the thrower");
    assert_eq!(m.count(2, T_EFFECTS), 0, "but not at the trap from 8 tiles away");

    // Run at it. Within sense_radius it shows; inside its radius it springs.
    let sense = m.world.rules.core.sense_radius as i64;
    let full = m.world.max_speed(2);
    let mut spotted = false;
    for _ in 0..40 {
        if m.world.effects.is_empty() {
            break;
        }
        let d = fixed::dist(m.world.units[2].pos, c(4, 3)) as i64;
        spotted |= d <= sense && d > trap.radius as i64 && m.count(2, T_EFFECTS) == 1;
        m.act(2, A_MOVE, WEST, m.world.rules.core.run_speed, 0);
        m.step();
    }
    assert!(m.world.effects.is_empty(), "the trap never sprang");
    assert!(spotted, "the trap showed once the enemy came within sense_radius");
    let victim = me(&m, 2);
    assert!(victim.state & S_SLOWED != 0 && victim.state & S_REVEALED != 0);
    assert_eq!((m.world.units[2].slowed, m.world.units[2].revealed), (trap.power, trap.power));
    assert!(sees(&m, 1, 2), "the trapper's teammate sees the victim through the reveal");
    assert_eq!(m.world.units[0].slowed, 0);

    // Slowed means half speed for `power` turns.
    let x = m.world.units[2].pos.0;
    m.act(2, A_MOVE, WEST, m.world.rules.core.run_speed, 0);
    m.step();
    assert_eq!(x - m.world.units[2].pos.0, full / 2, "moved at half speed");
    steps(&mut m, trap.power);
    assert_eq!((m.world.units[2].slowed, m.world.units[2].revealed), (0, 0));
}

#[test]
fn trap_catches_teammates_and_a_dash_cannot_jump_it() {
    let trap = ability("trap");
    let mut m = setup(&[(0, "trapper"), (1, "lancer")]);
    place(&mut m, 0, c(2, 5), NORTH);
    place(&mut m, 1, c(6, 3), WEST);
    m.step();
    assert_eq!(use_at(&mut m, 0, 0, c(4, 3)), OK);
    steps(&mut m, trap.delay + 1);
    assert_eq!(kinds(&m), [EFFECT_TRAP]);

    // The teammate dashes 3 tiles west, from 2 tiles east of the trap to 1 tile west of it: both ends are
    // outside the trigger radius, the path is not.
    let from = m.world.units[1].pos;
    assert_eq!(use_at(&mut m, 1, 0, c(2, 3)), OK);
    m.step();
    let to = m.world.units[1].pos;
    assert!(to.0 < c(4, 3).0 - trap.radius && from.0 > c(4, 3).0 + trap.radius);
    assert!(m.world.effects.is_empty(), "the trap sprang and is spent");
    assert_eq!(m.world.units[1].slowed, trap.power, "friendly fire: the teammate is caught");
    assert!(sees(&m, 2, 1), "and revealed to the enemy");
}

#[test]
fn trap_does_nothing_during_its_startup_and_expires_unsprung() {
    let mut m = setup(&[(0, "trapper")]);
    let id = ability_id("trap");
    m.world.rules.abilities[id].duration = 5;
    let trap = m.world.rules.abilities[id];
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, c(4, 3), WEST);
    assert_eq!(use_at(&mut m, 0, 0, c(4, 3)), OK);
    steps(&mut m, trap.delay);
    assert!(kinds(&m).is_empty(), "nothing in the world during the startup");
    assert!(me(&m, 0).state & S_CASTING != 0);
    assert_eq!(m.world.units[2].slowed, 0);
    m.step();
    assert!(m.world.effects.is_empty(), "placed under a unit: springs at once");
    assert_eq!(m.world.units[2].slowed, trap.power);

    // A second round with nobody near: it lies for `duration` ticks and goes.
    let round = m.world.round;
    while m.world.round == round || m.world.phase != PHASE_ACTION {
        m.step();
    }
    place(&mut m, 0, c(2, 3), EAST);
    m.step();
    assert_eq!(me(&m, 0).charges[0], 1, "the signature is refilled every round");
    assert_eq!(use_at(&mut m, 0, 0, c(4, 3)), OK);
    steps(&mut m, trap.delay + trap.duration - 1);
    assert_eq!(kinds(&m), [EFFECT_TRAP], "one tick left");
    m.step();
    assert!(m.world.effects.is_empty());
    assert!(m.world.units.iter().all(|u| u.slowed == 0));
}

// ---------------------------------------------------------------- sonar

#[test]
fn sonar_reveals_everyone_in_range_through_walls_teammates_included() {
    let sonar = ability("sonar");
    let mut m = setup(&[(0, "echo")]);
    place(&mut m, 0, c(2, 3), WEST);
    place(&mut m, 1, c(7, 4), WEST); // a teammate 2 tiles from the pulse
    place(&mut m, 2, c(10, 5), EAST); // an enemy behind the wall tile at (9, 5), facing it
    m.step();
    assert!(!sees(&m, 0, 2) && !sees(&m, 1, 2) && !sees(&m, 2, 1));
    assert_eq!(use_at(&mut m, 0, 0, c(8, 5)), OK);
    for _ in 0..sonar.delay {
        m.step();
        assert!(kinds(&m).is_empty() && me(&m, 0).state & S_CASTING != 0, "in its startup");
        assert_eq!(m.world.units[2].revealed, 0);
    }
    m.step();
    assert!(m.world.effects.is_empty(), "a pulse lasts one tick");
    assert_eq!(m.world.units[2].revealed, sonar.power, "the wall does not stop it");
    assert_eq!(m.world.units[1].revealed, sonar.power, "nor does it spare a teammate");
    assert_eq!(m.world.units[0].revealed, 0, "the caster is out of its radius");
    assert_eq!(m.world.units[3].revealed, 0);
    assert!(sees(&m, 0, 2), "the caster sees the enemy behind the wall");
    assert!(sees(&m, 2, 1), "and the enemy sees the caster's teammate");
    steps(&mut m, sonar.power);
    assert!(!sees(&m, 0, 2) && !sees(&m, 2, 1), "the marks wear off");
}

// ---------------------------------------------------------------- concussion

#[test]
fn concussion_breaks_a_plant_and_a_reload_and_slows_both_teams() {
    let conc = ability("concussion");
    let mut m = setup(&[(2, "breaker")]);
    let carrier = m.world.units.iter().position(|u| u.has_charge).unwrap();
    assert_eq!(m.world.units[carrier].team, 0, "team A attacks first on this map");
    place(&mut m, carrier, c(17, 7), NORTH);
    place(&mut m, 2, c(13, 7), EAST);
    place(&mut m, 3, c(16, 6), WEST); // the caster's teammate, inside the pop
    m.world.units[2].abil.charges[1] = 1;
    let slot = m.world.units[3].slot;
    m.world.units[3].ammo[slot] = 1;
    assert_eq!(m.act(carrier as u32, A_PLANT, 0, 0, 0), OK);
    m.step();
    assert!(me(&m, carrier as u32).state & S_PLANTING != 0);

    assert_eq!(use_at(&mut m, 2, 1, c(17, 7)), OK);
    assert_eq!(m.act(3, A_RELOAD, 0, 0, 0), OK);
    m.step();
    assert!(kinds(&m).is_empty() && me(&m, 2).state & S_CASTING != 0, "in its startup");
    assert!(me(&m, carrier as u32).state & S_PLANTING != 0, "still planting during the startup");
    assert!(me(&m, 3).state & S_RELOADING != 0);
    steps(&mut m, conc.delay);
    assert!(m.world.effects.is_empty(), "a pop lasts one tick");
    let planter = me(&m, carrier as u32);
    assert!(planter.state & S_PLANTING == 0 && planter.channel == 0, "the plant is lost");
    assert!(m.world.units[carrier].has_charge);
    assert_eq!(m.world.units[carrier].slowed, conc.power);
    let mate = me(&m, 3);
    assert!(mate.state & S_RELOADING == 0 && mate.ammo == 1, "friendly fire: the reload is lost too");
    assert_eq!(m.world.units[3].slowed, conc.power);
    assert_eq!(m.world.units[2].slowed, 0, "the thrower is 4 tiles away");
    steps(&mut m, conc.power);
    assert!(m.world.units.iter().all(|u| u.slowed == 0));
}

#[test]
fn concussion_is_stopped_by_walls() {
    let mut m = setup(&[(0, "breaker")]);
    m.world.units[0].abil.charges[1] = 1;
    place(&mut m, 0, c(5, 5), EAST);
    place(&mut m, 2, c(10, 5), WEST); // behind the wall tile at (9, 5), within the pop's radius
    place(&mut m, 3, c(8, 6), WEST); // in the open, within it too
    assert_eq!(use_at(&mut m, 0, 1, c(8, 5)), OK);
    steps(&mut m, 1 + ability("concussion").delay);
    assert_eq!(m.world.units[2].slowed, 0);
    assert!(m.world.units[3].slowed > 0);
}

// ---------------------------------------------------------------- a match

/// Three per team, so trapper, echo and breaker are all drafted on both sides.
const YARD: &str = "name: yard\nteam_size: 3\n---\n\
################\n\
#a............d#\n\
#a....#.......d#\n\
#a....#.......d#\n\
#.........#....#\n\
#............11#\n\
#............11#\n\
################\n";

/// Draft trapper, echo or breaker, walk at the site, use the abilities whenever the host allows: the trap
/// at our own feet, sonar ahead, the concussion on an enemy; shoot on sight.
fn controller(m: &mut Match, unit: u32) {
    let info: MatchInfo = row(m, unit, T_MATCH, 0);
    if info.phase == PHASE_DRAFT {
        let name = ["trapper", "echo", "breaker"][unit as usize % 3];
        assert_eq!(m.act(unit, A_DRAFT, kit(name), 0, 0), OK);
        return;
    }
    if info.phase != PHASE_ACTION && info.phase != PHASE_PLANTED {
        return;
    }
    let me: UnitInfo = row(m, unit, T_SELF, 0);
    let site: SiteInfo = row(m, unit, T_SITES, 0);
    let goal = (site.centre_x, site.centre_y);
    let heading = fixed::atan2(goal.1 - me.y, goal.0 - me.x);
    if fixed::dist((me.x, me.y), goal) > 600 {
        m.act(unit, A_MOVE, heading, 700, 0);
        m.act(unit, A_FACE, heading, 0, 0);
    }
    if m.count(unit, T_ENEMIES) > 0 {
        let e: UnitInfo = row(m, unit, T_ENEMIES, 0);
        let aim = fixed::atan2(e.y - me.y, e.x - me.x);
        m.act(unit, A_FACE, aim, 0, 0);
        if m.act(unit, A_ABILITY, 1, e.x, e.y) != OK {
            m.act(unit, if me.ammo == 0 { A_RELOAD } else { A_FIRE }, aim, 0, 0);
        }
    } else if info.tick % 4 == unit as i32 % 4 {
        let ahead = (me.x + (goal.0 - me.x) / 2, me.y + (goal.1 - me.y) / 2);
        m.act(unit, A_ABILITY, 0, ahead.0, ahead.1);
    }
}

struct Run {
    summary: String,
    log: Vec<Vec<Input>>,
    hashes: Vec<u64>,
    kinds: u32,
    slowed: u32,
    revealed: u32,
}

/// Stand-in for the shop (the economy unit owns the items): one bought charge each at the start of a round.
/// Not an input, so a replay of the run has to do the same.
fn stock(m: &mut Match, round: &mut i32) {
    if m.world.round != *round && m.world.phase == PHASE_ACTION {
        *round = m.world.round;
        for u in m.world.units.iter_mut() {
            u.abil.charges[1] = 1;
        }
    }
}

fn play(seed: u64) -> Run {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(YARD).unwrap(), seed);
    let mut run = Run { summary: String::new(), log: Vec::new(), hashes: Vec::new(), kinds: 0, slowed: 0, revealed: 0 };
    let mut round = 0;
    while !m.finished() {
        stock(&mut m, &mut round);
        for unit in m.turn_order() {
            controller(&mut m, unit);
        }
        let inputs = m.take_inputs();
        let before: Vec<[i32; 2]> = m.world.units.iter().map(|u| u.abil.charges).collect();
        m.step();
        for e in &m.world.effects {
            run.kinds |= 1 << e.info.kind;
        }
        // A sonar pulse or a concussion appears and is gone within one step: count the use itself.
        for (u, had) in m.world.units.iter().zip(before) {
            for slot in 0..2 {
                let id = m.world.rules.kits[u.kit as usize].abilities[slot];
                if id >= 0 && u.abil.charges[slot] < had[slot] {
                    run.kinds |= 1 << m.world.rules.abilities[id as usize].effect;
                }
            }
        }
        run.slowed += m.world.units.iter().filter(|u| u.slowed > 0).count() as u32;
        run.revealed += m.world.units.iter().filter(|u| u.revealed > 0).count() as u32;
        run.log.push(inputs);
        run.hashes.push(m.state_hash());
        assert!(m.tick() < 5000, "match did not finish");
    }
    run.summary = m.summary_json();
    run
}

#[test]
fn the_new_kits_play_deterministically_and_replay_from_inputs() {
    let a = play(3);
    let b = play(3);
    assert_eq!(a.hashes, b.hashes);
    assert_eq!(a.summary, b.summary);
    for kind in [EFFECT_TRAP, EFFECT_SONAR, EFFECT_CONCUSSION] {
        assert!(a.kinds & (1 << kind) != 0, "effect {kind} never appeared");
    }
    assert!(a.slowed > 0 && a.revealed > 0, "nobody was ever slowed or revealed");

    let mut r = Match::new(Ruleset::builtin(), Map::parse(YARD).unwrap(), 3);
    let mut round = 0;
    for (tick, inputs) in a.log.iter().enumerate() {
        stock(&mut r, &mut round);
        for i in inputs {
            r.apply_input(i);
        }
        r.step();
        assert_eq!(r.state_hash(), a.hashes[tick], "replay diverged at tick {tick}");
    }
    assert!(r.finished());
    assert_ne!(play(4).hashes, a.hashes, "the seed is part of the state");
}
