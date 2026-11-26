//! Match flow: regulation, overtime pairs, the tie-break fallback, the tick bound, and a fully scripted match
//! that goes 6-6 and is decided in overtime, replayed from its inputs.

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

fn new_match(rules: Ruleset, seed: u64) -> Match {
    Match::new(rules, Map::parse(OPEN).unwrap(), seed)
}

/// Decide the current round and step to the next buy phase (or the end of the match).
fn play_round(m: &mut Match, winner: i32) {
    while m.world.phase != PHASE_ACTION {
        assert!(!m.finished());
        m.step();
    }
    m.world.end_round(winner, ROUND_WIN_ELIMINATION);
    while m.world.phase == PHASE_ROUND_END {
        m.step();
    }
}

/// Play regulation to a level score: whoever attacks wins.
fn level_regulation(m: &mut Match) {
    let regulation = 2 * m.world.rules.core.rounds_per_half;
    for _ in 0..regulation {
        to_buy(m);
        let attackers = m.world.team_with_side(SIDE_ATTACK);
        play_round(m, attackers);
    }
    assert_eq!(m.score(), [regulation / 2, regulation / 2]);
    assert!(!m.finished(), "a level regulation goes to overtime");
}

fn to_buy(m: &mut Match) {
    while m.world.phase != PHASE_BUY {
        assert!(!m.finished());
        m.step();
    }
}

#[test]
fn regulation_ends_at_rounds_to_win() {
    let r = Ruleset::builtin();
    let mut m = new_match(r.clone(), 1);
    for round in 1..=r.core.rounds_to_win {
        assert!(!m.finished(), "still playing before round {round}");
        play_round(&mut m, TEAM_B);
    }
    assert_eq!(
        (m.winner(), m.score()),
        (Some(TEAM_B), [0, r.core.rounds_to_win])
    );
}

#[test]
fn regulation_that_ends_unlevel_goes_to_the_leader_without_overtime() {
    let mut r = Ruleset::builtin();
    r.core.rounds_to_win = 99; // unreachable: only the round count ends regulation
    let regulation = 2 * r.core.rounds_per_half;
    let mut m = new_match(r, 1);
    for round in 0..regulation {
        play_round(&mut m, if round < 5 { TEAM_A } else { TEAM_B });
    }
    assert_eq!((m.winner(), m.score()), (Some(TEAM_B), [5, 7]));
}

#[test]
fn overtime_is_played_in_pairs_until_one_team_takes_both() {
    let r = Ruleset::builtin();
    let regulation = 2 * r.core.rounds_per_half;
    let mut m = new_match(r.clone(), 1);
    level_regulation(&mut m);

    // Pair 1 is split. A leads 7-6 in between, which is `rounds_to_win`, but the pair is played out.
    assert_eq!(
        (m.world.round, m.world.side_of(TEAM_A)),
        (regulation + 1, SIDE_ATTACK)
    );
    play_round(&mut m, TEAM_A);
    assert!(
        !m.finished(),
        "7-6 inside an overtime pair does not end the match"
    );
    assert_eq!(
        (m.world.round, m.world.side_of(TEAM_A)),
        (regulation + 2, SIDE_DEFEND)
    );
    play_round(&mut m, TEAM_B);
    assert!(!m.finished());
    assert_eq!(m.score(), [7, 7]);

    // Pair 2: B takes its defence round, then its attack round.
    assert_eq!(m.world.side_of(TEAM_A), SIDE_ATTACK);
    play_round(&mut m, TEAM_B);
    assert!(!m.finished());
    assert_eq!(m.world.side_of(TEAM_B), SIDE_ATTACK);
    play_round(&mut m, TEAM_B);
    assert_eq!((m.winner(), m.score()), (Some(TEAM_B), [7, 9]));
}

#[test]
fn every_overtime_round_starts_fresh_with_fixed_credits() {
    let r = Ruleset::builtin();
    let mut m = new_match(r.clone(), 1);
    level_regulation(&mut m);
    for ot_round in 0..4 {
        to_buy(&mut m);
        for u in &m.world.units {
            assert_eq!(
                u.credits, r.flow.overtime_credits,
                "overtime round {ot_round}"
            );
            assert_eq!(
                (u.weapons, u.armor, u.has_kit),
                ([-1, r.econ.default_sidearm], 0, false)
            );
        }
        assert_eq!(m.world.econ[0].loss_streak + m.world.econ[1].loss_streak, 0);
        // A full buy fits, and a survivor's gear does not carry into the next overtime round.
        assert_eq!(m.act(0, A_BUY, 0, 0, 0), OK);
        assert_eq!(m.act(0, A_BUY, 2, 0, 0), OK);
        let attackers = m.world.team_with_side(SIDE_ATTACK);
        play_round(&mut m, attackers);
    }
    assert!(!m.finished());
}

