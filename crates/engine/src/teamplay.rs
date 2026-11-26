//! Team play: what a team knows about both rosters and the rounds played so far (`T_ROSTER`, `T_ROUNDS`),
//! buy-phase team memory (`MEM_BUY`), and acting for a teammate (`A_BUY_FOR`, `A_DRAFT_FOR`).
//!
//! State kept here lives on `World` (`buy_mem`, `round_credits`) and is hashed by `hash`.

use crate::world::World;
use crate::Fnv;
use ibc_abi::*;

/// Round start: clear the buy memory and snapshot every unit's credits as the buy phase opens.
pub fn on_round_start(w: &mut World) {
    for m in w.buy_mem.iter_mut() {
        m.fill(0);
    }
    w.round_credits = w.units.iter().map(|u| u.credits).collect();
}

pub fn hash(w: &World, h: &mut Fnv) {
    // `buy_mem` is team memory: it is in `World::memory_hash`, not here.
    for &c in &w.round_credits {
        h.i32(c);
    }
    for r in &w.rounds {
        for v in [r.round, r.winner, r.reason, r.attackers, r.ticks] {
            h.i32(v);
        }
    }
}

/// The unit in slot `slot` of `me`'s team, or `ERR_BAD_ARG`.
pub fn teammate(w: &World, me: usize, slot: i32) -> Result<usize, i32> {
    if !(0..w.team_size).contains(&slot) {
        return Err(ERR_BAD_ARG);
    }
    let id = (w.units[me].team * w.team_size + slot) as usize;
    if id < w.units.len() {
        Ok(id)
    } else {
        Err(ERR_BAD_ARG)
    }
}

/// Row `ix` of `T_ROSTER`: unit `ix`, of either team.
pub fn roster_row(w: &World, ix: usize) -> Option<RosterInfo> {
    let u = w.units.get(ix)?;
    Some(RosterInfo {
        id: u.id as i32,
        team: u.team,
        kit: u.kit,
        credits: w.round_credits.get(ix).copied().unwrap_or(0),
        alive: u.alive as i32,
    })
}

/// Row `ix` of `T_ROUNDS`: the `ix`-th finished round, with the score once it was decided.
pub fn round_row(w: &World, ix: usize) -> Option<RoundInfo> {
    let r = w.rounds.get(ix)?;
    let won = |team: i32| w.rounds[..=ix].iter().filter(|x| x.winner == team).count() as i32;
    Some(RoundInfo {
        round: r.round,
        winner: r.winner,
        reason: r.reason,
        attackers: r.attackers,
        ticks: r.ticks,
        score_a: won(TEAM_A),
        score_b: won(TEAM_B),
    })
}
