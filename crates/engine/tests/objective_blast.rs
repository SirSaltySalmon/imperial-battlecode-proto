//! The detonation blast (plain distance, through walls, both teams, no attacker) and the round-start wipe of
//! team memory.

use ibc_engine::abi::*;
use ibc_engine::{fixed, Map, Match, Ruleset};

/// Site 1 at the west end, a wall column at x = 7, open floor to the east.
const LONG: &str = "name: long\nteam_size: 2\n---\n\
##############################\n\
#a.....#.....................#\n\
#a.....#.....................#\n\
#11....#.....................#\n\
#11....#.....................#\n\
#......#...................dd#\n\
##############################\n";

fn tile_centre(tx: i32, ty: i32) -> (i32, i32) {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

fn to_phase(m: &mut Match, phase: i32) {
    while m.world.phase != phase {
        assert!(!m.finished(), "match ended while waiting for phase {phase}");
        m.step();
    }
}

/// A primary weapon that is not the free sidearm, so a death drops it.
fn primary(r: &Ruleset) -> i32 {
    (0..r.weapons.len() as i32)
        .find(|&i| i != r.econ.default_sidearm && r.weapons[i as usize].slot == 0)
        .expect("a primary weapon")
}

/// Plant the charge at `pos` with one tick of fuse left, so the next step detonates it.
fn arm(m: &mut Match, pos: (i32, i32)) {
    for u in m.world.units.iter_mut() {
        u.has_charge = false;
    }
    m.world.phase = PHASE_PLANTED;
    m.world.phase_left = 1;
    m.world.charge_state = 2; // planted
    m.world.charge_pos = pos;
}

#[test]
fn blast_kills_inside_the_radius_through_walls_and_spares_outside() {
    let r = Ruleset::builtin();
    let radius = r.core.blast_radius;
    assert_eq!(
        radius,
        12 * TILE,
        "rules/core.toml: blast_radius is 12 tiles"
    );
    let rifle = primary(&r);
    let mut m = Match::new(r.clone(), Map::parse(LONG).unwrap(), 1);
    to_phase(&mut m, PHASE_ACTION);
    let attackers = m.world.team_with_side(SIDE_ATTACK);
    assert_eq!(attackers, TEAM_A);

    let charge = tile_centre(1, 3);
    // 11.9 tiles due east, behind the wall column; 12.1 tiles away to the south-east.
    let inside = (charge.0 + 11_900 * TILE / 1000, charge.1);
    let outside = (charge.0 + 12_220, charge.1 + 2 * TILE);
    assert!(fixed::dist2(charge, inside) < (radius as i64).pow(2));
    assert!(fixed::dist2(charge, outside) > (radius as i64).pow(2));
    assert!(
        m.world.map.segment_wall(charge, inside).is_some(),
        "a wall stands between the charge and the unit it kills"
    );
    let far_attacker = tile_centre(25, 1);
    for (u, pos) in [
        (0, (charge.0 + TILE, charge.1)),
        (1, far_attacker),
        (2, inside),
        (3, outside),
    ] {
        m.world.units[u].pos = pos;
        m.world.units[u].weapons[0] = rifle;
    }
    let credits: Vec<i32> = m.world.units.iter().map(|u| u.credits).collect();
    arm(&mut m, charge);
    m.step();

    let alive: Vec<bool> = m.world.units.iter().map(|u| u.alive).collect();
    assert_eq!(alive, [false, true, false, true], "both teams are hit");
    let last = m.world.rounds.last().unwrap();
    assert_eq!(
        (last.winner, last.reason),
        (TEAM_A, ROUND_WIN_DETONATION),
        "the attackers still win, even the ones the blast killed"
    );

    // No attacker: kill events name nobody and carry no weapon, nobody is credited a kill or paid for one.
    let kinds: Vec<i32> = m.world.events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [EVENT_DETONATE, EVENT_KILL, EVENT_KILL, EVENT_ROUND_END]
    );
    for e in m.world.events.iter().filter(|e| e.kind == EVENT_KILL) {
        assert_eq!((e.actor, e.value), (-1, -1));
    }
    assert!(m.world.units.iter().all(|u| u.stats.kills == 0));
    for u in 0..4 {
        let won = m.world.units[u].team == TEAM_A;
        let paid = m.world.units[u].credits - credits[u];
        if won {
            assert_eq!(paid, r.econ.win_reward, "unit {u}: the win reward only");
        } else {
            assert!(paid < r.econ.win_reward, "unit {u}: loss income only");
        }
    }
    // The dead drop their guns where they stood; next round they start with the sidearm.
    let dropped: Vec<(i32, i32)> = m
        .world
        .drops
        .iter()
        .filter(|d| d.kind == DROP_WEAPON)
        .map(|d| (d.x, d.y))
        .collect();
    assert_eq!(dropped, [(charge.0 + TILE, charge.1), inside]);
    to_phase(&mut m, PHASE_BUY);
    let primaries: Vec<i32> = m.world.units.iter().map(|u| u.weapons[0]).collect();
    assert_eq!(primaries, [-1, rifle, -1, rifle]);
}

#[test]
fn the_blast_radius_is_part_of_the_ruleset_hash() {
    let mut r = Ruleset::builtin();
    let h = r.hash();
    r.core.blast_radius += 1;
    assert_ne!(h, r.hash());
}

#[test]
fn team_memory_is_wiped_at_every_round_start_and_draft_memory_is_kept() {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(LONG).unwrap(), 1);
    m.world.draft_mem[0][0] = 9;
    m.world.draft_mem[1][5] = 4;
    for round in 1..=3 {
        to_phase(&mut m, PHASE_ACTION);
        assert_eq!(m.world.round, round);
        for t in 0..2 {
            m.world.team_mem[t].fill(0xAB);
        }
        m.world.end_round(TEAM_B, ROUND_WIN_TIMEOUT);
        assert!(
            m.world
                .team_mem
                .iter()
                .all(|t| t.iter().all(|&b| b == 0xAB)),
            "kept until the round is over"
        );
        to_phase(&mut m, PHASE_BUY);
        for t in 0..2 {
            assert!(
                m.world.team_mem[t].iter().all(|&b| b == 0),
                "round {} starts with team {t}'s memory zeroed",
                m.world.round
            );
        }
        assert_eq!((m.world.draft_mem[0][0], m.world.draft_mem[1][5]), (9, 4));
    }
}
