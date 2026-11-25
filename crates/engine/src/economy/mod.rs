//! Economy and match-flow seam: credits, round/kill/objective income, the shop (`shop.rs`), loadout carry-over,
//! the half-time reset and overtime. The core calls the SEAM functions. Exact rules: docs/rules/economy.md.

mod shop;

use crate::world::{Unit, World};
use crate::Fnv;
use ibc_abi::*;
use toml::Table;

pub use shop::buy;

/// Units per team whose buy-phase purchases are tracked for trade-in refunds. Units beyond this (no shipped map
/// has that many) can still buy; they just get no refund when replacing something bought in the same phase.
pub const MAX_TRACKED: usize = 8;

/// What a unit paid, in the current buy phase, for what it now holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Paid {
    /// Per weapon slot (0 primary, 1 sidearm).
    pub weapon: [i32; 2],
    pub armor: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TeamEcon {
    /// Rounds lost in a row, going into the next payout.
    pub loss_streak: i32,
    /// Indexed by the unit's slot in its team. Cleared at every round start.
    pub paid: [Paid; MAX_TRACKED],
}

impl TeamEcon {
    pub fn hash(&self, h: &mut Fnv) {
        h.i32(self.loss_streak);
        for p in &self.paid {
            for v in [p.weapon[0], p.weapon[1], p.armor] {
                h.i32(v);
            }
        }
    }
}

/// Economy and match-flow numbers added after the foundation. They live in `rules/economy.toml` next to the
/// `EconRules` keys and are part of the ruleset hash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowRules {
    /// Paid to every defender when a round is won by defusing.
    pub defuse_reward: i32,
    /// Kill reward when the kill was not made with a weapon (ability damage).
    pub ability_kill_reward: i32,
    /// Share of the price returned when an item bought this buy phase is replaced in the same phase.
    pub refund_pct: i32,
    /// Everyone's credits at the start of every overtime round.
    pub overtime_credits: i32,
    /// Overtime pairs played before the match falls back to `tiebreak`. 0 disables overtime.
    pub overtime_max_pairs: i32,
}

impl FlowRules {
    pub fn from_table(t: &Table) -> FlowRules {
        let gi = |k: &str, d: i32| {
            t.get(k)
                .and_then(|v| v.as_integer())
                .map(|v| v as i32)
                .unwrap_or(d)
        };
        FlowRules {
            defuse_reward: gi("defuse_reward", 300),
            ability_kill_reward: gi("ability_kill_reward", 200),
            refund_pct: gi("refund_pct", 100).clamp(0, 100),
            overtime_credits: gi("overtime_credits", 5000),
            overtime_max_pairs: gi("overtime_max_pairs", 3).max(0),
        }
    }

    pub fn hash(&self, h: &mut Fnv) {
        for v in [
            self.defuse_reward,
            self.ability_kill_reward,
            self.refund_pct,
            self.overtime_credits,
            self.overtime_max_pairs,
        ] {
            h.i32(v);
        }
    }
}

fn give(w: &mut World, u: usize, amount: i32) {
    let max = w.rules.econ.max_credits;
    let c = &mut w.units[u].credits;
    *c = (*c + amount).min(max).max(0);
}

fn give_team(w: &mut World, team: i32, amount: i32) {
    for i in 0..w.units.len() {
        if w.units[i].team == team {
            give(w, i, amount);
        }
    }
}

/// Rounds in regulation (both halves).
fn regulation_rounds(w: &World) -> i32 {
    2 * w.rules.core.rounds_per_half.max(1)
}

/// Is the current round (`w.round`) an overtime round?
pub fn in_overtime(w: &World) -> bool {
    w.round > regulation_rounds(w)
}

/// SEAM
pub fn on_match_start(w: &mut World) {
    let start = w
        .rules
        .econ
        .start_credits
        .min(w.rules.econ.max_credits)
        .max(0);
    for u in w.units.iter_mut() {
        u.credits = start;
    }
}

