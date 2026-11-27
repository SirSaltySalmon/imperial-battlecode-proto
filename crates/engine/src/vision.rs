//! Team sight and hearing. CORE FILE, split out of `world.rs`.
//!
//! Vision and sound are shared by the team: a team sees the union of what its living units see and hears every
//! sound any living teammate hears. `World::compute_vision` builds one `TeamView` per team at the end of each
//! tick (the enemies in sight and the fogged events); `World::perceived` filters bullets, effects and drops when
//! a bot reads them. Fog is applied to what a bot reads, never to what the world records, so replays and the
//! viewer keep everything.
//!
//! One thing is not shared by the team: where a hit came from. A unit that takes damage or is given a status
//! by an effect learns the direction (`EventInfo.dir`, `src_x`, `src_y`); its teammates read the same event
//! without it (`World::event_row`). Nobody learns which enemy did it.

use crate::fixed::{self, V2};
use crate::world::{World, CHARGE_CARRIED, CHARGE_DROPPED, CHARGE_PLANTED};
use crate::Fnv;
use ibc_abi::*;

/// Where a hit came from, as the unit that takes it learns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitFrom {
    /// Not said: a hit dealt by a tool or a test rather than by something in the world.
    Unknown,
    /// A bullet flying with this velocity: the victim learns the way back along its flight, never where the
    /// shooter stands.
    Bullet(V2),
    /// An effect centred here (fire, a flash pop, ...): the victim learns the centre, never the thrower.
    Area(V2),
}

impl HitFrom {
    /// (`dir`, `src_x`, `src_y`) for a victim standing at `at`. `dir` is 0 for a victim on the very centre.
    fn bearing(self, at: V2) -> (i32, i32, i32) {
        match self {
            HitFrom::Unknown => (DIR_HIDDEN, POS_HIDDEN, POS_HIDDEN),
            HitFrom::Bullet(v) => (fixed::atan2(-v.1, -v.0), POS_HIDDEN, POS_HIDDEN),
            HitFrom::Area(c) => (fixed::atan2(c.1 - at.1, c.0 - at.0), c.0, c.1),
        }
    }
}

/// An event without where it came from, as everyone but its target reads it.
fn undirected(e: EventInfo) -> EventInfo {
    EventInfo { dir: DIR_HIDDEN, src_x: POS_HIDDEN, src_y: POS_HIDDEN, ..e }
}

/// What one team perceives after a tick.
#[derive(Clone, Debug, Default)]
pub struct TeamView {
    /// Bit `j`: unit `j` is alive and seen by the team. The team's own living units are always set.
    pub units: u64,
    /// The tick's events as this team receives them (see `World::fog_event`).
    pub events: Vec<EventInfo>,
}

impl TeamView {
    pub fn hash(&self, h: &mut Fnv) {
        h.u64(self.units);
        h.i32(self.events.len() as i32);
        for e in &self.events {
            h.bytes(bytes_of(e));
        }
    }
}

impl World {
    /// Can any living unit of `team` see point `p`?
    pub fn team_sees_point(&self, team: i32, p: V2) -> bool {
        (0..self.units.len()).any(|i| self.units[i].team == team && self.can_see_point(i, p))
    }

    /// Rebuild both teams' views from the current state. Called at the end of every tick.
    pub(crate) fn compute_vision(&mut self) {
        self.views = [self.team_view(TEAM_A), self.team_view(TEAM_B)];
    }

    fn team_view(&self, team: i32) -> TeamView {
        let mut units = 0u64;
        for (j, u) in self.units.iter().enumerate() {
            if u.alive && (u.team == team || u.revealed > 0 || self.team_sees_point(team, u.pos)) {
                units |= 1 << j;
            }
        }
        TeamView { units, events: self.events.iter().filter_map(|e| self.fog_event(team, e)).collect() }
    }

    /// Indices of the rows of `T_PROJECTILES`, `T_EFFECTS` or `T_DROPS` that `team` perceives, in list order.
    /// Computed when read rather than every tick: bullets are many and few bots read them.
    pub fn perceived(&self, team: i32, table: u32) -> Vec<usize> {
        let attacking = self.side_of(team) == SIDE_ATTACK;
        let pick = |n: usize, keep: &dyn Fn(usize) -> bool| (0..n).filter(|&j| keep(j)).collect();
        match table {
            T_PROJECTILES => pick(self.projectiles.len(), &|j| {
                let p = &self.projectiles[j];
                p.team == team || self.team_sees_point(team, p.pos)
            }),
            T_EFFECTS => pick(self.effects.len(), &|j| crate::abilities::effect_visible(self, team, j)),
            T_DROPS => pick(self.drops.len(), &|j| {
                let d = &self.drops[j];
                (d.kind == DROP_CHARGE && attacking) || self.team_sees_point(team, (d.x, d.y))
            }),
            _ => Vec::new(),
        }
    }

