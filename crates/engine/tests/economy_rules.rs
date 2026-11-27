//! Income, credit cap, loss streak, shop validation, trade-in, carry-over and the half-time reset.
//! Rounds are decided by calling the core's `end_round` directly, so each test costs a handful of ticks.

use ibc_engine::abi::*;
use ibc_engine::{economy, Map, Match, Ruleset};

/// Two units per team: 0 and 1 are team A (attacking first), 2 and 3 team B.
const OPEN: &str = "name: open\nteam_size: 2\n---\n\
##############\n\
#a..........d#\n\
#a....#.....d#\n\
#.....#......#\n\
#..........11#\n\
#..........11#\n\
##############\n";

const RIFLE: i32 = 0;
const LIGHT: i32 = 1;
const HEAVY: i32 = 2;
const KIT: i32 = 3;
const PISTOL: i32 = 4;
const CHARGE_0: i32 = 5;

fn new_match(rules: Ruleset) -> Match {
    let mut m = Match::new(rules, Map::parse(OPEN).unwrap(), 1);
    to_phase(&mut m, PHASE_BUY);
    m
}

fn to_phase(m: &mut Match, phase: i32) {
    while m.world.phase != phase {
        assert!(!m.finished(), "match ended while waiting for phase {phase}");
        m.step();
    }
}

/// Decide the current round and step to the next buy phase (or the end of the match).
fn play_round(m: &mut Match, winner: i32, reason: i32) {
    to_phase(m, PHASE_ACTION);
    m.world.end_round(winner, reason);
    while m.world.phase == PHASE_ROUND_END {
        m.step();
    }
}

fn kill(m: &mut Match, killer: usize, victim: usize, weapon: i32) {
    m.world.damage(victim, 10_000, Some(killer), weapon);
    assert!(!m.world.units[victim].alive);
}

fn credits(m: &Match, unit: usize) -> i32 {
    m.world.units[unit].credits
}

fn buy(m: &mut Match, unit: u32, item: i32) -> i32 {
    m.act(unit, A_BUY, item, 0, 0)
}

/// The built-in ruleset plus a second primary and a second sidearm, each with its one-block shop item.
fn rules_with_extra_weapons() -> (Ruleset, i32, i32) {
    // The real ruleset has the smg; add a priced sidearm so sidearm trade-ins can be tested too.
    let base = Ruleset::builtin();
    let revolver_id = base.weapons.len();
    let weapons = format!(
        "{}
[[weapon]]
name = \"revolver\"
slot = 1
price = 600
",
        include_str!("../../../rules/weapons.toml")
    );
    let economy = format!(
        "{}
[[item]]
name = \"revolver\"
kind = 1
price = 600
value = {revolver_id}
",
        include_str!("../../../rules/economy.toml")
    );
    let r = Ruleset::from_toml(
        include_str!("../../../rules/core.toml"),
        &weapons,
        include_str!("../../../rules/abilities.toml"),
        &economy,
    )
    .unwrap();
    let smg = r.items.iter().find(|i| i.kind == ITEM_KIND_WEAPON && i.value == 2).unwrap().id;
    let n = r.items.len() as i32;
    (r, smg, n - 1)
}

#[test]
fn shop_lists_every_weapon_once_at_its_price() {
    let r = Ruleset::builtin();
    for w in &r.weapons {
        let items: Vec<_> = r
            .items
            .iter()
            .filter(|i| i.kind == ITEM_KIND_WEAPON && i.value == w.id)
            .collect();
        let name = &r.weapon_names[w.id as usize];
        assert_eq!(
            items.len(),
            1,
            "weapon `{name}` needs exactly one [[item]] in rules/economy.toml"
        );
        assert_eq!(
            items[0].price, w.price,
            "item price of `{name}` must equal its weapon price"
        );
    }
    // Items that the foundation shipped keep their ids.
    assert_eq!(
        &r.item_names[..4],
        ["rifle", "light_armor", "heavy_armor", "defuse_kit"]
    );
    for slot in 0..2 {
        assert!(r
            .items
            .iter()
            .any(|i| i.kind == ITEM_KIND_ABILITY_CHARGE && i.value == slot));
    }
    assert!(r.items.iter().filter(|i| i.kind == ITEM_KIND_ARMOR).count() >= 2);
    assert!(r.items.iter().any(|i| i.kind == ITEM_KIND_DEFUSE_KIT));
}

