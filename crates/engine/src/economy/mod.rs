//! Economy and match-flow seam. Foundation: start credits, round and kill income, a simple shop, default
//! sidearm each round, and a tie-break instead of overtime. The core calls the SEAM functions.

use crate::world::{Unit, World};
use crate::Fnv;
use ibc_abi::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct TeamEcon {
    pub loss_streak: i32,
}

impl TeamEcon {
    pub fn hash(&self, h: &mut Fnv) {
        h.i32(self.loss_streak);
    }
}

fn give(w: &mut World, u: usize, amount: i32) {
    let max = w.rules.econ.max_credits;
    let c = &mut w.units[u].credits;
    *c = (*c + amount).clamp(0, max);
}

/// SEAM
pub fn on_match_start(w: &mut World) {
    let start = w.rules.econ.start_credits;
    for u in w.units.iter_mut() {
        u.credits = start;
    }
}

/// SEAM: loadouts for the new round. `survived[i]` says whether unit i was alive when the last round ended.
pub fn on_round_start(w: &mut World, survived: &[bool]) {
    let sidearm = w.rules.econ.default_sidearm;
    let half = w.rules.core.rounds_per_half;
    let swap_round = w.round == half + 1;
    for i in 0..w.units.len() {
        let keep = survived.get(i).copied().unwrap_or(false) && !swap_round && w.round > 1;
        if swap_round {
            w.units[i].credits = w.rules.econ.start_credits;
        }
        let u = &mut w.units[i];
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
            u.ammo[s] = if u.weapons[s] >= 0 { w.rules.weapons[u.weapons[s] as usize].magazine } else { 0 };
        }
    }
    if swap_round {
        w.econ = Default::default();
    }
}

/// SEAM: A_BUY in the buy phase. Applied immediately.
pub fn buy(w: &mut World, u: usize, item: i32) -> i32 {
    if item < 0 {
        return ERR_BAD_ARG;
    }
    let it = match w.rules.items.get(item as usize) {
        Some(it) => *it,
        None => return ERR_BAD_ARG,
    };
    if w.units[u].credits < it.price {
        return ERR_INSUFFICIENT_FUNDS;
    }
    match it.kind {
        ITEM_KIND_WEAPON => {
            let wr = match w.rules.weapons.get(it.value.max(0) as usize) {
                Some(wr) => *wr,
                None => return ERR_BAD_ARG,
            };
            let unit = &mut w.units[u];
            let s = wr.slot as usize;
            if unit.weapons[s] == wr.id {
                return ERR_NOT_ALLOWED;
            }
            unit.weapons[s] = wr.id;
            unit.ammo[s] = wr.magazine;
            unit.slot = if unit.weapons[0] >= 0 { 0 } else { 1 };
        }
        ITEM_KIND_ARMOR => {
            if w.units[u].armor >= it.value {
                return ERR_NOT_ALLOWED;
            }
            w.units[u].armor = it.value;
        }
        ITEM_KIND_DEFUSE_KIT => {
            if w.units[u].has_kit || w.side_of(w.units[u].team) != SIDE_DEFEND {
                return ERR_NOT_ALLOWED;
            }
            w.units[u].has_kit = true;
        }
        ITEM_KIND_ABILITY_CHARGE => {
            let r = crate::abilities::add_charge(w, u, it.value);
            if r != OK {
                return r;
            }
        }
        _ => return ERR_UNSUPPORTED,
    }
    w.units[u].credits -= it.price;
    OK
}

/// SEAM
pub fn on_kill(w: &mut World, killer: usize, _victim: usize, weapon: i32) {
    let reward = w.rules.weapons.get(weapon.max(0) as usize).map(|x| x.kill_reward).unwrap_or(200);
    give(w, killer, reward);
}

/// SEAM
pub fn on_plant(w: &mut World, planter: usize) {
    let team = w.units[planter].team;
    let reward = w.rules.econ.plant_reward;
    for i in 0..w.units.len() {
        if w.units[i].team == team {
            give(w, i, reward);
        }
    }
}

/// SEAM
pub fn on_round_end(w: &mut World, winner: i32, _reason: i32) {
    let e = w.rules.econ;
    let loser = 1 - winner;
    let streak = w.econ[loser as usize].loss_streak.min(e.loss_streak_cap);
    for i in 0..w.units.len() {
        let amount = if w.units[i].team == winner { e.win_reward } else { e.loss_reward + e.loss_streak_bonus * streak };
        give(w, i, amount);
    }
    w.econ[winner as usize].loss_streak = 0;
    w.econ[loser as usize].loss_streak += 1;
}

/// SEAM: regulation rounds are over and nobody has won. Return `Some(team)` to end the match, or `None` to
/// play another round (overtime).
pub fn after_regulation(w: &mut World) -> Option<i32> {
    Some(tiebreak(w))
}

/// SEAM: pick a winner when the score cannot: rounds, then kills, then damage, then a seed coin flip.
pub fn tiebreak(w: &World) -> i32 {
    if w.score[0] != w.score[1] {
        return if w.score[0] > w.score[1] { TEAM_A } else { TEAM_B };
    }
    let sum = |team: i32, f: fn(&Unit) -> i32| -> i32 { w.units.iter().filter(|u| u.team == team).map(f).sum() };
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
