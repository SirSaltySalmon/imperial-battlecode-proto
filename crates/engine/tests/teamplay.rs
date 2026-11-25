//! Team play through the `Host` calls bots use: drafting and buying for a teammate, unique kits, the memory
//! scopes and their phases, the roster and round tables, and the extra `UnitInfo` fields.

use ibc_engine::abi::*;
use ibc_engine::{Map, Match, Ruleset};

/// 16x7, three per team.
const ARENA: &str = "name: arena\nteam_size: 3\n---\n\
################\n\
#a............d#\n\
#a............d#\n\
#a............d#\n\
#..............#\n\
#...........11.#\n\
################\n";

fn new_match() -> Match {
    Match::new(Ruleset::builtin(), Map::parse(ARENA).unwrap(), 7)
}

fn to_phase(m: &mut Match, phase: i32) {
    while m.world.phase != phase {
        m.step();
    }
}

fn row<T: Copy + Default>(m: &Match, unit: u32, table: u32, index: u32) -> T {
    let mut buf = Vec::new();
    assert!(m.get(unit, table, index, &mut buf) >= 0, "table {table} row {index}");
    from_bytes(&buf).unwrap()
}

fn item(name: &str) -> i32 {
    Ruleset::builtin().item_names.iter().position(|n| n == name).expect("item exists") as i32
}

fn weapon(name: &str) -> i32 {
    Ruleset::builtin().weapon_names.iter().position(|n| n == name).expect("weapon exists") as i32
}

#[test]
fn draft_for_sets_a_teammates_kit() {
    let mut m = new_match();
    assert_eq!(m.act(0, A_DRAFT_FOR, 2, 1, 0), OK);
    assert_eq!(m.act(0, A_DRAFT_FOR, 3, 2, 0), OK);
    assert_eq!(m.act(0, A_DRAFT_FOR, 1, 0, 0), OK, "your own slot works too");
    assert_eq!([m.world.units[0].kit, m.world.units[1].kit, m.world.units[2].kit], [1, 2, 3]);
    // Slots are within the caller's team: slot 0 of team B is unit 3.
    assert_eq!(m.act(4, A_DRAFT_FOR, 2, 0, 0), OK);
    assert_eq!(m.world.units[3].kit, 2);
    for slot in [-1, 3, 100] {
        assert_eq!(m.act(0, A_DRAFT_FOR, 4, slot, 0), ERR_BAD_ARG, "slot {slot}");
    }
    assert_eq!(m.act(0, A_DRAFT_FOR, 99, 1, 0), ERR_BAD_ARG, "no such kit");
    to_phase(&mut m, PHASE_BUY);
    assert_eq!(m.act(0, A_DRAFT_FOR, 4, 1, 0), ERR_NOT_ALLOWED, "the draft is one tick");
}

#[test]
fn kits_are_unique_within_a_team() {
    let mut m = new_match();
    assert_eq!(m.act(0, A_DRAFT, 1, 0, 0), OK);
    assert_eq!(m.act(1, A_DRAFT, 1, 0, 0), ERR_NOT_ALLOWED, "a teammate holds kit 1");
    assert_eq!(m.act(2, A_DRAFT_FOR, 1, 1, 0), ERR_NOT_ALLOWED, "drafting for a teammate is no different");
    assert_eq!(m.world.units[1].kit, 0, "a refused draft changes nothing");
    assert_eq!(m.act(0, A_DRAFT, 1, 0, 0), OK, "re-drafting the kit you hold is fine");
    assert_eq!(m.act(3, A_DRAFT, 1, 0, 0), OK, "the other team's picks do not block");
    // Kit 0 is what undrafted units play, so it can be shared.
    assert_eq!(m.act(1, A_DRAFT, 0, 0, 0), OK);
    // Changing your mind frees the kit you held.
    assert_eq!(m.act(0, A_DRAFT, 2, 0, 0), OK);
    assert_eq!(m.act(1, A_DRAFT, 1, 0, 0), OK);
}

#[test]
fn buy_for_charges_the_buyer_and_equips_the_teammate() {
    let mut m = new_match();
    to_phase(&mut m, PHASE_BUY);
    m.world.units[0].credits = 5000;
    m.world.units[1].credits = 100;
    assert_eq!(m.act(0, A_BUY_FOR, item("rifle"), 1, 0), OK);
    assert_eq!(m.world.units[1].weapons[0], weapon("rifle"));
    assert_eq!(m.world.units[0].weapons[0], -1, "the buyer gets nothing");
    assert_eq!((m.world.units[0].credits, m.world.units[1].credits), (2300, 100));
    assert_eq!(m.act(0, A_BUY_FOR, item("rifle"), 1, 0), ERR_NOT_ALLOWED, "it already holds one");
    assert_eq!(m.act(0, A_BUY_FOR, item("marksman"), 1, 0), OK, "the trade-in goes to the buyer");
    assert_eq!(m.world.units[0].credits, 2300 + 2700 - 3200);
    assert_eq!(m.act(1, A_BUY_FOR, item("heavy_armor"), 2, 0), ERR_INSUFFICIENT_FUNDS, "the buyer's credits count");
    assert_eq!(m.act(0, A_BUY_FOR, item("rifle"), 7, 0), ERR_BAD_ARG);
}

