//! What the player pressed, turned into the acts a bot would make.
//!
//! The page sends the state of its controls (`Input`): held things (move direction, walk, aim point, fire,
//! interact) replace the previous ones, one-shot things (reload, ability, buy, ...) are queued. Once
//! per tick `Controller::acts` turns them into `act(kind, a, b, c)` calls, reading the unit through the same
//! `Host` queries a bot has. Nothing here decides what is allowed: the engine validates every act exactly as
//! it validates a bot's (turn rate, prices, phases, cooldowns).

use ibc_abi::*;
use ibc_engine::fixed;
use serde::Deserialize;
use std::collections::VecDeque;

/// One message from the page. Every field is optional; held fields that are left out keep their value.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Move direction as a vector in screen axes (+x right, +y down), e.g. from WASD. `[0, 0]` stops.
    #[serde(rename = "move")]
    pub mv: Option<[i32; 2]>,
    /// Move at walk speed (quiet) instead of running.
    pub walk: Option<bool>,
    /// World point (fixed-point units) to face, shoot at and target abilities at. `null` clears it.
    #[serde(default, deserialize_with = "some_or_null")]
    pub aim: Option<Option<[i32; 2]>>,
    /// Trigger held.
    pub fire: Option<bool>,
    /// F held: first pick up what is in reach (once per press), else plant (attackers) or defuse (defenders).
    pub interact: Option<bool>,
    /// Active mode: let time pass while held, even with nothing else to do (Space).
    pub wait: Option<bool>,
    /// One-shot actions, applied in order on the next ticks.
    #[serde(default)]
    pub actions: Vec<Action>,
}

/// Distinguishes `"aim": null` (clear) from a missing `aim` (keep).
fn some_or_null<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<[i32; 2]>>, D::Error> {
    Option::<[i32; 2]>::deserialize(d).map(Some)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "act", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Reload,
    /// Use the ability in this slot (0 or 1) at the aim point.
    Ability {
        slot: i32,
    },
    Buy {
        item: i32,
    },
    Swap {
        slot: i32,
    },
    Pickup,
    Drop {
        kind: i32,
    },
    Cancel,
}

/// One `act(kind, a, b, c)` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Act {
    pub kind: u32,
    pub a: i32,
    pub b: i32,
    pub c: i32,
}

/// Held controls plus the queue of one-shot actions.
#[derive(Clone, Debug, Default)]
pub struct Controller {
    mv: [i32; 2],
    walk: bool,
    aim: Option<[i32; 2]>,
    fire: bool,
    interact: bool,
    /// F already picked something up during this press: one pickup per press, so holding F over two guns
    /// does not swap them back and forth every tick.
    picked: bool,
    wait: bool,
    queue: VecDeque<Action>,
}

/// Most one-shot actions waiting at once; more are dropped (a stuck key must not grow memory).
const MAX_QUEUE: usize = 32;
/// Largest aim coordinate taken from the page, in fixed-point units (16384 tiles).
const MAX_AIM: i32 = 1 << 24;

impl Controller {
    pub fn apply(&mut self, input: Input) {
        if let Some(mv) = input.mv {
            self.mv = mv;
        }
        if let Some(walk) = input.walk {
            self.walk = walk;
        }
        if let Some(aim) = input.aim {
            // Far beyond any map, and small enough that `aim - position` cannot overflow.
            self.aim = aim.map(|p| p.map(|v| v.clamp(-MAX_AIM, MAX_AIM)));
        }
        if let Some(fire) = input.fire {
            self.fire = fire;
        }
        if let Some(interact) = input.interact {
            if interact && !self.interact {
                self.picked = false;
            }
            self.interact = interact;
        }
        if let Some(wait) = input.wait {
            self.wait = wait;
        }
        for a in input.actions {
            if self.queue.len() < MAX_QUEUE {
                self.queue.push_back(a);
            }
        }
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Drop the queued actions (they were meant for another unit).
    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }

    /// Active mode: whether the player is doing something that takes ticks. Waiting always does; moving,
    /// firing, planting and queued actions (reload, ability, ...) do in the action phases for a live unit.
    /// Aiming alone, and buys outside the action phases, wait for the next tick someone asks for.
    pub fn busy(&self, phase: i32, alive: bool) -> bool {
        let acting = self.mv != [0, 0] || self.fire || self.interact || !self.queue.is_empty();
        self.wait || (alive && matches!(phase, PHASE_ACTION | PHASE_PLANTED) && acting)
    }

