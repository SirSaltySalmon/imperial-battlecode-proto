//! Per-tick observation of a match. The engine's summary has end-of-match totals, every round (with
//! whether the charge was planted) and every kill (with its weapon), so a balance report reads those from
//! it; what it needs beyond them (first contact, kill distances, buy patterns, movement, shots by weapon) is
//! collected here by watching the world after every step. The same collector runs on a live native match and
//! on a re-simulated replay, so both backends produce identical details.

use ibc_abi::EventInfo;
use ibc_abi::{
    EVENT_DAMAGE, EVENT_KILL, EVENT_ROUND_END, ITEM_KIND_ARMOR, PHASE_BUY, SIDE_ATTACK, TEAM_A,
    TEAM_B,
};
use ibc_engine::{fixed, Match, Ruleset};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundDetail {
    pub round: i32,
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
    /// The attacking team (`TEAM_A` or `TEAM_B`). Only meaningful from `DETAIL_VERSION` 1 on.
    #[serde(default)]
    pub attack_team: i32,
}

/// What a `Detail` records. 0: up to the kill distance; 1: per-weapon kill and damage attribution
/// (`KillRec`'s weapon, carrier, teams and victim armour; `Detail`'s rewards, armour classes and
/// `damage_by_carried`; `RoundDetail::attack_team`). Older details load with version 0 and the newer
/// fields at their defaults, which nothing may read.
pub const DETAIL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillRec {
    pub round: i32,
    /// Euclidean distance between killer and victim in fixed-point units, -1 without a killer.
    pub dist: i32,
    /// Action tick of the round.
    pub tick: i32,
    /// Weapon id the kill was made with (the engine's `EVENT_KILL` value), -1 for an ability.
    #[serde(default)]
    pub weapon: i32,
    /// Weapon the killer carried when the action phase started (see `RoundDetail::loadout`), -1 unknown.
    #[serde(default)]
    pub carried: i32,
    /// Killer's and victim's teams.
    #[serde(default)]
    pub team: i32,
    #[serde(default)]
    pub victim_team: i32,
    /// The victim's armour points when the action phase started.
    #[serde(default)]
    pub victim_armor: i32,
}

