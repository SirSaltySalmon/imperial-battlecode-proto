//! The round itself: shoot, move, plant, defuse.
use crate::kit::Scene;
use crate::{Attack, Defence, Squad, MEM_SIGHT, SOUND_TTL};
use ibc::*;

/// Keep channelling a plant or defuse under fire when this few ticks remain.
const FINISH_CHANNEL_TICKS: i32 = 2;
/// Interact from well inside the allowed reach, so rounding never puts us outside it.
const REACH_PCT: i32 = 70;
/// `MatchInfo::charge_state` values.
const CHARGE_DROPPED: i32 = 1;
/// Guards stand this far from the planted charge.
const GUARD_RING: i32 = 1400;
/// A teammate this close covers a plant or a defuse.
const COVER: i32 = 5 * TILE;
/// A retaking team gathers once it is this close (path length) to the charge.
const STAGE: i32 = 8 * TILE;
/// Attackers walking in start running when the walk would leave fewer spare ticks than this.
const RUN_MARGIN_TICKS: i32 = 15;
/// A retake stops waiting when the fuse leaves fewer spare ticks than this.
const RETAKE_MARGIN_TICKS: i32 = 4;
/// Attackers run at the charge for this long after hearing a defuse.
const DEFUSE_ALERT_TICKS: i32 = 3;

impl Squad {
    pub(crate) fn fight(&mut self, me: &UnitInfo, info: &MatchInfo) {
        // A plant or defuse that is nearly done is worth more than one more shot.
        if me.has(S_PLANTING | S_DEFUSING) && me.channel <= FINISH_CHANNEL_TICKS {
            let _ = if me.has(S_PLANTING) { plant() } else { defuse() };
            return;
        }
        if self.kit.looking_away(info) {
            return;
        }
        self.listen(me, info);
        let attacking = info.my_side == SIDE_ATTACK;
        let pace = self.pace(me, info);
        let shot = self.gun.engage(me);
        if let Some(s) = shot {
            let _ = team_mem_set(MEM_SIGHT, [info.tick + 1, s.enemy.x, s.enemy.y]);
        }
        let fighting = shot.is_some();
        // Shooting roots a unit: stand still while the gun holds. Styles pressing forward walk on past an
        // enemy out of effective range (a far-off one must not pin the team down until the clock runs out);
        // the others also stop for any enemy in effective range.
        let effective = self.gun.weapon(me).falloff_start;
        let pressing = self.pressing(info);
        let hold = shot.is_some_and(|s| {
            let close = s.distance <= effective;
            if pressing {
                s.hold && close
            } else {
                s.hold || close
            }
        });
        let used_hands =
            if attacking { self.attack(me, info, fighting, hold, pace) } else { self.defend(me, info, fighting, hold) };
        if used_hands {
            return;
        }
        let scene = self.scene(me, info, shot.map(|s| s.enemy));
        if self.kit.cast(me, info, &scene) {
            return;
        }
        if !fighting {
            self.gun.top_up(me);
        }
    }

    /// Keep moving through a fight: an execute on its way in, a forward defender before the plant.
    fn pressing(&self, info: &MatchInfo) -> bool {
        if info.phase != PHASE_ACTION {
            return false;
        }
        if info.my_side == SIDE_ATTACK {
            self.style.attack == Attack::Execute
        } else {
            self.forward
        }
    }

    fn pace(&mut self, me: &UnitInfo, info: &MatchInfo) -> i32 {
        let (walk, run) = (self.core.walk_speed, self.core.run_speed);
        if info.my_side != SIDE_ATTACK || info.phase == PHASE_PLANTED {
            return run;
        }
        match self.style.attack {
            Attack::Execute => run,
            Attack::Split if self.detour.is_some() => walk,
            Attack::Split => run,
            Attack::Slow => {
                // Walk (silent) while there is time to walk there and plant; otherwise run.
                let walk_ticks = self.far_len(me, self.site) / walk.max(1);
                if walk_ticks + self.core.plant_ticks + RUN_MARGIN_TICKS > info.phase_ticks_left {
                    run
                } else {
                    walk
                }
            }
        }
    }

    /// Remember enemy sounds: one place to go and look, and when a defuse was last heard. The place is kept
    /// until it is reached or goes stale; chasing every new sound would start a new path search each tick.
    fn listen(&mut self, me: &UnitInfo, info: &MatchInfo) {
        if matches!(self.heard, Some((_, at)) if info.tick - at > SOUND_TTL) {
            self.heard = None;
        }
        for s in sounds().filter(|s| s.team != me.team && s.kind != SOUND_DECOY) {
            if s.kind == SOUND_DEFUSE {
                self.defuse_heard = info.tick;
            }
            self.heard.get_or_insert(((s.x, s.y), info.tick));
        }
    }

    fn defuse_alert(&self, info: &MatchInfo) -> bool {
        info.tick - self.defuse_heard <= DEFUSE_ALERT_TICKS
    }

    fn scene(&self, me: &UnitInfo, info: &MatchInfo, enemy: Option<UnitInfo>) -> Scene {
        let attacking = info.my_side == SIDE_ATTACK;
        let planted = info.phase == PHASE_PLANTED;
        let objective = if planted { (info.charge_x, info.charge_y) } else { self.centre(self.site) };
        let near = dist(me.pos(), objective);
        let channel_soon = if planted {
            !attacking && near <= 2 * TILE && self.nearest_to(me, objective)
        } else {
            attacking && me.has(S_HAS_CHARGE) && near <= 2 * TILE
        };
        let other = (self.site + 1) % self.sites.len();
        Scene {
            attacking,
            planted,
            enemy,
            objective,
            watch: self.watch,
            sighting: self.sighting(info),
            other_site: (other != self.site).then(|| self.centre(other)),
            defuse_heard: self.defuse_alert(info),
            opened: planted || self.core.round_ticks - info.phase_ticks_left >= 6,
            channel_soon,
        }
    }