#[test]
fn pistol_round_cannot_afford_a_primary() {
    let r = Ruleset::builtin();
    let mut m = new_match(r.clone());
    assert!(m
        .world
        .units
        .iter()
        .all(|u| u.credits == r.econ.start_credits && u.weapons == [-1, r.econ.default_sidearm]));
    assert_eq!(buy(&mut m, 0, RIFLE), ERR_INSUFFICIENT_FUNDS);
    assert_eq!(credits(&m, 0), r.econ.start_credits);
    assert_eq!(buy(&mut m, 0, LIGHT), OK, "armour is the pistol-round buy");
    assert_eq!(
        credits(&m, 0),
        r.econ.start_credits - r.items[LIGHT as usize].price
    );
}

#[test]
fn buy_validation_errors() {
    let r = Ruleset::builtin();
    let mut m = new_match(r.clone());
    assert_eq!(buy(&mut m, 0, -1), ERR_BAD_ARG);
    assert_eq!(buy(&mut m, 0, r.items.len() as i32), ERR_BAD_ARG);
    assert_eq!(buy(&mut m, 0, RIFLE), ERR_INSUFFICIENT_FUNDS);
    assert_eq!(
        buy(&mut m, 0, PISTOL),
        ERR_NOT_ALLOWED,
        "already holds the default sidearm"
    );
    assert_eq!(
        buy(&mut m, 0, CHARGE_0),
        ERR_NOT_ALLOWED,
        "the standard kit has no ability in slot 0"
    );
    // Armour: owning as much or more is refused before the price is looked at.
    assert_eq!(buy(&mut m, 0, LIGHT), OK);
    assert_eq!(buy(&mut m, 0, LIGHT), ERR_NOT_ALLOWED);
    assert_eq!(
        buy(&mut m, 0, HEAVY),
        ERR_INSUFFICIENT_FUNDS,
        "800 credits do not cover heavy armour even with the trade-in"
    );
    assert_eq!((m.world.units[0].armor, credits(&m, 0)), (25, 400));
    // Defuse kit: defenders only, one each.
    assert_eq!(
        buy(&mut m, 1, KIT),
        ERR_NOT_ALLOWED,
        "attackers cannot buy a kit"
    );
    assert_eq!(buy(&mut m, 2, KIT), OK);
    assert_eq!(buy(&mut m, 2, KIT), ERR_NOT_ALLOWED);
    assert!(m.world.units[2].has_kit && !m.world.units[1].has_kit);
    // Nothing is sold outside the buy phase.
    to_phase(&mut m, PHASE_ACTION);
    assert_ne!(buy(&mut m, 3, LIGHT), OK);
    assert_eq!(credits(&m, 3), r.econ.start_credits);
}

#[test]
fn trade_in_refunds_purchases_of_the_same_buy_phase() {
    let (r, smg, revolver) = rules_with_extra_weapons();
    let mut m = new_match(r);
    m.world.units[0].credits = 5000;
    // Armour upgrade: light then heavy costs what heavy costs.
    assert_eq!(buy(&mut m, 0, LIGHT), OK);
    assert_eq!(buy(&mut m, 0, HEAVY), OK);
    assert_eq!((m.world.units[0].armor, credits(&m, 0)), (50, 4000));
    // Primary: rifle, change of mind to the smg, and back.
    assert_eq!(buy(&mut m, 0, RIFLE), OK);
    assert_eq!(credits(&m, 0), 1300);
    assert_eq!(buy(&mut m, 0, smg), OK);
    assert_eq!((m.world.units[0].weapons[0], credits(&m, 0)), (2, 2500));
    assert_eq!(buy(&mut m, 0, smg), ERR_NOT_ALLOWED);
    assert_eq!(buy(&mut m, 0, RIFLE), OK);
    assert_eq!((m.world.units[0].weapons[0], credits(&m, 0)), (1, 1300));
    // Sidearm: buying the free default back returns the upgrade's price.
    assert_eq!(buy(&mut m, 0, revolver), OK);
    let revolver_weapon = m.world.rules.items[revolver as usize].value;
    assert_eq!((m.world.units[0].weapons[1], credits(&m, 0)), (revolver_weapon, 700));
    assert_eq!(buy(&mut m, 0, PISTOL), OK);
    assert_eq!((m.world.units[0].weapons[1], credits(&m, 0)), (0, 1300));
    // The trade-in can make an otherwise unaffordable swap affordable, and never mints credits.
    m.world.units[1].credits = 2700;
    assert_eq!(buy(&mut m, 1, RIFLE), OK);
    assert_eq!(buy(&mut m, 1, smg), OK);
    assert_eq!(credits(&m, 1), 1200);

    // Gear carried over from an earlier round is not refundable.
    play_round(&mut m, TEAM_A, ROUND_WIN_ELIMINATION);
    assert_eq!(m.world.units[0].weapons[0], 1, "survivor kept the rifle");
    let before = credits(&m, 0);
    assert_eq!(buy(&mut m, 0, smg), OK);
    assert_eq!(credits(&m, 0), before - 1500);
}