    /// An event as `team` receives it, or `None` if the team does not get it.
    ///
    /// The position is known when the team sees it, when the event happened to one of the team's own units
    /// (damage, an effect, a kill), or when it is the charge's position and the team knows where the charge lies
    /// (everyone once it is planted, attackers when it is dropped). While the charge is carried its recorded
    /// position is stale, so a round end then never reveals one. Kills, plant, defuse, detonation and round end
    /// are always delivered, with the position replaced by `POS_HIDDEN` when it is not known. Damage, effects
    /// and any other kind are delivered only when the position is known.
    ///
    /// The actor of damage and of an effect is kept only for its own team, so a unit never learns which enemy
    /// hit it (kills keep their killer). Where a hit came from stays only on events that happened to one of the
    /// team's units; `event_row` then keeps it for that unit alone.
    fn fog_event(&self, team: i32, e: &EventInfo) -> Option<EventInfo> {
        let own_unit = |id: i32| id >= 0 && self.units.get(id as usize).is_some_and(|u| u.team == team);
        let sees = || self.team_sees_point(team, (e.x, e.y));
        let hit = matches!(e.kind, EVENT_DAMAGE | EVENT_EFFECT);
        let known = match e.kind {
            EVENT_KILL | EVENT_DAMAGE | EVENT_EFFECT => own_unit(e.target) || sees(),
            EVENT_PLANT | EVENT_DEFUSE | EVENT_DETONATE | EVENT_ROUND_END => match self.charge_state {
                CHARGE_PLANTED => true,
                CHARGE_DROPPED => self.side_of(team) == SIDE_ATTACK || sees(),
                CHARGE_CARRIED => false,
                _ => sees(),
            },
            _ => sees(),
        };
        let always = matches!(e.kind, EVENT_KILL | EVENT_PLANT | EVENT_DEFUSE | EVENT_DETONATE | EVENT_ROUND_END);
        let mut seen = match (known, always) {
            (true, _) => *e,
            (false, true) => EventInfo { x: POS_HIDDEN, y: POS_HIDDEN, ..*e },
            (false, false) => return None,
        };
        if hit && !own_unit(e.actor) {
            seen.actor = -1;
        }
        Some(if own_unit(e.target) { seen } else { undirected(seen) })
    }

    /// Row `ix` of `T_EVENTS` as unit `me` reads it: its team's event, with where it came from only if it
    /// happened to `me`. This is the one table teammates do not read alike.
    pub fn event_row(&self, me: usize, ix: usize) -> Option<EventInfo> {
        let e = *self.views[self.units[me].team as usize].events.get(ix)?;
        Some(if e.target == me as i32 { e } else { undirected(e) })
    }

    /// Record that unit `target` was hit from `from`: damage, or an effect giving it a status. The event is
    /// at the target's position and keeps what the target learns of where the hit came from.
    pub fn emit_hit(&mut self, kind: i32, actor: Option<usize>, target: usize, value: i32, from: HitFrom) {
        let at = self.units[target].pos;
        let (dir, src_x, src_y) = from.bearing(at);
        let actor = actor.map_or(-1, |a| a as i32);
        let (x, y, target) = (at.0, at.1, target as i32);
        self.events.push(EventInfo { kind, actor, target, x, y, value, dir, src_x, src_y });
    }

    /// Let `team`'s enemies hear a sound if any living one of them is within `radius`. The position is
    /// snapped to the tile centre.
    pub fn emit_sound(&mut self, kind: i32, pos: V2, team: i32, radius: i32) {
        let r2 = (radius as i64) * (radius as i64);
        for listener in [TEAM_A, TEAM_B] {
            if listener != team
                && self.units.iter().any(|u| u.alive && u.team == listener && fixed::dist2(u.pos, pos) <= r2)
            {
                self.hear(listener, kind, pos, team);
            }
        }
    }

    /// Add a sound made by `source` at `pos` to what team `listener` heard this tick.
    pub fn hear(&mut self, listener: i32, kind: i32, pos: V2, source: i32) {
        let snapped = (pos.0.div_euclid(TILE) * TILE + TILE / 2, pos.1.div_euclid(TILE) * TILE + TILE / 2);
        self.team_sounds[listener as usize].push(SoundInfo { kind, x: snapped.0, y: snapped.1, team: source });
    }
}