    /// The acts for this tick, from what `unit` can see of the match. Outside the action phases every
    /// queued action is sent (buys apply at once); in them, at most one primary action per tick
    /// (the engine keeps only one), queued actions before held ones.
    pub fn acts(&mut self, game: &impl Host, unit: u32) -> Vec<Act> {
        let (Some(me), Some(info), Some(core)) = (
            read::<UnitInfo>(game, unit, T_SELF),
            read::<MatchInfo>(game, unit, T_MATCH),
            read::<CoreRules>(game, unit, T_RULES_CORE),
        ) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let aim_angle = self.aim.map(|[x, y]| fixed::atan2(y - me.y, x - me.x));
        let target = self.aim.unwrap_or([me.x, me.y]);
        let action_phase = matches!(info.phase, PHASE_ACTION | PHASE_PLANTED);
        if !action_phase {
            if let (PHASE_BUY, Some(angle)) = (info.phase, aim_angle) {
                out.push(act(A_FACE, angle, 0, 0));
            }
            out.extend(self.queue.drain(..).map(|a| one_shot(a, target)));
            return out;
        }
        if self.mv != [0, 0] {
            let speed = if self.walk {
                core.walk_speed
            } else {
                core.run_speed
            };
            out.push(act(A_MOVE, fixed::atan2(self.mv[1], self.mv[0]), speed, 0));
        }
        if let Some(angle) = aim_angle {
            out.push(act(A_FACE, angle, 0, 0));
        }
        if let Some(a) = self.queue.pop_front() {
            out.push(one_shot(a, target));
        } else if self.interact {
            let attacking = info.my_side == SIDE_ATTACK;
            let channeling = me.state & (S_PLANTING | S_DEFUSING) != 0;
            let kind = if !channeling
                && !self.picked
                && pickable_in_reach(game, unit, &me, &core, attacking)
            {
                self.picked = true;
                A_PICKUP
            } else if attacking {
                A_PLANT
            } else {
                A_DEFUSE
            };
            out.push(act(kind, 0, 0, 0));
        } else if self.fire {
            out.push(act(A_FIRE, aim_angle.unwrap_or(me.facing), 0, 0));
        }
        out
    }
}

/// Whether something `A_PICKUP` would take lies within the unit's pickup reach: a weapon, or the charge for
/// an attacker.
fn pickable_in_reach(
    game: &impl Host,
    unit: u32,
    me: &UnitInfo,
    core: &CoreRules,
    attacking: bool,
) -> bool {
    let reach = core.pickup_reach as i64;
    let mut buf = Vec::new();
    (0..game.count(unit, T_DROPS).max(0) as u32).any(|i| {
        buf.clear();
        game.get(unit, T_DROPS, i, &mut buf) >= 0
            && from_bytes::<DropInfo>(&buf).is_some_and(|d| {
                (d.kind == DROP_WEAPON || (d.kind == DROP_CHARGE && attacking))
                    && fixed::dist2((me.x, me.y), (d.x, d.y)) <= reach * reach
            })
    })
}

fn act(kind: u32, a: i32, b: i32, c: i32) -> Act {
    Act { kind, a, b, c }
}

fn one_shot(a: Action, target: [i32; 2]) -> Act {
    match a {
        Action::Reload => act(A_RELOAD, 0, 0, 0),
        Action::Ability { slot } => act(A_ABILITY, slot, target[0], target[1]),
        Action::Buy { item } => act(A_BUY, item, 0, 0),
        Action::Swap { slot } => act(A_SWAP, slot, 0, 0),
        Action::Pickup => act(A_PICKUP, 0, 0, 0),
        Action::Drop { kind } => act(A_DROP, kind, 0, 0),
        Action::Cancel => act(A_CANCEL, 0, 0, 0),
    }
}

/// One record of a single-row table, as a bot reads it.
pub fn read<T: Copy + Default>(game: &impl Host, unit: u32, table: u32) -> Option<T> {
    let mut buf = Vec::new();
    (game.get(unit, table, 0, &mut buf) >= 0)
        .then(|| from_bytes(&buf))
        .flatten()
}

/// Name of an act kind, for the page's feedback line.
pub fn act_name(kind: u32) -> &'static str {
    match kind {
        A_MOVE => "move",
        A_FACE => "face",
        A_FIRE => "fire",
        A_RELOAD => "reload",
        A_PLANT => "plant",
        A_DEFUSE => "defuse",
        A_ABILITY => "ability",
        A_BUY => "buy",
        A_SWAP => "swap",
        A_DROP => "drop",
        A_PICKUP => "pickup",
        A_DRAFT => "draft",
        A_CANCEL => "cancel",
        _ => "act",
    }
}