#[test]
fn round_income_and_loss_streak() {
    let r = Ruleset::builtin();
    let e = r.econ;
    let mut m = new_match(r.clone());
    for u in m.world.units.iter_mut() {
        u.credits = 0;
    }
    // Team A loses rounds 1-5: the loss payout climbs to the cap and stays there.
    let mut expect_a = 0;
    for streak in 0..5 {
        play_round(&mut m, TEAM_B, ROUND_WIN_TIMEOUT);
        let pay = e.loss_reward + e.loss_streak_bonus * streak.min(e.loss_streak_cap);
        expect_a = (expect_a + pay).min(e.max_credits);
        assert_eq!(
            credits(&m, 0),
            expect_a,
            "after {} losses in a row",
            streak + 1
        );
        assert_eq!(credits(&m, 1), expect_a);
    }
    assert_eq!(
        e.loss_reward + e.loss_streak_bonus * e.loss_streak_cap,
        2900
    );
    assert_eq!(
        m.world.econ[TEAM_A as usize].loss_streak,
        e.loss_streak_cap + 1
    );
    assert_eq!(
        credits(&m, 2),
        e.max_credits,
        "five wins hit the credit cap"
    );
    // A win pays win_reward and clears the streak.
    m.world.units[0].credits = 0;
    m.world.units[2].credits = 0;
    play_round(&mut m, TEAM_A, ROUND_WIN_ELIMINATION);
    assert_eq!(
        credits(&m, 0),
        e.start_credits,
        "round 7 is the half-time reset"
    );
    assert_eq!(m.world.econ[TEAM_A as usize].loss_streak, 0);
    m.world.units[0].credits = 0;
    m.world.units[2].credits = 0;
    play_round(&mut m, TEAM_A, ROUND_WIN_ELIMINATION);
    assert_eq!(
        (credits(&m, 0), credits(&m, 2)),
        (e.win_reward, e.loss_reward)
    );
    play_round(&mut m, TEAM_B, ROUND_WIN_ELIMINATION);
    assert_eq!(
        (credits(&m, 0), credits(&m, 2)),
        (e.win_reward + e.loss_reward, e.loss_reward + e.win_reward)
    );
    play_round(&mut m, TEAM_B, ROUND_WIN_ELIMINATION);
    assert_eq!(
        credits(&m, 0),
        e.win_reward + 2 * e.loss_reward + e.loss_streak_bonus
    );
}

#[test]
fn kill_plant_and_defuse_rewards() {
    let r = Ruleset::builtin();
    let (e, f) = (r.econ, r.flow);
    let mut m = new_match(r.clone());
    to_phase(&mut m, PHASE_ACTION);
    let start = e.start_credits;
    // Unit 0 (attacker) kills unit 2 with the pistol; unit 3 kills unit 1 with an ability (weapon -1).
    kill(&mut m, 0, 2, 0);
    assert_eq!(credits(&m, 0), start + r.weapons[0].kill_reward);
    kill(&mut m, 3, 1, -1);
    assert_eq!(credits(&m, 3), start + f.ability_kill_reward);
    // A plant pays both attackers, dead or alive, and nobody else.
    economy::on_plant(&mut m.world, 0);
    assert_eq!(
        credits(&m, 0),
        start + r.weapons[0].kill_reward + e.plant_reward
    );
    assert_eq!(credits(&m, 1), start + e.plant_reward);
    assert_eq!(credits(&m, 2), start);
    // The defenders defuse: win reward plus the defuse reward each; the attackers get the loss reward.
    play_round(&mut m, TEAM_B, ROUND_WIN_DEFUSE);
    assert_eq!(credits(&m, 2), start + e.win_reward + f.defuse_reward);
    assert_eq!(
        credits(&m, 3),
        start + f.ability_kill_reward + e.win_reward + f.defuse_reward
    );
    assert_eq!(credits(&m, 1), start + e.plant_reward + e.loss_reward);
}

#[test]
fn credits_never_exceed_the_cap() {
    let r = Ruleset::builtin();
    let max = r.econ.max_credits;
    let mut m = new_match(r);
    m.world.units[0].credits = max - 100;
    m.world.units[2].credits = max - 100;
    to_phase(&mut m, PHASE_ACTION);
    kill(&mut m, 0, 2, 0);
    assert_eq!(credits(&m, 0), max);
    economy::on_plant(&mut m.world, 0);
    play_round(&mut m, TEAM_A, ROUND_WIN_DETONATION);
    assert_eq!(credits(&m, 0), max);
    assert_eq!(credits(&m, 2), max);
}

