//! Per-tick observation of a match. The engine's summary has only end-of-match totals, so everything a
//! balance report needs beyond those (first contact, kill distances, buy patterns, movement) is collected
//! here by watching the world after every step. The same collector runs on a live native match and on a
//! re-simulated replay, so both backends produce identical details.

use ibc_abi::{EVENT_DAMAGE, EVENT_KILL, EVENT_PLANT, EVENT_ROUND_END, PHASE_BUY, SIDE_ATTACK};
use ibc_engine::{fixed, Match};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundDetail {
    pub round: i32,
    pub planted: bool,
    /// Action ticks until some unit first saw an enemy / until the first kill. `None` if it never happened.
    pub first_contact: Option<i32>,
    pub first_kill: Option<i32>,
    pub shots: i64,
    pub kills: i64,
    pub damage: i64,
    /// Distance covered by all units together during the action phase, in fixed-point units.
    pub moved: i64,
    pub action_ticks: i32,
    /// Per side (attackers, defenders): team credits at the first buy tick and credits spent in the buy phase.
    pub credits_start: [i64; 2],
    pub spent: [i64; 2],
    /// Per side: weapon id in hand of every unit when the action phase starts.
    pub loadout: [Vec<i32>; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillRec {
    pub round: i32,
    /// Weapon id from the kill event (negative when it was not a weapon).
    pub weapon: i32,
    /// Euclidean distance between killer and victim in fixed-point units, -1 without a killer.
    pub dist: i32,
    /// Action tick of the round.
    pub tick: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detail {
    pub rounds: Vec<RoundDetail>,
    pub kills: Vec<KillRec>,
    /// Trigger pulls by weapon id.
    pub shots_by_weapon: Vec<i64>,
    /// Weapon names of the ruleset the game was played on, by id.
    pub weapon_names: Vec<String>,
}

#[derive(Clone, Copy)]
struct UnitSnap {
    pos: (i32, i32),
    credits: i32,
    shots: i32,
}

/// Feed it every frame of a match (before the first tick and after every step), then call `finish`.
#[derive(Default)]
pub struct Collector {
    detail: Detail,
    current: RoundDetail,
    prev_units: Vec<UnitSnap>,
    prev_round: i32,
    prev_phase: i32,
    prev_in_action: bool,
    buy_seen: bool,
    action_seen: bool,
}

impl Collector {
    pub fn new() -> Collector {
        Collector::default()
    }

    pub fn observe(&mut self, m: &Match) {
        let w = &m.world;
        if self.detail.weapon_names.is_empty() {
            self.detail.weapon_names = w.rules.weapon_names.clone();
            self.detail.shots_by_weapon = vec![0; w.rules.weapons.len()];
        }
        let same_round = w.round == self.prev_round && self.prev_units.len() == w.units.len();
        if !same_round {
            self.current = RoundDetail {
                round: w.round,
                ..Default::default()
            };
            self.buy_seen = false;
            self.action_seen = false;
        }
        let side = |team: i32| if w.side_of(team) == SIDE_ATTACK { 0 } else { 1 };
        let in_action = w.in_action();

        if w.phase == PHASE_BUY && !self.buy_seen {
            self.buy_seen = true;
            for u in &w.units {
                self.current.credits_start[side(u.team)] += u.credits as i64;
            }
        }
        if in_action {
            if self.action_seen {
                self.current.action_ticks += 1;
            } else {
                self.action_seen = true;
                for u in &w.units {
                    let held = if u.weapons[0] >= 0 {
                        u.weapons[0]
                    } else {
                        u.weapons[1]
                    };
                    self.current.loadout[side(u.team)].push(held);
                }
            }
        }
        // Events of the step that just resolved belong to the action tick after the last counted one when
        // that step also ended the round.
        let event_tick = if in_action {
            self.current.action_ticks
        } else {
            self.current.action_ticks + 1
        };

        for (i, u) in w.units.iter().enumerate() {
            let prev = self.prev_units.get(i).copied();
            if let Some(p) = prev {
                let fired = (u.stats.shots - p.shots).max(0) as i64;
                if fired > 0 && w.round > 0 {
                    self.current.shots += fired;
                    if let Some(n) = self
                        .detail
                        .shots_by_weapon
                        .get_mut(u.weapon().max(0) as usize)
                    {
                        *n += fired;
                    }
                }
                if same_round && self.prev_phase == PHASE_BUY && p.credits > u.credits {
                    self.current.spent[side(u.team)] += (p.credits - u.credits) as i64;
                }
                if same_round && self.prev_in_action {
                    self.current.moved += fixed::dist(p.pos, u.pos) as i64;
                }
            }
        }

        if in_action && self.current.first_contact.is_none() {
            let sees_enemy = w.units.iter().any(|u| {
                u.alive
                    && w.units
                        .iter()
                        .any(|o| o.alive && o.team != u.team && u.seen & (1u64 << o.id) != 0)
            });
            if sees_enemy {
                self.current.first_contact = Some(self.current.action_ticks);
            }
        }

        let mut round_ended = false;
        for e in &w.events {
            match e.kind {
                EVENT_KILL => {
                    self.current.kills += 1;
                    self.current.first_kill.get_or_insert(event_tick);
                    let dist = match w.units.get(e.actor.max(0) as usize) {
                        Some(killer) if e.actor >= 0 => fixed::dist(killer.pos, (e.x, e.y)),
                        _ => -1,
                    };
                    self.detail.kills.push(KillRec {
                        round: w.round,
                        weapon: e.value,
                        dist,
                        tick: event_tick,
                    });
                }
                EVENT_DAMAGE => self.current.damage += e.value as i64,
                EVENT_PLANT => self.current.planted = true,
                EVENT_ROUND_END => round_ended = true,
                _ => {}
            }
        }
        if round_ended {
            // The step that ended the round was an action step too, though its frame is past the phase.
            self.current.action_ticks = event_tick;
            self.detail.rounds.push(self.current.clone());
        }

        self.prev_units = w
            .units
            .iter()
            .map(|u| UnitSnap {
                pos: u.pos,
                credits: u.credits,
                shots: u.stats.shots,
            })
            .collect();
        self.prev_round = w.round;
        self.prev_phase = w.phase;
        self.prev_in_action = in_action;
    }

    pub fn finish(self) -> Detail {
        self.detail
    }
}