/// SEAM: loadouts for the new round. `survived[i]` says whether unit i was alive when the last round ended.
///
/// The first round of each half and every overtime round start fresh: sidearm only, no armour, no kit, fixed
/// credits, loss streaks cleared. In every other round survivors keep weapons, remaining armour and their kit.
pub fn on_round_start(w: &mut World, survived: &[bool]) {
    let sidearm = w.rules.econ.default_sidearm;
    let max = w.rules.econ.max_credits;
    let second_half = w.round == w.rules.core.rounds_per_half.max(1) + 1;
    let overtime = in_overtime(w);
    let fresh = w.round <= 1 || second_half || overtime;
    let reset_credits = if overtime {
        Some(w.rules.flow.overtime_credits.min(max).max(0))
    } else if second_half {
        Some(w.rules.econ.start_credits.min(max).max(0))
    } else {
        None
    };
    for i in 0..w.units.len() {
        let keep = !fresh && survived.get(i).copied().unwrap_or(false);
        let u = &mut w.units[i];
        if let Some(c) = reset_credits {
            u.credits = c;
        }
        if !keep {
            u.weapons = [-1, sidearm];
            u.armor = 0;
            u.has_kit = false;
        }
        if u.weapons[1] < 0 {
            u.weapons[1] = sidearm;
        }
        u.slot = if u.weapons[0] >= 0 { 0 } else { 1 };
        for s in 0..2 {
            u.ammo[s] = if u.weapons[s] >= 0 {
                w.rules.weapons[u.weapons[s] as usize].magazine
            } else {
                0
            };
        }
    }
    for e in w.econ.iter_mut() {
        e.paid = Default::default();
        if fresh {
            e.loss_streak = 0;
        }
    }
}

/// SEAM: the killer is paid the weapon's `kill_reward`. Damage that does not come from a weapon must pass a
/// negative `weapon` to `World::damage`; such kills pay `ability_kill_reward`.
pub fn on_kill(w: &mut World, killer: usize, _victim: usize, weapon: i32) {
    let by_weapon = usize::try_from(weapon)
        .ok()
        .and_then(|i| w.rules.weapons.get(i))
        .map(|x| x.kill_reward);
    let reward = by_weapon.unwrap_or(w.rules.flow.ability_kill_reward);
    give(w, killer, reward);
}

/// SEAM: a plant pays every attacker, whoever goes on to win the round.
pub fn on_plant(w: &mut World, planter: usize) {
    let team = w.units[planter].team;
    let reward = w.rules.econ.plant_reward;
    give_team(w, team, reward);
}

/// SEAM: round income. Winners get `win_reward` (plus `defuse_reward` for a defuse); losers get `loss_reward`
/// plus `loss_streak_bonus` for each loss in a row before this one, up to `loss_streak_cap`.
pub fn on_round_end(w: &mut World, winner: i32, reason: i32) {
    if winner != TEAM_A && winner != TEAM_B {
        return;
    }
    let e = w.rules.econ;
    let loser = 1 - winner;
    let streak = w.econ[loser as usize]
        .loss_streak
        .clamp(0, e.loss_streak_cap.max(0));
    let defuse = if reason == ROUND_WIN_DEFUSE {
        w.rules.flow.defuse_reward
    } else {
        0
    };
    give_team(w, winner, e.win_reward + defuse);
    give_team(w, loser, e.loss_reward + e.loss_streak_bonus * streak);
    w.econ[winner as usize].loss_streak = 0;
    w.econ[loser as usize].loss_streak = streak + 1;
}

/// SEAM: called after every round from the last regulation round on, when the core has not ended the match.
/// Return `Some(team)` to end the match, or `None` to play another round.
///
/// Overtime is played in pairs of rounds, one per side. After a complete pair the team ahead wins (it took
/// both rounds); a split pair leads to another pair, and after `overtime_max_pairs` level pairs to `tiebreak`.
pub fn after_regulation(w: &mut World) -> Option<i32> {
    let played = w.round - regulation_rounds(w);
    if played < 0 || played % 2 != 0 {
        return None;
    }
    if w.score[0] != w.score[1] {
        return Some(if w.score[0] > w.score[1] {
            TEAM_A
        } else {
            TEAM_B
        });
    }
    if played / 2 >= w.rules.flow.overtime_max_pairs {
        return Some(tiebreak(w));
    }
    None
}

/// SEAM: pick a winner when the score cannot: rounds, then kills, then damage, then a seed coin flip.
pub fn tiebreak(w: &World) -> i32 {
    if w.score[0] != w.score[1] {
        return if w.score[0] > w.score[1] {
            TEAM_A
        } else {
            TEAM_B
        };
    }
    let sum = |team: i32, f: fn(&Unit) -> i32| -> i32 {
        w.units.iter().filter(|u| u.team == team).map(f).sum()
    };
    let kills: fn(&Unit) -> i32 = |u| u.stats.kills;
    let damage: fn(&Unit) -> i32 = |u| u.stats.damage;
    for f in [kills, damage] {
        let (a, b) = (sum(TEAM_A, f), sum(TEAM_B, f));
        if a != b {
            return if a > b { TEAM_A } else { TEAM_B };
        }
    }
    (w.seed & 1) as i32
}