#[test]
fn trade_in_moves_credits_between_teammates() {
    let mut m = new_match();
    to_phase(&mut m, PHASE_BUY);
    m.world.units[0].credits = 5000;
    m.world.units[1].credits = 0;
    assert_eq!(m.act(0, A_BUY_FOR, item("rifle"), 1, 0), OK);
    // B swaps the rifle A paid for for an SMG and keeps the difference.
    assert_eq!(m.act(1, A_BUY, item("smg"), 0, 0), OK);
    assert_eq!((m.world.units[0].credits, m.world.units[1].credits), (2300, 1200));
}

#[test]
fn buy_for_is_buy_phase_only() {
    let mut m = new_match();
    assert_eq!(m.act(0, A_BUY_FOR, item("light_armor"), 1, 0), ERR_NOT_ALLOWED);
    to_phase(&mut m, PHASE_ACTION);
    assert_eq!(m.act(0, A_BUY_FOR, item("light_armor"), 1, 0), ERR_UNSUPPORTED);
}

fn write(m: &mut Match, unit: u32, scope: u32, data: &[u8]) -> i32 {
    m.mem_write(unit, scope, 0, data)
}

fn read(m: &Match, unit: u32, scope: u32) -> [u8; 4] {
    let mut out = [0u8; 4];
    assert_eq!(m.mem_read(unit, scope, 0, &mut out), OK);
    out
}

#[test]
fn memory_scopes_are_written_in_their_own_phase() {
    let mut m = new_match();
    assert_eq!(m.world.buy_mem[0].len(), m.world.rules.core.buy_mem_bytes as usize);
    assert!(m.world.rules.core.buy_mem_bytes < m.world.rules.core.draft_mem_bytes);
    // Draft: draft memory only.
    assert_eq!(write(&mut m, 0, MEM_DRAFT, &[1, 2, 3, 4]), OK);
    assert_eq!(write(&mut m, 0, MEM_TEAM, &[1]), ERR_NOT_ALLOWED);
    assert_eq!(write(&mut m, 0, MEM_BUY, &[1]), ERR_NOT_ALLOWED);
    to_phase(&mut m, PHASE_BUY);
    // Buy: buy memory only.
    assert_eq!(write(&mut m, 0, MEM_TEAM, &[1]), ERR_NOT_ALLOWED);
    assert_eq!(write(&mut m, 0, MEM_DRAFT, &[1]), ERR_NOT_ALLOWED);
    assert_eq!(write(&mut m, 1, MEM_BUY, &[9, 8, 7, 6]), OK);
    assert_eq!(read(&m, 2, MEM_BUY), [9, 8, 7, 6], "shared by the team");
    assert_eq!(read(&m, 3, MEM_BUY), [0; 4], "not by the other team");
    assert_eq!(m.mem_write(1, MEM_BUY, 254, &[0; 4]), ERR_OUT_OF_RANGE);
    to_phase(&mut m, PHASE_ACTION);
    // Action: team memory only; buy memory is still readable.
    assert_eq!(write(&mut m, 0, MEM_BUY, &[1]), ERR_NOT_ALLOWED);
    assert_eq!(write(&mut m, 0, MEM_DRAFT, &[1]), ERR_NOT_ALLOWED);
    assert_eq!(write(&mut m, 0, MEM_TEAM, &[5, 5, 5, 5]), OK);
    assert_eq!(read(&m, 1, MEM_BUY), [9, 8, 7, 6]);
    assert_eq!(read(&m, 1, MEM_DRAFT), [1, 2, 3, 4]);
    assert_eq!(write(&mut m, 0, 3, &[1]), ERR_BAD_ARG, "no such scope");
    // The next round clears buy memory and keeps the rest.
    m.world.end_round(TEAM_A, ROUND_WIN_TIMEOUT);
    to_phase(&mut m, PHASE_BUY);
    assert_eq!(m.world.round, 2);
    assert_eq!(read(&m, 1, MEM_BUY), [0; 4]);
    assert_eq!(read(&m, 1, MEM_TEAM), [5; 4]);
}