/// Play every overtime pair as a split (whoever attacks wins), up to the cap.
fn split_every_pair(m: &mut Match) {
    for pair in 0..m.world.rules.flow.overtime_max_pairs {
        assert!(!m.finished(), "pair {pair} is played");
        let attackers = m.world.team_with_side(SIDE_ATTACK);
        play_round(m, attackers);
        assert!(!m.finished());
        let attackers = m.world.team_with_side(SIDE_ATTACK);
        play_round(m, attackers);
    }
}

#[test]
fn split_pairs_fall_back_to_the_tiebreak_after_the_cap() {
    let r = Ruleset::builtin();
    let regulation = 2 * r.core.rounds_per_half;
    // With the score level, the tie-break goes to the team with more kills, whatever the seed.
    for seed in [4u64, 5] {
        let mut m = new_match(r.clone(), seed);
        level_regulation(&mut m);
        m.world.units[2].stats.kills += 1;
        split_every_pair(&mut m);
        assert!(m.finished());
        assert_eq!(m.world.round, regulation + 2 * r.flow.overtime_max_pairs);
        assert_eq!(m.winner(), Some(TEAM_B));
    }

    // Kills level: damage decides.
    let mut m = new_match(r.clone(), 4);
    level_regulation(&mut m);
    m.world.units[0].stats.damage += 1;
    split_every_pair(&mut m);
    assert_eq!(m.winner(), Some(TEAM_A));
}

#[test]
fn a_fully_level_tiebreak_is_a_draw() {
    let r = Ruleset::builtin();
    for seed in [4u64, 5] {
        let mut m = new_match(r.clone(), seed);
        level_regulation(&mut m);
        // Equal kills and damage on both sides: nothing separates the teams, and the seed does not either.
        for u in [0, 2] {
            m.world.units[u].stats.kills += 2;
            m.world.units[u].stats.damage += 150;
        }
        split_every_pair(&mut m);
        assert!(m.finished());
        assert_eq!((m.winner(), m.score()), (None, [9, 9]), "seed {seed}");
        assert_eq!(m.world.phase, PHASE_MATCH_END);
        let s: serde_json::Value = serde_json::from_str(&m.summary_json()).unwrap();
        assert!(s["winner"].is_null());
    }
}

#[test]
fn running_out_of_ticks_is_a_draw_whatever_the_score() {
    let mut r = Ruleset::builtin();
    r.core.rounds_to_win = 99;
    let mut m = new_match(r, 1);
    play_round(&mut m, TEAM_A);
    play_round(&mut m, TEAM_A);
    m.world.units[0].stats.kills += 5;
    assert_eq!(m.score(), [2, 0]);
    m.world.rules.core.max_ticks = m.tick() as i32 + 3;
    for _ in 0..3 {
        assert!(!m.finished());
        m.step();
    }
    assert!(m.finished());
    assert_eq!(
        (m.winner(), m.tick() as i32),
        (None, m.world.rules.core.max_ticks)
    );
}

#[test]
fn overtime_can_be_switched_off() {
    let mut r = Ruleset::builtin();
    r.flow.overtime_max_pairs = 0;
    let regulation = 2 * r.core.rounds_per_half;
    let mut m = new_match(r, 5);
    for _ in 0..regulation {
        let attackers = m.world.team_with_side(SIDE_ATTACK);
        play_round(&mut m, attackers);
    }
    // 6-6 with nothing else scored goes straight to the tie-break, which is level: a draw.
    assert!(m.finished());
    assert_eq!((m.world.round, m.winner()), (regulation, None));
}

/// `max_ticks` must cover the draft tick, regulation and every overtime round at its longest: buy phase, a
/// plant on the last action tick, the whole fuse, and the round-end pause.
#[test]
fn max_ticks_bounds_the_longest_possible_match() {
    let r = Ruleset::builtin();
    let c = r.core;
    let longest_round = c.buy_ticks + c.round_ticks + c.fuse_ticks + r.round_end_ticks;
    let rounds = 2 * c.rounds_per_half + 2 * r.flow.overtime_max_pairs;
    assert!(
        1 + rounds * longest_round <= c.max_ticks,
        "{rounds} rounds of up to {longest_round} ticks do not fit in max_ticks = {}",
        c.max_ticks
    );
}