#[test]
fn survivors_carry_their_loadout_and_the_dead_start_over() {
    let r = Ruleset::builtin();
    let mut m = new_match(r.clone());
    for u in [0, 1, 2] {
        m.world.units[u].credits = 5000;
    }
    for u in [0, 1] {
        assert_eq!(buy(&mut m, u, RIFLE), OK);
        assert_eq!(buy(&mut m, u, HEAVY), OK);
    }
    assert_eq!(buy(&mut m, 2, KIT), OK);
    assert_eq!(buy(&mut m, 2, LIGHT), OK);
    to_phase(&mut m, PHASE_ACTION);
    m.world.damage(0, 30, None, -1); // armour soaks half
    m.world.units[0].ammo[0] = 1;
    kill(&mut m, 2, 1, 0);
    play_round(&mut m, TEAM_A, ROUND_WIN_ELIMINATION);

    let rifle = r.weapons[1];
    let u0 = &m.world.units[0];
    assert_eq!(
        (u0.weapons, u0.slot, u0.armor),
        ([1, 0], 0, 35),
        "survivor keeps weapons and what is left of the armour"
    );
    assert_eq!(
        (u0.ammo[0], u0.hp),
        (rifle.magazine, 100),
        "magazines and health are refilled"
    );
    let u1 = &m.world.units[1];
    assert_eq!(
        (u1.weapons, u1.slot, u1.armor),
        ([-1, 0], 1, 0),
        "the dead are back to the sidearm"
    );
    let u2 = &m.world.units[2];
    assert!(
        u2.has_kit && u2.armor == 25,
        "a surviving defender keeps kit and armour, win or lose"
    );
    assert_eq!(buy(&mut m, 2, KIT), ERR_NOT_ALLOWED);
    assert_eq!(buy(&mut m, 0, RIFLE), ERR_NOT_ALLOWED);
    assert_eq!(
        buy(&mut m, 0, HEAVY),
        OK,
        "damaged armour can be topped up at full price"
    );
    assert_eq!(m.world.units[0].armor, 50);
}

#[test]
fn half_time_resets_credits_loadouts_and_streaks() {
    let r = Ruleset::builtin();
    let half = r.core.rounds_per_half;
    let mut m = new_match(r.clone());
    for _ in 0..half - 1 {
        play_round(&mut m, TEAM_B, ROUND_WIN_TIMEOUT);
    }
    assert_eq!(m.world.round, half);
    assert_eq!(m.world.side_of(TEAM_A), SIDE_ATTACK);
    assert_eq!(buy(&mut m, 2, RIFLE), OK);
    assert_eq!(buy(&mut m, 2, HEAVY), OK);
    assert_eq!(buy(&mut m, 2, KIT), OK);
    assert!(m.world.econ[TEAM_A as usize].loss_streak > 0);
    play_round(&mut m, TEAM_B, ROUND_WIN_TIMEOUT);

    assert_eq!(m.world.round, half + 1);
    assert_eq!(m.world.side_of(TEAM_A), SIDE_DEFEND);
    for u in &m.world.units {
        assert_eq!(u.credits, r.econ.start_credits);
        assert_eq!(
            (u.weapons, u.armor, u.has_kit),
            ([-1, r.econ.default_sidearm], 0, false),
            "unit {}",
            u.id
        );
    }
    assert_eq!(m.world.econ[TEAM_A as usize].loss_streak, 0);
    // The first loss of the new half pays the base loss reward again.
    play_round(&mut m, TEAM_B, ROUND_WIN_ELIMINATION);
    assert_eq!(credits(&m, 0), r.econ.start_credits + r.econ.loss_reward);
}

#[test]
fn economy_state_is_hashed() {
    let mut m = new_match(Ruleset::builtin());
    let base = m.state_hash();
    m.world.econ[0].loss_streak += 1;
    let streak = m.state_hash();
    assert_ne!(base, streak);
    m.world.econ[0].loss_streak -= 1;
    m.world.econ[1].paid[1].armor = 400;
    assert_ne!(base, m.state_hash());
    m.world.econ[1].paid[1].armor = 0;
    m.world.econ[1].paid[1].weapon[1] = 1;
    assert_ne!(base, m.state_hash());
    m.world.econ[1].paid[1].weapon[1] = 0;
    assert_eq!(base, m.state_hash());

    // The new numbers are part of the ruleset hash.
    let mut r = Ruleset::builtin();
    let h = r.hash();
    r.flow.overtime_max_pairs += 1;
    assert_ne!(h, r.hash());
}

