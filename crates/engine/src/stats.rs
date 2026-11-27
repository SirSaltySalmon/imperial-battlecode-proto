//! Match statistics the world does not keep: whether each round saw a plant, every kill, and a digest of how
//! the match went that leaves out the seed. `Match::step` feeds this the events of every resolved tick
//! (`World::events`), so no mechanic writes here. It is not hashed into the state: nothing here feeds back
//! into the simulation, and it is a pure function of the events, so the same match gives the same statistics.

use crate::world::World;
use crate::Fnv;
use ibc_abi::{EVENT_KILL, EVENT_PLANT, EVENT_ROUND_END};

/// One `EVENT_KILL`: friendly kills and the detonation blast included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kill {
    /// The tick being played when the kill was resolved (`MatchInfo.tick`); its event is in the next frame.
    pub tick: u32,
    pub round: i32,
    /// Unit ids. `killer` is -1 for the detonation blast.
    pub killer: i32,
    pub victim: i32,
    /// Teams of the two units (`killer_team` -1 without a killer).
    pub killer_team: i32,
    pub victim_team: i32,
    /// Weapon id of the shot, -1 for an ability or the blast (the event's value).
    pub weapon: i32,
}

#[derive(Clone, Debug, Default)]
pub struct MatchStats {
    /// One per finished round, in the order of `World::rounds`: the charge was planted in that round.
    pub planted: Vec<bool>,
    pub kills: Vec<Kill>,
    /// The round being played has seen a plant.
    plant_seen: bool,
}

impl MatchStats {
    /// Read the events of the tick `w` has just resolved. Called once after every step.
    pub fn observe(&mut self, w: &World) {
        let team = |id: i32| u32::try_from(id).ok().and_then(|i| w.units.get(i as usize)).map_or(-1, |u| u.team);
        let mut round_ended = false;
        for e in &w.events {
            match e.kind {
                EVENT_PLANT => self.plant_seen = true,
                EVENT_ROUND_END => round_ended = true,
                EVENT_KILL => self.kills.push(Kill {
                    tick: w.tick.saturating_sub(1),
                    round: w.round,
                    killer: e.actor,
                    victim: e.target,
                    killer_team: team(e.actor),
                    victim_team: team(e.target),
                    weapon: e.value,
                }),
                _ => {}
            }
        }
        // A plant and the end of its round can come in the same tick, in either order.
        if round_ended {
            self.planted.push(std::mem::take(&mut self.plant_seen));
        }
    }

    /// FNV-1a over everything the summary says happened, and nothing else: the ruleset and map hashes, team
    /// size, result, length, every round (with `planted`), every kill and every unit's kit, fault and totals.
    /// The seed is not in it, so the same game played under two seeds has one digest.
    pub fn digest(&self, w: &World) -> u64 {
        let mut h = Fnv::new();
        h.u64(w.rules.hash());
        h.u64(w.map.hash());
        let head = [w.team_size, w.winner.unwrap_or(-1), w.score[0], w.score[1], w.tick as i32];
        for v in head.into_iter().chain([w.rounds.len() as i32]) {
            h.i32(v);
        }
        for (i, r) in w.rounds.iter().enumerate() {
            let planted = self.planted.get(i).copied().unwrap_or(false) as i32;
            for v in [r.round, r.winner, r.reason, r.attackers, r.ticks, planted] {
                h.i32(v);
            }
        }
        h.i32(self.kills.len() as i32);
        for k in &self.kills {
            for v in [k.tick as i32, k.round, k.killer, k.victim, k.killer_team, k.victim_team, k.weapon] {
                h.i32(v);
            }
        }
        for u in &w.units {
            let s = &u.stats;
            for v in [u.kit, u.faulted as i32, s.kills, s.deaths, s.damage, s.shots, s.plants, s.defuses] {
                h.i32(v);
            }
        }
        h.finish()
    }
}
