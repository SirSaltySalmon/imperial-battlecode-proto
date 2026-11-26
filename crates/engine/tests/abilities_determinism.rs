//! Matches with every kit in play are deterministic and replay from their inputs alone.

use ibc_engine::abi::*;
use ibc_engine::{abilities, fixed, Input, Map, Match, Ruleset};

/// Three per team, so the six ability kits are all drafted.
const YARD: &str = "name: yard\nteam_size: 3\n---\n\
################\n\
#a............d#\n\
#a....#.......d#\n\
#a....#.......d#\n\
#.........#....#\n\
#............11#\n\
#............11#\n\
################\n";

fn row<T: Copy + Default>(m: &Match, unit: u32, table: u32, index: u32) -> T {
    let mut buf = Vec::new();
    assert!(m.get(unit, table, index, &mut buf) >= 0);
    from_bytes(&buf).unwrap()
}

/// Draft kit `1 + unit`, walk at the site, use both ability slots whenever the host allows, stop and shoot on
/// sight (a pull on a moving tick is refused).
fn controller(m: &mut Match, unit: u32) {
    let info: MatchInfo = row(m, unit, T_MATCH, 0);
    if info.phase == PHASE_DRAFT {
        assert_eq!(m.act(unit, A_DRAFT, 1 + unit as i32, 0, 0), OK);
        return;
    }
    if info.phase != PHASE_ACTION && info.phase != PHASE_PLANTED {
        return;
    }
    let me: UnitInfo = row(m, unit, T_SELF, 0);
    let site: SiteInfo = row(m, unit, T_SITES, 0);
    let goal = (site.centre_x, site.centre_y);
    let heading = fixed::atan2(goal.1 - me.y, goal.0 - me.x);
    if m.count(unit, T_ENEMIES) == 0 && fixed::dist((me.x, me.y), goal) > 600 {
        m.act(unit, A_MOVE, heading, 700, 0);
        m.act(unit, A_FACE, heading, 0, 0);
    }
    if m.count(unit, T_ENEMIES) > 0 {
        let e: UnitInfo = row(m, unit, T_ENEMIES, 0);
        let aim = fixed::atan2(e.y - me.y, e.x - me.x);
        m.act(unit, A_FACE, aim, 0, 0);
        // Throw at the enemy first, or heal ourselves; the gun is the fallback.
        let used = |m: &mut Match, slot: i32, at: (i32, i32)| m.act(unit, A_ABILITY, slot, at.0, at.1) == OK;
        if !used(m, 0, (e.x, e.y)) && !used(m, 1, (e.x, e.y)) && !used(m, 0, (me.x, me.y)) {
            m.act(unit, if me.ammo == 0 { A_RELOAD } else { A_FIRE }, aim, 0, 0);
        }
    } else if info.tick % 3 == unit as i32 % 3 {
        // Nobody in sight: a few tiles ahead (dash, recon, decoy) or on ourselves (heal).
        let ahead = (me.x + (goal.0 - me.x) / 2, me.y + (goal.1 - me.y) / 2);
        if m.act(unit, A_ABILITY, 0, ahead.0, ahead.1) != OK {
            m.act(unit, A_ABILITY, 0, me.x, me.y);
        }
    }
}

struct Run {
    summary: String,
    log: Vec<Vec<Input>>,
    hashes: Vec<u64>,
    /// EFFECT_* kinds that existed at some point, as a bitmask.
    kinds: u32,
    dashes: u32,
    /// Ticks that ended with a thrown device in the air.
    thrown: u32,
}

fn play(seed: u64, stock_bought: bool) -> Run {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(YARD).unwrap(), seed);
    let dash = m.world.rules.abilities.iter().position(|a| a.effect == abilities::EFFECT_DASH).unwrap() as i32;
    let mut run = Run { summary: String::new(), log: Vec::new(), hashes: Vec::new(), kinds: 0, dashes: 0, thrown: 0 };
    let mut round = 0;
    while !m.finished() {
        if stock_bought && m.world.round != round && m.world.phase == PHASE_ACTION {
            // Stand-in for the shop (the economy unit owns the items): one bought charge per round.
            round = m.world.round;
            for u in m.world.units.iter_mut() {
                u.abil.charges[1] = 1;
            }
        }
        for unit in m.turn_order() {
            controller(&mut m, unit);
        }
        let inputs = m.take_inputs();
        let before: Vec<_> = m.world.units.iter().map(|u| u.abil.charges[0]).collect();
        m.step();
        for (u, charges) in m.world.units.iter().zip(before) {
            let dasher = m.world.rules.kits[u.kit as usize].abilities[0] == dash;
            run.dashes += (dasher && u.abil.charges[0] < charges) as u32;
        }
        for e in &m.world.effects {
            run.kinds |= 1 << e.info.kind;
        }
        // A thrown flash pops in the tick it lands and is gone by its end: count devices in the air too.
        for p in m.world.projectiles.iter().filter(|p| p.kind != 0) {
            run.kinds |= 1 << m.world.rules.abilities[p.kind as usize - 1].effect;
        }
        run.thrown += m.world.projectiles.iter().any(|p| p.kind != 0) as u32;
        run.log.push(inputs);
        run.hashes.push(m.state_hash());
        assert!(m.tick() < 5000, "match did not finish");
    }
    run.summary = m.summary_json();
    run
}

#[test]
fn free_abilities_replay_from_inputs() {
    let a = play(11, false);
    let b = play(11, false);
    assert_eq!(a.hashes, b.hashes);
    assert_eq!(a.summary, b.summary);
    for kind in [EFFECT_RECON, EFFECT_DECOY, EFFECT_HEAL] {
        assert!(a.kinds & (1 << kind) != 0, "effect {kind} never appeared");
    }
    assert!(a.dashes > 0, "nobody dashed");

    let mut r = Match::new(Ruleset::builtin(), Map::parse(YARD).unwrap(), 11);
    for (tick, inputs) in a.log.iter().enumerate() {
        for i in inputs {
            r.apply_input(i);
        }
        r.step();
        assert_eq!(r.state_hash(), a.hashes[tick], "replay diverged at tick {tick}");
    }
    assert!(r.finished());
}

#[test]
fn all_eight_abilities_are_deterministic() {
    let a = play(5, true);
    let b = play(5, true);
    assert_eq!(a.hashes, b.hashes);
    assert_eq!(a.summary, b.summary);
    for kind in EFFECT_SMOKE..=EFFECT_DECOY {
        assert!(a.kinds & (1 << kind) != 0, "effect {kind} never appeared");
    }
    assert!(a.dashes > 0);
    assert!(a.thrown > 0, "no device was ever in the air");
    assert_ne!(play(6, true).hashes, a.hashes, "the seed is part of the state");
}