    /// Returns true if the tick's one "hands" action (plant, pickup) was used.
    fn attack(&mut self, me: &UnitInfo, info: &MatchInfo, fighting: bool, hold: bool, pace: i32) -> bool {
        let anchor = self.centre(self.site);
        let charge = (info.charge_x, info.charge_y);
        let mut hold = hold;
        let goal = if info.phase == PHASE_PLANTED {
            if self.defuse_alert(info) {
                hold = false; // a defuse in progress loses the round: go and stop it
                charge
            } else {
                self.post
            }
        } else if me.has(S_HAS_CHARGE) {
            if dist(me.pos(), anchor) <= TILE / 4 {
                // Plant when it is quiet, or when a teammate is close enough to cover it.
                return (!fighting || self.allies_within(me, COVER) > 0) && plant().is_ok();
            }
            anchor
        } else if info.charge_state == CHARGE_DROPPED && self.nearest_to(me, charge) {
            if dist(me.pos(), charge) <= self.core.pickup_reach * REACH_PCT / 100 {
                return !fighting && pickup().is_ok();
            }
            charge
        } else if let Some(via) = self.detour {
            // The long way round ends at the other site, or at half time.
            if dist(me.pos(), via) <= 2 * TILE || info.phase_ticks_left * 2 < self.core.round_ticks {
                self.detour = None;
            }
            via
        } else {
            self.post
        };
        self.travel(me, goal, self.sighting(info), fighting, hold, pace);
        false
    }

    /// Returns true if the tick's one "hands" action (defuse) was used.
    fn defend(&mut self, me: &UnitInfo, info: &MatchInfo, fighting: bool, hold: bool) -> bool {
        if info.phase == PHASE_PLANTED {
            return self.retake(me, info, fighting, hold);
        }
        let sighting = self.sighting(info);
        // Spare defenders rotate to the site nearest a fresh sighting.
        if let (Some(seen), true) = (sighting, self.spare) {
            let hot = (0..self.sites.len()).min_by_key(|&i| dist2(self.centre(i), seen)).unwrap_or(self.site);
            if hot != self.site {
                self.site = hot;
                self.stand();
            }
        }
        // A forward defender goes to look at what it heard.
        let goal = match self.heard {
            Some((at, _)) if self.forward => at,
            _ => self.post,
        };
        if self.travel(me, goal, sighting, fighting, hold, self.core.run_speed) && goal != self.post {
            self.heard = None;
        }
        false
    }

    /// After the plant: the defender nearest the charge defuses, the rest guard it.
    fn retake(&mut self, me: &UnitInfo, info: &MatchInfo, fighting: bool, hold: bool) -> bool {
        let charge = (info.charge_x, info.charge_y);
        let run = self.core.run_speed;
        let direct = dist(me.pos(), charge);
        // The charge lies on a site, so the distance to that site's centre stands in for the distance to it.
        let planted_on = (0..self.sites.len()).min_by_key(|&i| dist2(self.centre(i), charge)).unwrap_or(self.site);
        let path = self.far_len(me, planted_on).max(direct);
        let defuse_ticks = if me.has(S_HAS_KIT) { self.core.defuse_kit_ticks } else { self.core.defuse_ticks };
        // The fuse leaves no time for anything but going in.
        let forced = info.fuse_left <= path / run.max(1) + defuse_ticks + RETAKE_MARGIN_TICKS;
        let covered = self.allies_within(me, COVER) > 0;
        let defuser = self.nearest_to(me, charge);
        if defuser && direct <= self.core.interact_reach * REACH_PCT / 100 {
            return (!fighting || forced || covered) && defuse().is_ok();
        }
        // A retake team waits short of the charge until someone is with it.
        let waits = self.style.defence == Defence::Retake && info.allies_alive > 1 && !covered;
        if waits && !forced && path <= STAGE {
            if !fighting {
                let _ = face(angle_to(charge.0 - me.x, charge.1 - me.y));
            }
            return false;
        }
        let goal = if defuser { charge } else { self.guard_spot(charge, info) };
        self.travel(me, goal, self.sighting(info), fighting, hold && !forced, run);
        false
    }

    /// Go to `goal` unless the gun wants us still. Look where we walk; once there, at a reported enemy
    /// if it could be seen from here, else at the usual approach. Returns true on arrival.
    fn travel(
        &mut self,
        me: &UnitInfo,
        goal: (i32, i32),
        sighting: Option<(i32, i32)>,
        fighting: bool,
        hold: bool,
        speed: i32,
    ) -> bool {
        if hold {
            return false;
        }
        let arrived = self.nav.go(me, goal.0, goal.1, speed).unwrap_or(false);
        if !fighting {
            let look = match sighting {
                _ if !arrived => self.nav.heading,
                Some(seen) if los(me.pos(), seen) => angle_to(seen.0 - me.x, seen.1 - me.y),
                _ => angle_to(self.watch.0 - me.x, self.watch.1 - me.y),
            };
            let _ = face(look);
        }
        arrived
    }

    /// This unit's place in the ring around `centre`: a floor tile that can see the centre, else the centre.
    fn guard_spot(&self, centre: (i32, i32), info: &MatchInfo) -> (i32, i32) {
        let angle = info.unit_slot * FULL_TURN / info.team_size.max(1);
        let p = offset(centre.0, centre.1, angle, GUARD_RING);
        let (tx, ty) = tile_of(p.0, p.1);
        let spot = tile_centre(tx, ty);
        if !self.nav.grid.is_blocked(tx, ty) && los(spot, centre) {
            spot
        } else {
            centre
        }
    }
}