#[test]
fn roster_lists_both_teams_with_round_start_credits() {
    let mut m = new_match();
    assert_eq!(m.act(3, A_DRAFT, 4, 0, 0), OK);
    assert_eq!(m.count(0, T_ROSTER), 6);
    let r: RosterInfo = row(&m, 0, T_ROSTER, 3);
    assert_eq!((r.id, r.team, r.kit, r.alive), (3, TEAM_B, 4, 0), "readable in the draft");
    to_phase(&mut m, PHASE_BUY);
    let start = m.world.units[3].credits;
    assert_eq!(m.act(3, A_BUY, item("light_armor"), 0, 0), OK);
    let r: RosterInfo = row(&m, 0, T_ROSTER, 3);
    assert_eq!((r.credits, r.alive), (start, 1), "credits as the buy phase began, not after spending");
    to_phase(&mut m, PHASE_ACTION);
    m.world.units[4].alive = false;
    let alive: Vec<i32> = (0..6).map(|i| row::<RosterInfo>(&m, 0, T_ROSTER, i).alive).collect();
    assert_eq!(alive, [1, 1, 1, 1, 0, 1]);
    assert_eq!(m.get(0, T_ROSTER, 6, &mut Vec::new()), -ERR_OUT_OF_RANGE);
}

#[test]
fn rounds_table_logs_finished_rounds() {
    let mut m = new_match();
    to_phase(&mut m, PHASE_ACTION);
    assert_eq!(m.count(0, T_ROUNDS), 0);
    m.world.end_round(TEAM_B, ROUND_WIN_ELIMINATION);
    to_phase(&mut m, PHASE_ACTION);
    m.world.end_round(TEAM_B, ROUND_WIN_TIMEOUT);
    assert_eq!(m.count(3, T_ROUNDS), 2);
    let r: RoundInfo = row(&m, 3, T_ROUNDS, 0);
    assert_eq!((r.round, r.winner, r.reason, r.attackers), (1, TEAM_B, ROUND_WIN_ELIMINATION, TEAM_A));
    assert_eq!((r.score_a, r.score_b), (0, 1));
    assert!(r.ticks >= m.world.rules.core.buy_ticks);
    let r: RoundInfo = row(&m, 0, T_ROUNDS, 1);
    assert_eq!((r.round, r.reason, r.score_a, r.score_b), (2, ROUND_WIN_TIMEOUT, 0, 2));
    assert_eq!(m.get(0, T_ROUNDS, 2, &mut Vec::new()), -ERR_OUT_OF_RANGE);
}

#[test]
fn unit_info_shows_the_second_weapon_and_cooldown_to_the_team_only() {
    let mut m = new_match();
    to_phase(&mut m, PHASE_BUY);
    m.world.units[0].credits = 5000;
    assert_eq!(m.act(0, A_BUY, item("rifle"), 0, 0), OK);
    to_phase(&mut m, PHASE_ACTION);
    m.world.units[0].combat.cooldown = 3;
    let me: UnitInfo = row(&m, 0, T_SELF, 0);
    let sidearm = m.world.rules.econ.default_sidearm;
    assert_eq!((me.weapon, me.other_weapon, me.fire_cooldown), (weapon("rifle"), sidearm, 3));
    let ally: UnitInfo = row(&m, 1, T_ALLIES, 0);
    assert_eq!((ally.id, ally.other_weapon, ally.fire_cooldown), (0, sidearm, 3));
    let bare: UnitInfo = row(&m, 1, T_SELF, 0);
    assert_eq!((bare.weapon, bare.other_weapon), (sidearm, -1), "the primary slot is empty");
    // Put unit 0 right next to enemy 3 so it is sensed, and read it from the enemy's side.
    let p = m.world.units[3].pos;
    m.world.units[0].pos = (p.0 - TILE, p.1);
    m.step();
    let seen = (0..m.count(3, T_ENEMIES) as u32).map(|i| row::<UnitInfo>(&m, 3, T_ENEMIES, i)).find(|e| e.id == 0);
    let seen = seen.expect("unit 3 senses unit 0");
    assert_eq!((seen.weapon, seen.other_weapon, seen.fire_cooldown), (weapon("rifle"), 0, 0));
}

#[test]
fn new_state_is_hashed() {
    let base = {
        let mut m = new_match();
        to_phase(&mut m, PHASE_BUY);
        m
    };
    let h = base.state_hash();
    let mut m = new_match();
    to_phase(&mut m, PHASE_BUY);
    assert_eq!(m.state_hash(), h);
    assert_eq!(write(&mut m, 0, MEM_BUY, &[1]), OK);
    assert_ne!(m.state_hash(), h, "buy memory");
    let mut m = new_match();
    to_phase(&mut m, PHASE_BUY);
    m.world.round_credits[0] += 1;
    assert_ne!(m.state_hash(), h, "round-start credits");
    let mut m = new_match();
    to_phase(&mut m, PHASE_BUY);
    m.world.rounds.push(ibc_engine::world::RoundLog { round: 0, winner: 0, reason: 0, attackers: 0, ticks: 0 });
    assert_ne!(m.state_hash(), h, "round log");
}
