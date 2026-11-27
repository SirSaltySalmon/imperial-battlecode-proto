//! Holder: the patient baseline.
//!
//! Defence: units split across the sites. Each picks a post near its site with a long sightline onto
//! the point where attackers come into view, stands still there (standing shots are accurate) and
//! shoots what appears. Sightings are shared through team memory; spare defenders rotate towards them.
//! When the charge is planted everyone converges on it and defuses.
//! Attack: the team agrees on a site through team memory, walks there silently (running only when the
//! clock demands it), plants, then holds posts around the charge facing the defenders' approach.
use ibc::tactics::{approach_point, hold_spot, shop, Gunner};
use ibc::*;

/// Posts are picked within this many tiles of the site's plant point.
const POST_RADIUS_TILES: i32 = 3;
/// A shared enemy sighting is trusted for this many ticks.
const SIGHT_TTL: i32 = 15;
/// Attackers start running when the walk would leave fewer spare ticks than this.
const RUN_MARGIN_TICKS: i32 = 15;
/// Keep channelling a plant or defuse under fire when this few ticks remain.
const FINISH_CHANNEL_TICKS: i32 = 2;
/// Interact from well inside the allowed reach, so rounding never puts us outside it.
const REACH_PCT: i32 = 70;
/// `MatchInfo::charge_state` values.
const CHARGE_DROPPED: i32 = 1;

// Team memory layout (little-endian i32s).
/// [tick + 1, x, y] of the last enemy any teammate saw.
const MEM_SIGHT: u32 = 0;
/// [round + 1, site index] chosen by the attackers this round.
const MEM_SITE: u32 = 12;

pub struct Holder {
    core: CoreRules,
    map: MapInfo,
    nav: Navigator,
    gun: Gunner,
    sites: Vec<SiteInfo>,
    /// (round, side) the plan below was made for.
    plan_for: (i32, i32),
    /// Index of the site this unit defends or attacks.
    site: usize,
    /// Where this unit stands, and the point it watches from there.
    post: (i32, i32),
    watch: (i32, i32),
}

impl Bot for Holder {
    fn new() -> Self {
        let core = core_rules();
        Holder {
            core,
            map: map_info(),
            nav: Navigator::new(),
            gun: Gunner::new(),
            sites: sites().collect(),
            plan_for: (-1, -1),
            site: 0,
            post: (0, 0),
            watch: (0, 0),
        }
    }

    fn turn(&mut self) {
        let me = me();
        if !me.alive() || self.sites.is_empty() {
            return;
        }
        let info = match_info();
        if info.phase == PHASE_DRAFT {
            return; // sides and spawns are not set yet
        }
        if self.plan_for != (info.round, info.my_side) {
            self.plan(&me, &info); // lands on the first buy tick, where the fuel budget is larger
        }
        match info.phase {
            PHASE_BUY => {
                shop(&me);
            }
            PHASE_ACTION | PHASE_PLANTED => self.fight(&me, &info),
            _ => {}
        }
    }
}

impl Holder {
    /// Where this team plants on `site`, and the point its posts are picked around.
    fn plant_point(&self, site: usize) -> (i32, i32) {
        self.sites[site].plant_point()
    }

    /// Once per round: choose a site, a post near it and the point to watch.
    fn plan(&mut self, me: &UnitInfo, info: &MatchInfo) {
        self.plan_for = (info.round, info.my_side);
        self.map = map_info(); // spawns swap with the sides
        let n = self.sites.len();
        let slot = info.unit_slot.max(0) as usize;
        let rank = if info.my_side == SIDE_ATTACK {
            // The first attacker to run this round picks the site; the rest read it.
            let [stamp, chosen] = team_mem_get::<2>(MEM_SITE);
            self.site = if stamp == info.round + 1 {
                chosen.max(0) as usize % n
            } else {
                let pick = (info.seed_lo.unsigned_abs() as usize + info.round.max(0) as usize) % n;
                let _ = team_mem_set(MEM_SITE, [info.round + 1, pick as i32]);
                pick
            };
            slot
        } else {
            self.site = slot % n;
            slot / n
        };
        self.take_post(self.site, rank);
        // Do the path search to the site now, so the per-tick `path_len` in `attack` is a lookup.
        let _ = self.nav.path_len(me.pos(), self.plant_point(self.site));
    }

    fn take_post(&mut self, site: usize, rank: usize) {
        let anchor = self.plant_point(site);
        self.watch = approach_point(&mut self.nav, anchor, (self.map.enemy_spawn_x, self.map.enemy_spawn_y));
        self.post = hold_spot(&self.nav, anchor, self.watch, POST_RADIUS_TILES, rank);
    }