/// The worked example of docs/rules/economy.md, pinned. One unit per team is followed; the winner's followed
/// unit kills the loser's each round. If a tuning change breaks this, update the table in the doc with it.
#[test]
fn worked_example_matches_the_docs() {
    // (winner, reason) per round. Team A attacks rounds 1-6.
    let script = [
        (TEAM_A, ROUND_WIN_DETONATION),
        (TEAM_A, ROUND_WIN_ELIMINATION),
        (TEAM_B, ROUND_WIN_ELIMINATION),
        (TEAM_B, ROUND_WIN_DEFUSE),
        (TEAM_B, ROUND_WIN_TIMEOUT),
        (TEAM_A, ROUND_WIN_DETONATION),
        (TEAM_B, ROUND_WIN_DETONATION),
        (TEAM_A, ROUND_WIN_ELIMINATION),
        (TEAM_A, ROUND_WIN_DEFUSE),
        (TEAM_B, ROUND_WIN_ELIMINATION),
        (TEAM_A, ROUND_WIN_ELIMINATION),
        (TEAM_A, ROUND_WIN_TIMEOUT),
    ];
    // (credits at buy start, credits after buying) for unit 0 (team A) and unit 2 (team B).
    let expected: [[i32; 4]; 12] = [
        [800, 400, 800, 400],
        [4000, 300, 1800, 1800],
        [3500, 3500, 3700, 0],
        [4900, 1200, 3200, 3200],
        [3400, 300, 6700, 6700],
        [2700, 2700, 8000, 8000],
        [800, 400, 800, 400],
        [1800, 1800, 4000, 300],
        [5100, 1400, 1700, 1700],
        [4900, 4900, 3900, 200],
        [6300, 2600, 3400, 3400],
        [5800, 5800, 4800, 1100],
    ];
    let mut m = new_match(Ruleset::builtin());
    let mut table = Vec::new();
    for (round, (winner, reason)) in script.iter().copied().enumerate() {
        assert_eq!(m.world.round, round as i32 + 1);
        let mut row = [0; 4];
        for (k, unit) in [0usize, 2].into_iter().enumerate() {
            row[2 * k] = credits(&m, unit);
            example_buy(&mut m, unit);
            row[2 * k + 1] = credits(&m, unit);
        }
        table.push(row);
        to_phase(&mut m, PHASE_ACTION);
        let attackers = m.world.team_with_side(SIDE_ATTACK);
        if reason == ROUND_WIN_DETONATION || reason == ROUND_WIN_DEFUSE {
            economy::on_plant(&mut m.world, (attackers * 2) as usize);
        }
        let (killer, victim) = if winner == TEAM_A { (0, 2) } else { (2, 0) };
        let weapon = m.world.units[killer].weapon();
        kill(&mut m, killer, victim, weapon);
        // The doc's detonation rounds assume the followed attacker ran clear of the blast. `end_round` is
        // called directly, so no blast happens here; the winner must still be alive to carry its gear.
        assert!(m.world.units[killer].alive);
        play_round(&mut m, winner, reason);
    }
    assert_eq!(
        table, expected,
        "update docs/rules/economy.md together with this table"
    );
    assert_eq!((m.winner(), m.score()), (Some(TEAM_A), [7, 5]));
    assert_eq!((credits(&m, 0), credits(&m, 2)), (8000, 3000));
}

/// The followed units' policy: full buy (rifle + heavy armour) when it is affordable, light armour in a pistol
/// round, a force buy (rifle + light armour) from 3100 credits, otherwise save. Survivors top armour up.
fn example_buy(m: &mut Match, unit: usize) {
    let u = unit as u32;
    let (c, has_rifle, armor) = (
        credits(m, unit),
        m.world.units[unit].weapons[0] == 1,
        m.world.units[unit].armor,
    );
    let half = m.world.rules.core.rounds_per_half;
    if has_rifle {
        if armor < 50 {
            buy(m, u, HEAVY);
        }
    } else if c >= 3700 {
        assert_eq!(buy(m, u, RIFLE), OK);
        assert_eq!(buy(m, u, HEAVY), OK);
    } else if c >= 3100 {
        assert_eq!(buy(m, u, RIFLE), OK);
        assert_eq!(buy(m, u, LIGHT), OK);
    } else if m.world.round % half == 1 {
        assert_eq!(buy(m, u, LIGHT), OK);
    }
}