/// Name of an `act` result code.
pub fn code_name(code: i32) -> &'static str {
    match code {
        OK => "ok",
        ERR_BAD_ARG => "bad argument",
        ERR_UNSUPPORTED => "unsupported",
        ERR_NOT_ALLOWED => "not allowed now",
        ERR_NOT_READY => "not ready",
        ERR_OUT_OF_RANGE => "out of range",
        ERR_INSUFFICIENT_FUNDS => "not enough credits",
        _ => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibc_engine::{Map, Match, Ruleset};

    const MAP: &str = include_str!("../../../../maps/crossfire.ibcmap");

    fn game() -> Match {
        Match::new(Ruleset::builtin(), Map::parse(MAP).unwrap(), 7)
    }

    fn parse(json: &str) -> Input {
        serde_json::from_str(json).unwrap()
    }

    /// Step through the draft and the buy phase into the action phase.
    fn to_action(m: &mut Match) {
        while m.world.phase != PHASE_ACTION {
            m.step();
        }
    }

    #[test]
    fn parses_held_and_one_shot_fields() {
        let i = parse(
            r#"{"move":[1,-1],"walk":true,"aim":[10,20],"fire":true,"actions":[{"act":"buy","item":3},{"act":"reload"}]}"#,
        );
        assert_eq!(i.mv, Some([1, -1]));
        assert_eq!(i.aim, Some(Some([10, 20])));
        assert_eq!(i.actions, vec![Action::Buy { item: 3 }, Action::Reload]);
        assert_eq!(parse(r#"{"aim":null}"#).aim, Some(None));
        assert_eq!(parse("{}").aim, None);
        assert!(serde_json::from_str::<Input>(r#"{"teleport":true}"#).is_err());
        assert!(serde_json::from_str::<Input>(r#"{"actions":[{"act":"win"}]}"#).is_err());
    }

    #[test]
    fn outside_the_action_phases_every_queued_action_goes_out() {
        let mut m = game();
        let mut c = Controller::default();
        assert!(
            serde_json::from_str::<Input>(r#"{"actions":[{"act":"draft","kit":2}]}"#).is_err(),
            "bots draft"
        );
        c.apply(parse(
            r#"{"actions":[{"act":"buy","item":2},{"act":"buy","item":1}]}"#,
        ));
        let acts = c.acts(&m, 0);
        assert_eq!(acts, vec![act(A_BUY, 2, 0, 0), act(A_BUY, 1, 0, 0)]);
        assert_eq!(c.queued(), 0);
        m.step();
        assert_eq!(m.world.phase, PHASE_BUY);
        c.apply(parse(r#"{"aim":[0,0],"move":[1,0]}"#));
        let acts = c.acts(&m, 0);
        assert_eq!(
            acts.len(),
            1,
            "no movement in the buy phase, only facing: {acts:?}"
        );
        assert_eq!(acts[0].kind, A_FACE);
    }

    #[test]
    fn action_phase_moves_faces_and_sends_one_primary() {
        let mut m = game();
        to_action(&mut m);
        let me = read::<UnitInfo>(&m, 0, T_SELF).unwrap();
        let core = m.world.rules.core;
        let mut c = Controller::default();
        c.apply(parse(&format!(
            r#"{{"move":[0,1],"walk":true,"aim":[{},{}],"fire":true,"actions":[{{"act":"reload"}},{{"act":"ability","slot":1}}]}}"#,
            me.x + 1000,
            me.y
        )));
        let acts = c.acts(&m, 0);
        assert_eq!(
            acts[0],
            act(A_MOVE, 16384, core.walk_speed, 0),
            "down is a quarter turn"
        );
        assert_eq!(acts[1], act(A_FACE, 0, 0, 0), "aim to the right is angle 0");
        assert_eq!(acts[2], act(A_RELOAD, 0, 0, 0), "queued before held");
        assert_eq!(acts.len(), 3);
        let acts = c.acts(&m, 0);
        assert_eq!(
            acts[2],
            act(A_ABILITY, 1, me.x + 1000, me.y),
            "abilities target the aim point"
        );
        let acts = c.acts(&m, 0);
        assert_eq!(acts[2], act(A_FIRE, 0, 0, 0), "then the held trigger");
        c.apply(parse(r#"{"interact":true,"walk":false}"#));
        let acts = c.acts(&m, 0);
        let side = read::<MatchInfo>(&m, 0, T_MATCH).unwrap().my_side;
        assert_eq!(acts[0].b, core.run_speed);
        assert_eq!(
            acts[2].kind,
            if side == SIDE_ATTACK {
                A_PLANT
            } else {
                A_DEFUSE
            },
            "interact wins over fire"
        );
        c.apply(parse(
            r#"{"move":[0,0],"aim":null,"interact":false,"fire":false}"#,
        ));
        assert!(c.acts(&m, 0).is_empty());
    }

    #[test]
    fn f_picks_up_once_per_press_then_plants_or_defuses() {
        let mut m = game();
        to_action(&mut m);
        let pos = m.world.units[0].pos;
        let attacking = read::<MatchInfo>(&m, 0, T_MATCH).unwrap().my_side == SIDE_ATTACK;
        let channel = if attacking { A_PLANT } else { A_DEFUSE };
        let primary = |c: &mut Controller, m: &Match| c.acts(m, 0).last().map(|a| a.kind);
        let mut c = Controller::default();
        c.apply(parse(r#"{"interact":true}"#));
        assert_eq!(
            primary(&mut c, &m),
            Some(channel),
            "nothing on the ground: F plants or defuses"
        );

        m.world.drops.push(DropInfo {
            kind: DROP_WEAPON,
            x: pos.0 + 100,
            y: pos.1,
            item: 1,
            ammo: 0,
        });
        c.apply(parse(r#"{"interact":false}"#));
        c.apply(parse(r#"{"interact":true}"#));
        assert_eq!(
            primary(&mut c, &m),
            Some(A_PICKUP),
            "a fresh press takes the weapon in reach"
        );
        assert_eq!(
            primary(&mut c, &m),
            Some(channel),
            "holding on does not pick up again"
        );
        c.apply(parse(r#"{"interact":true}"#));
        assert_eq!(
            primary(&mut c, &m),
            Some(channel),
            "a repeated held message is not a new press"
        );
        c.apply(parse(r#"{"interact":false}"#));
        c.apply(parse(r#"{"interact":true}"#));
        assert_eq!(primary(&mut c, &m), Some(A_PICKUP), "the next press does");

        m.world.drops.clear();
        m.world.drops.push(DropInfo {
            kind: DROP_CHARGE,
            x: pos.0,
            y: pos.1 + 100,
            item: 0,
            ammo: 0,
        });
        c.apply(parse(r#"{"interact":false}"#));
        c.apply(parse(r#"{"interact":true}"#));
        let want = if attacking { A_PICKUP } else { A_DEFUSE };
        assert_eq!(
            primary(&mut c, &m),
            Some(want),
            "only attackers pick up the charge"
        );

        let far = m.world.rules.core.pickup_reach + 1;
        m.world.drops[0] = DropInfo {
            kind: DROP_WEAPON,
            x: pos.0 + far,
            y: pos.1,
            item: 1,
            ammo: 0,
        };
        c.apply(parse(r#"{"interact":false}"#));
        c.apply(parse(r#"{"interact":true}"#));
        assert_eq!(primary(&mut c, &m), Some(channel), "out of reach");
    }

    #[test]
    fn absurd_aim_points_are_clamped() {
        let mut m = game();
        to_action(&mut m);
        let mut c = Controller::default();
        c.apply(parse(r#"{"aim":[-2147483648,2147483647],"fire":true}"#));
        assert_eq!(c.aim, Some([-MAX_AIM, MAX_AIM]));
        assert_eq!(c.acts(&m, 0).len(), 2, "face and fire, no overflow");
    }

    #[test]
    fn busy_means_acting_in_the_action_phase_or_waiting() {
        let mut c = Controller::default();
        c.apply(parse(r#"{"aim":[5,5]}"#));
        assert!(!c.busy(PHASE_ACTION, true), "aiming alone takes no ticks");
        c.apply(parse(r#"{"move":[1,0]}"#));
        assert!(c.busy(PHASE_ACTION, true) && c.busy(PHASE_PLANTED, true));
        assert!(!c.busy(PHASE_ACTION, false), "a dead unit cannot act");
        assert!(!c.busy(PHASE_BUY, true), "no moving in the buy phase");
        c.apply(parse(
            r#"{"move":[0,0],"actions":[{"act":"buy","item":1}]}"#,
        ));
        assert!(
            !c.busy(PHASE_BUY, true),
            "a buy waits for a tick someone asks for"
        );
        assert!(c.busy(PHASE_ACTION, true), "a queued action takes a tick");
        c.clear_queue();
        assert!(!c.busy(PHASE_ACTION, true));
        c.apply(parse(r#"{"wait":true}"#));
        assert!(
            c.busy(PHASE_BUY, false) && c.busy(PHASE_ROUND_END, true),
            "waiting passes time anywhere"
        );
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut c = Controller::default();
        for _ in 0..100 {
            c.apply(Input {
                actions: vec![Action::Reload],
                ..Input::default()
            });
        }
        assert_eq!(c.queued(), MAX_QUEUE);
    }
}