impl KillRec {
    /// A kill of an enemy: the only kind that is paid, and counted in the engine's stats.
    pub fn enemy(&self) -> bool {
        self.team != self.victim_team
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detail {
    pub rounds: Vec<RoundDetail>,
    pub kills: Vec<KillRec>,
    /// Trigger pulls by weapon id.
    pub shots_by_weapon: Vec<i64>,
    /// Weapon names of the ruleset the game was played on, by id.
    pub weapon_names: Vec<String>,
    /// `DETAIL_VERSION` of the collector that wrote this; 0 for details older than the field.
    #[serde(default)]
    pub version: u32,
    /// Kill reward by weapon id, and the reward for a kill without a weapon, of the game's ruleset.
    #[serde(default)]
    pub kill_rewards: Vec<i64>,
    #[serde(default)]
    pub ability_kill_reward: i64,
    /// Armour items of the ruleset: armour points and name (see `armor_class`).
    #[serde(default)]
    pub armor_classes: Vec<(i32, String)>,
    /// Damage dealt to enemies by weapon carried at action start (index = weapon id), per side
    /// (attackers, defenders). Abilities included: it is what the carriers did.
    #[serde(default)]
    pub damage_by_carried: Vec<[i64; 2]>,
}

impl Detail {
    /// The kill reward the engine paid for a kill made with `weapon` (`economy::on_kill`).
    pub fn reward(&self, weapon: i32) -> i64 {
        usize::try_from(weapon)
            .ok()
            .and_then(|i| self.kill_rewards.get(i))
            .copied()
            .unwrap_or(self.ability_kill_reward)
    }

    /// Armour class of `armor` points: "none" at 0, else the armour item with the fewest points that
    /// still covers them (worn armour carried over from the last round counts as the tier it fits in),
    /// the biggest item above them all.
    pub fn armor_class(&self, armor: i32) -> String {
        if armor <= 0 {
            return "none".into();
        }
        let covering = self.armor_classes.iter().filter(|c| c.0 >= armor);
        covering
            .min_by_key(|c| c.0)
            .or_else(|| self.armor_classes.iter().max_by_key(|c| c.0))
            .map(|c| c.1.clone())
            .unwrap_or_else(|| "armor".into())
    }
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
    /// Per unit index, this round: weapon in hand and armour points when the action phase started.
    carried: Vec<i32>,
    armor_start: Vec<i32>,
}

impl Collector {
    pub fn new() -> Collector {
        Collector::default()
    }

    pub fn observe(&mut self, m: &Match) {
        let w = &m.world;
        if self.detail.weapon_names.is_empty() {
            self.take_rules(&w.rules);
        }
        let same_round = w.round == self.prev_round && self.prev_units.len() == w.units.len();
        if !same_round {
            self.current = RoundDetail {
                round: w.round,
                ..Default::default()
            };
            self.buy_seen = false;
            self.action_seen = false;
            self.carried.clear();
            self.armor_start.clear();
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
                self.snapshot_action_start(m);
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
            let sees_enemy = (0..2).any(|t| {
                w.units.iter().any(|o| {
                    o.alive && o.team != t && w.views[t as usize].units & (1u64 << o.id) != 0
                })
            });
            if sees_enemy {
                self.current.first_contact = Some(self.current.action_ticks);
            }
        }

        let mut round_ended = false;
        for e in &w.events {
            match e.kind {
                // Detonation-blast deaths (no killer) are not kills, as in the engine's own stats.
                EVENT_KILL if e.actor >= 0 => {
                    self.current.kills += 1;
                    self.current.first_kill.get_or_insert(event_tick);
                    let rec = self.kill_rec(m, e, event_tick);
                    self.detail.kills.push(rec);
                }
                EVENT_DAMAGE => {
                    self.current.damage += e.value as i64;
                    self.damage_by_carrier(m, e.actor, e.target, e.value);
                }
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

    /// What the details need from the ruleset, read once.
    fn take_rules(&mut self, rules: &Ruleset) {
        let d = &mut self.detail;
        d.weapon_names = rules.weapon_names.clone();
        d.shots_by_weapon = vec![0; rules.weapons.len()];
        d.version = DETAIL_VERSION;
        d.kill_rewards = rules.weapons.iter().map(|x| x.kill_reward as i64).collect();
        d.ability_kill_reward = rules.flow.ability_kill_reward as i64;
        d.armor_classes = rules
            .items
            .iter()
            .zip(&rules.item_names)
            .filter(|(it, _)| it.kind == ITEM_KIND_ARMOR)
            .map(|(it, name)| (it.value, name.clone()))
            .collect();
        d.damage_by_carried = vec![[0; 2]; rules.weapons.len()];
    }

    /// The first action frame of a round: who attacks, and every unit's weapon in hand and armour.
    fn snapshot_action_start(&mut self, m: &Match) {
        let w = &m.world;
        self.current.attack_team = if w.side_of(TEAM_A) == SIDE_ATTACK {
            TEAM_A
        } else {
            TEAM_B
        };
        self.carried.clear();
        self.armor_start.clear();
        for u in &w.units {
            let held = if u.weapons[0] >= 0 {
                u.weapons[0]
            } else {
                u.weapons[1]
            };
            let side = if w.side_of(u.team) == SIDE_ATTACK {
                0
            } else {
                1
            };
            self.current.loadout[side].push(held);
            self.carried.push(held);
            self.armor_start.push(u.armor);
        }
    }

    /// The record of an `EVENT_KILL` with a killer.
    fn kill_rec(&self, m: &Match, e: &EventInfo, tick: i32) -> KillRec {
        let w = &m.world;
        let unit = |i: i32| usize::try_from(i).ok().and_then(|i| w.units.get(i));
        let at_start = |v: &[i32], i: i32| usize::try_from(i).ok().and_then(|i| v.get(i).copied());
        KillRec {
            round: w.round,
            dist: unit(e.actor).map_or(-1, |killer| fixed::dist(killer.pos, (e.x, e.y))),
            tick,
            weapon: e.value,
            carried: at_start(&self.carried, e.actor).unwrap_or(-1),
            team: unit(e.actor).map_or(-1, |u| u.team),
            victim_team: unit(e.target).map_or(-1, |u| u.team),
            victim_armor: at_start(&self.armor_start, e.target).unwrap_or(0),
        }
    }

    /// Credit damage to an enemy to the weapon its dealer carried at action start.
    fn damage_by_carrier(&mut self, m: &Match, actor: i32, target: i32, value: i32) {
        let w = &m.world;
        let (Ok(ai), Ok(ti)) = (usize::try_from(actor), usize::try_from(target)) else {
            return;
        };
        let (Some(a), Some(t)) = (w.units.get(ai), w.units.get(ti)) else {
            return;
        };
        let carried = self.carried.get(ai).copied().unwrap_or(-1);
        if a.team == t.team || carried < 0 {
            return;
        }
        let side = if w.side_of(a.team) == SIDE_ATTACK {
            0
        } else {
            1
        };
        if let Some(d) = self.detail.damage_by_carried.get_mut(carried as usize) {
            d[side] += value as i64;
        }
    }

    pub fn finish(self) -> Detail {
        self.detail
    }
}
