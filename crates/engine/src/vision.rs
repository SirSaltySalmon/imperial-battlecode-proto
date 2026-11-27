//! Team sight and hearing. CORE FILE, split out of `world.rs`.
//!
//! Vision and sound are shared by the team: a team sees the union of what its living units see and hears every
//! sound any living teammate hears. `World::compute_vision` builds one `TeamView` per team at the end of each
//! tick (the enemies in sight and the fogged events); `World::perceived` filters bullets, effects and drops when
//! a bot reads them. Fog is applied to what a bot reads, never to what the world records, so replays and the
//! viewer keep everything.

use crate::fixed::{self, V2};
use crate::world::{World, CHARGE_CARRIED, CHARGE_DROPPED, CHARGE_PLANTED};
use crate::Fnv;
use ibc_abi::*;
use std::borrow::Cow;
use std::cell::OnceCell;

/// The tables `World::perceived` filters, in the order of `TeamView::listed`.
const PERCEIVED: [u32; 3] = [T_PROJECTILES, T_EFFECTS, T_DROPS];

/// What one team perceives after a tick.
#[derive(Clone, Debug, Default)]
pub struct TeamView {
    /// Bit `j`: unit `j` is alive and seen by the team. The team's own living units are always set.
    pub units: u64,
    /// The tick's events as this team receives them (see `World::fog_event`).
    pub events: Vec<EventInfo>,
    /// The rows of each `PERCEIVED` table, kept from the first read of the tick (`World::perceived`). Not
    /// state, so not hashed: a function of the state, dropped with the view at every step.
    listed: [OnceCell<Vec<u32>>; 3],
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
        let events = self.events.iter().filter_map(|e| self.fog_event(team, e)).collect();
        TeamView { units, events, ..TeamView::default() }
    }

    /// Indices of the rows of `T_PROJECTILES`, `T_EFFECTS` or `T_DROPS` that `team` perceives, in list order.
    /// Computed when read rather than every tick: bullets are many and few bots read them.
    ///
    /// Only in the draft and buy phases do acts apply at once (`Host::act`; a buy-phase `A_FACE` turns the
    /// unit), so there every read lists afresh. In the other phases acts are queued as intents or refused, so
    /// nothing a bot reads changes until the next step: the first read of a tick lists the rows into the
    /// team's view, and later reads, by any unit of the team, reuse them. A bot reading n rows pays for one
    /// pass, not n. Code that edits `world` directly must do so before the tick's first read; debug builds
    /// check it.
    pub fn perceived(&self, team: i32, table: u32) -> Cow<'_, [u32]> {
        let Some(slot) = PERCEIVED.iter().position(|&t| t == table) else { return Cow::Borrowed(&[]) };
        if self.phase == PHASE_DRAFT || self.phase == PHASE_BUY {
            return Cow::Owned(self.list_perceived(team, table));
        }
        let rows = self.views[team as usize].listed[slot].get_or_init(|| self.list_perceived(team, table));
        debug_assert!(*rows == self.list_perceived(team, table), "`world` was edited after a read this tick");
        Cow::Borrowed(rows)
    }

    fn list_perceived(&self, team: i32, table: u32) -> Vec<u32> {
        let attacking = self.side_of(team) == SIDE_ATTACK;
        let pick = |n: usize, keep: &dyn Fn(usize) -> bool| (0..n as u32).filter(|&j| keep(j as usize)).collect();
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
    /// (damage or kill), or when it is the charge's position and the team knows where the charge lies
    /// (everyone once it is planted, attackers when it is dropped). While the charge is carried its recorded
    /// position is stale, so a round end then never reveals one. Kills, plant, defuse, detonation and round end
    /// are always delivered, with the position replaced by `POS_HIDDEN` when it is not known. Damage, and any
    /// other kind, is delivered only when its position is known.
    fn fog_event(&self, team: i32, e: &EventInfo) -> Option<EventInfo> {
        let own_unit = |id: i32| id >= 0 && self.units.get(id as usize).is_some_and(|u| u.team == team);
        let sees = || self.team_sees_point(team, (e.x, e.y));
        let known = match e.kind {
            EVENT_KILL | EVENT_DAMAGE => own_unit(e.target) || sees(),
            EVENT_PLANT | EVENT_DEFUSE | EVENT_DETONATE | EVENT_ROUND_END => match self.charge_state {
                CHARGE_PLANTED => true,
                CHARGE_DROPPED => self.side_of(team) == SIDE_ATTACK || sees(),
                CHARGE_CARRIED => false,
                _ => sees(),
            },
            _ => sees(),
        };
        let always = matches!(e.kind, EVENT_KILL | EVENT_PLANT | EVENT_DEFUSE | EVENT_DETONATE | EVENT_ROUND_END);
        match (known, always) {
            (true, _) => Some(*e),
            (false, true) => Some(EventInfo { x: POS_HIDDEN, y: POS_HIDDEN, ..*e }),
            (false, false) => None,
        }
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