fn get<T: Copy + Default>(m: &Match, unit: u32, table: u32) -> T {
    let mut buf = Vec::new();
    assert!(m.get(unit, table, 0, &mut buf) >= 0);
    from_bytes(&buf).unwrap()
}

/// Team A always plays: buys what it can, walks the charge to the site and plants when attacking, and stands
/// still when defending. Team B plays the same way in regulation and does nothing at all in overtime.
/// Nobody ever shoots, so attackers win every regulation round (6-6) and team A wins both overtime rounds:
/// its attack by detonation, its defence on the clock.
fn controller(m: &mut Match, unit: u32) {
    let me: UnitInfo = get(m, unit, T_SELF);
    let info: MatchInfo = get(m, unit, T_MATCH);
    let core: CoreRules = get(m, unit, T_RULES_CORE);
    let overtime = info.round > 2 * core.rounds_per_half;
    if overtime && me.team == TEAM_B {
        return;
    }
    if info.phase == PHASE_BUY {
        for item in [0, 2, 1] {
            m.act(unit, A_BUY, item, 0, 0);
        }
        return;
    }
    if info.phase != PHASE_ACTION || me.side != SIDE_ATTACK {
        return;
    }
    if me.state & S_HAS_CHARGE != 0 && m.act(unit, A_PLANT, 0, 0, 0) == OK {
        return;
    }
    let site: SiteInfo = get(m, unit, T_SITES);
    let target = (site.centre_x, site.centre_y);
    if fixed::dist((me.x, me.y), target) > 200 {
        let ang = fixed::atan2(target.1 - me.y, target.0 - me.x);
        m.act(unit, A_MOVE, ang, core.run_speed, 0);
    }
}

fn play_scripted(seed: u64) -> (Match, Vec<Vec<Input>>, Vec<u64>) {
    let mut m = new_match(Ruleset::builtin(), seed);
    let (mut log, mut hashes) = (Vec::new(), Vec::new());
    while !m.finished() {
        for unit in m.turn_order() {
            controller(&mut m, unit);
        }
        log.push(m.take_inputs());
        m.step();
        hashes.push(m.state_hash());
    }
    (m, log, hashes)
}

#[test]
fn scripted_match_goes_six_all_and_is_decided_in_overtime() {
    let (m, log, hashes) = play_scripted(11);
    let r = Ruleset::builtin();
    let regulation = (2 * r.core.rounds_per_half) as usize;
    assert_eq!((m.winner(), m.score()), (Some(TEAM_A), [8, 6]));
    assert!(
        (m.tick() as i32) < r.core.max_ticks,
        "decided by play, not by the tick cap"
    );

    let rounds = &m.world.rounds;
    assert_eq!(rounds.len(), regulation + 2);
    for round in &rounds[..regulation] {
        assert_eq!(
            (round.winner, round.reason),
            (round.attackers, ROUND_WIN_DETONATION),
            "round {}",
            round.round
        );
    }
    let half = regulation / 2;
    assert!(
        rounds[..half].iter().all(|x| x.attackers == TEAM_A)
            && rounds[half..regulation]
                .iter()
                .all(|x| x.attackers == TEAM_B)
    );
    let (ot1, ot2) = (rounds[regulation], rounds[regulation + 1]);
    assert_eq!(
        (ot1.attackers, ot1.winner, ot1.reason),
        (TEAM_A, TEAM_A, ROUND_WIN_DETONATION)
    );
    assert_eq!(
        (ot2.attackers, ot2.winner, ot2.reason),
        (TEAM_B, TEAM_A, ROUND_WIN_TIMEOUT)
    );

    // Same seed, same match; and the recorded inputs alone reproduce every state hash.
    let (again, _, hashes_again) = play_scripted(11);
    assert_eq!(hashes, hashes_again);
    assert_eq!(m.summary_json(), again.summary_json());
    let mut replay = new_match(Ruleset::builtin(), 11);
    for (tick, inputs) in log.iter().enumerate() {
        for i in inputs {
            replay.apply_input(i);
        }
        replay.step();
        assert_eq!(
            replay.state_hash(),
            hashes[tick],
            "replay diverged at tick {tick}"
        );
    }
    assert!(replay.finished());
    assert_eq!(replay.winner(), Some(TEAM_A));
}
