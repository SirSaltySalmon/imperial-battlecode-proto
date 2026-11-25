//! End-to-end engine checks with tiny scripted controllers driving the same `Host` calls bots use.

use ibc_engine::abi::*;
use ibc_engine::{fixed, Input, Map, Match, Ruleset};

const OPEN: &str = "name: open\nteam_size: 2\n---\n\
##############\n\
#a..........d#\n\
#a....#.....d#\n\
#.....#......#\n\
#..........11#\n\
#..........11#\n\
##############\n";

fn get<T: Copy + Default>(m: &Match, unit: u32, table: u32, index: u32) -> Option<T> {
    let mut buf = Vec::new();
    if m.get(unit, table, index, &mut buf) < 0 {
        return None;
    }
    from_bytes(&buf)
}

/// Shoot what you see; attackers walk to the site and plant; defenders hold (or idle).
fn controller(m: &mut Match, unit: u32, defenders_idle: bool) {
    let me: UnitInfo = get(m, unit, T_SELF, 0).unwrap();
    let info: MatchInfo = get(m, unit, T_MATCH, 0).unwrap();
    if info.phase == PHASE_BUY {
        m.act(unit, A_BUY, 0, 0, 0); // rifle if affordable
        return;
    }
    if info.phase != PHASE_ACTION && info.phase != PHASE_PLANTED {
        return;
    }
    if me.side == SIDE_DEFEND && defenders_idle {
        return;
    }
    if !defenders_idle && m.count(unit, T_ENEMIES) > 0 {
        let e: UnitInfo = get(m, unit, T_ENEMIES, 0).unwrap();
        let ang = fixed::atan2(e.y - me.y, e.x - me.x);
        m.act(unit, A_FACE, ang, 0, 0);
        if me.ammo == 0 {
            m.act(unit, A_RELOAD, 0, 0, 0);
        } else {
            m.act(unit, A_FIRE, ang, 0, 0);
        }
        return;
    }
    let site: SiteInfo = get(m, unit, T_SITES, 0).unwrap();
    let target = if me.side == SIDE_ATTACK || info.phase == PHASE_PLANTED {
        if info.charge_state == 2 { (info.charge_x, info.charge_y) } else { (site.centre_x, site.centre_y) }
    } else {
        (me.x, me.y)
    };
    if me.side == SIDE_ATTACK && me.state & S_HAS_CHARGE != 0 && m.act(unit, A_PLANT, 0, 0, 0) == OK {
        return;
    }
    if me.side == SIDE_DEFEND && info.phase == PHASE_PLANTED && m.act(unit, A_DEFUSE, 0, 0, 0) == OK {
        return;
    }
    let d = fixed::dist((me.x, me.y), target);
    if d > 200 {
        let ang = fixed::atan2(target.1 - me.y, target.0 - me.x);
        m.act(unit, A_MOVE, ang, 700, 0);
        m.act(unit, A_FACE, ang, 0, 0);
    }
}

fn play(seed: u64, defenders_idle: bool) -> (Match, Vec<Vec<Input>>, Vec<u64>) {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(OPEN).unwrap(), seed);
    let (mut log, mut hashes) = (Vec::new(), Vec::new());
    while !m.finished() {
        for unit in m.turn_order() {
            controller(&mut m, unit, defenders_idle);
        }
        log.push(m.take_inputs());
        m.step();
        hashes.push(m.state_hash());
        assert!(m.tick() < 5000, "match did not finish");
    }
    (m, log, hashes)
}

#[test]
fn attackers_beat_idle_defenders_by_planting() {
    let (m, _, _) = play(1, true);
    let s: serde_json::Value = serde_json::from_str(&m.summary_json()).unwrap();
    let rounds = s["rounds"].as_array().unwrap();
    assert!(rounds.len() >= 7);
    // Whoever attacks wins every round against idle defenders (detonation or elimination).
    for r in rounds {
        assert_eq!(r["winner"], r["attackers"], "round {r}");
    }
    assert!(s["units"].as_array().unwrap().iter().any(|u| u["plants"].as_i64().unwrap() > 0 || u["kills"].as_i64().unwrap() > 0));
}

#[test]
fn fights_happen_and_matches_are_deterministic() {
    let (a, log, hashes) = play(7, false);
    let (b, _, hashes_b) = play(7, false);
    assert_eq!(hashes, hashes_b);
    assert_eq!(a.summary_json(), b.summary_json());
    let s: serde_json::Value = serde_json::from_str(&a.summary_json()).unwrap();
    let kills: i64 = s["units"].as_array().unwrap().iter().map(|u| u["kills"].as_i64().unwrap()).sum();
    assert!(kills > 0, "nobody was ever hit: {s}");
    assert!(a.winner().is_some());

    // Replay: the recorded inputs alone reproduce every state hash.
    let mut r = Match::new(Ruleset::builtin(), Map::parse(OPEN).unwrap(), 7);
    for (tick, inputs) in log.iter().enumerate() {
        for i in inputs {
            r.apply_input(i);
        }
        r.step();
        assert_eq!(r.state_hash(), hashes[tick], "replay diverged at tick {tick}");
    }
    assert!(r.finished());
}

#[test]
fn fog_hides_enemies_behind_walls_and_outside_the_cone() {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(OPEN).unwrap(), 3);
    for _ in 0..5 {
        m.step(); // draft + buy
    }
    // Unit 0 spawns on the open row 1; unit 1 on row 2, which has a wall tile at (6,2) in the way. Vision is
    // shared, so unit 1 reads what unit 0 sees.
    let seen: Vec<UnitInfo> = (0..m.count(0, T_ENEMIES)).map(|i| get(&m, 0, T_ENEMIES, i as u32).unwrap()).collect();
    assert!(!seen.is_empty(), "unit 0 sees along the open row");
    assert!(seen.iter().all(|e| !m.world.can_see_point(1, (e.x, e.y))), "unit 1 is behind the wall");
    assert_eq!(m.count(1, T_ENEMIES), seen.len() as i32, "unit 1 shares unit 0's sight");
    // Turn unit 0 away: then nobody on the team sees anyone.
    for _ in 0..4 {
        m.act(0, A_FACE, 32768, 0, 0);
        m.step();
    }
    assert_eq!(m.count(0, T_ENEMIES), 0);
    assert_eq!(m.count(1, T_ENEMIES), 0);
    // Allies are always known.
    assert_eq!(m.count(0, T_ALLIES), 1);
}