    fn fight(&mut self, me: &UnitInfo, info: &MatchInfo) {
        // A plant or defuse that is nearly done is worth more than one more shot.
        if me.has(S_PLANTING | S_DEFUSING) && me.channel <= FINISH_CHANNEL_TICKS {
            let _ = if me.has(S_PLANTING) { plant() } else { defuse() };
            return;
        }
        // Shooting roots a unit. Attackers on their way in walk on past far-off enemies (which must not pin the
        // attack down until the clock runs out); everyone else stands and fights.
        let advancing = info.my_side == SIDE_ATTACK && info.phase == PHASE_ACTION;
        let mut fighting = false;
        if let Some(shot) = self.gun.engage(me) {
            let _ = team_mem_set(MEM_SIGHT, [info.tick + 1, shot.enemy.x, shot.enemy.y]);
            if !advancing || shot.distance <= self.gun.weapon(me).falloff_start {
                return;
            }
            fighting = true;
        }
        let used_hands = if info.my_side == SIDE_ATTACK {
            self.attack(me, info, fighting)
        } else {
            self.defend(me, info)
        };
        if !used_hands && !fighting {
            self.gun.top_up(me);
        }
    }

    /// The last enemy position a teammate reported, if it is recent.
    fn sighting(&self, info: &MatchInfo) -> Option<(i32, i32)> {
        let [stamp, x, y] = team_mem_get::<3>(MEM_SIGHT);
        (stamp > 0 && info.tick + 1 - stamp <= SIGHT_TTL).then_some((x, y))
    }

    /// Returns true if the tick's one "hands" action (plant, pickup) was used.
    fn attack(&mut self, me: &UnitInfo, info: &MatchInfo, fighting: bool) -> bool {
        let anchor = self.plant_point(self.site);
        let charge = (info.charge_x, info.charge_y);
        let goal = if info.phase == PHASE_PLANTED {
            self.post
        } else if me.has(S_HAS_CHARGE) {
            if dist(me.pos(), anchor) <= TILE / 4 {
                return plant().is_ok();
            }
            anchor
        } else if info.charge_state == CHARGE_DROPPED && self.nearest_to(me, charge) {
            if dist(me.pos(), charge) <= self.core.pickup_reach * REACH_PCT / 100 {
                return pickup().is_ok();
            }
            charge
        } else {
            self.post
        };
        // Walk (silent) while there is time to walk there and plant; otherwise run.
        let walk_ticks = self.nav.path_len(me.pos(), anchor).unwrap_or(0) / self.core.walk_speed.max(1);
        let hurry = info.phase == PHASE_ACTION
            && walk_ticks + self.core.plant_ticks + RUN_MARGIN_TICKS > info.phase_ticks_left;
        let speed = if hurry { self.core.run_speed } else { self.core.walk_speed };
        self.travel(me, goal, self.sighting(info), speed, fighting);
        false
    }

    /// Returns true if the tick's one "hands" action (defuse) was used.
    fn defend(&mut self, me: &UnitInfo, info: &MatchInfo) -> bool {
        let sighting = self.sighting(info);
        if info.phase == PHASE_PLANTED {
            let charge = (info.charge_x, info.charge_y);
            if dist(me.pos(), charge) <= self.core.interact_reach * REACH_PCT / 100 {
                return defuse().is_ok();
            }
            self.travel(me, charge, None, self.core.run_speed, false);
            return false;
        }
        // Spare defenders (not the first at their site) rotate to the site nearest a fresh sighting.
        let rank = info.unit_slot.max(0) as usize / self.sites.len();
        if let (Some(seen), true) = (sighting, rank > 0) {
            let hot = (0..self.sites.len()).min_by_key(|&i| dist2(self.plant_point(i), seen)).unwrap_or(self.site);
            if hot != self.site {
                self.site = hot;
                self.take_post(hot, rank);
            }
        }
        self.travel(me, self.post, sighting, self.core.run_speed, false);
        false
    }

    /// Go to `goal`, looking where we walk; once there, watch the reported enemy if it could be seen
    /// from here, else the usual approach. While `fighting` the gun already chose where to look.
    fn travel(&mut self, me: &UnitInfo, goal: (i32, i32), sighting: Option<(i32, i32)>, speed: i32, fighting: bool) {
        let arrived = self.nav.go(me, goal.0, goal.1, speed).unwrap_or(false);
        if fighting {
            return;
        }
        let look = match sighting {
            _ if !arrived => self.nav.heading,
            Some(seen) if los(me.pos(), seen) => angle_to(seen.0 - me.x, seen.1 - me.y),
            _ => angle_to(self.watch.0 - me.x, self.watch.1 - me.y),
        };
        let _ = face(look);
    }

    /// Am I the living teammate closest to `p`? (Ties go to the lower id.)
    fn nearest_to(&self, me: &UnitInfo, p: (i32, i32)) -> bool {
        let mine = (dist2(me.pos(), p), me.id);
        allies().filter(|a| a.alive()).all(|a| (dist2(a.pos(), p), a.id) > mine)
    }
}
