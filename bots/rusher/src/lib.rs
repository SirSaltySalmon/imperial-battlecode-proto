//! Rusher: the aggressive baseline.
//!
//! Attack: everyone runs to the site nearest their spawn. The carrier plants on arrival, the rest
//! stand in a ring around the site (then around the planted charge) watching where defenders appear.
//! Defence: run at an enemy sound until there or it goes stale, else at the attackers' spawn, then sweep
//! the sites;
//! once the charge is planted, run to it and defuse.
//! Always: shoot the nearest visible enemy, leading it; reload when it is quiet.
//!
//! Fuel: the site to rush is the same every round of a side, so the path searches that find it are
//! made once per side, one per buy tick as the last thing in the turn, and kept. A defender chases one
//! sound at a time, so the action phase starts a new search only when the place to go changes.
use ibc::tactics::{approach_point, shop, Gunner};
use ibc::*;

/// Guards stand this far from the site's plant point / planted charge.
const GUARD_RING: i32 = 1400;
/// A heard enemy position is chased for this many ticks.
const SOUND_MEMORY_TICKS: i32 = 12;
/// Keep channelling a plant or defuse under fire when this few ticks remain.
const FINISH_CHANNEL_TICKS: i32 = 2;
/// Interact from well inside the allowed reach, so rounding never puts us outside it.
const REACH_PCT: i32 = 70;
/// `MatchInfo::charge_state` values.
const CHARGE_DROPPED: i32 = 1;

pub struct Rusher {
    core: CoreRules,
    map: MapInfo,
    nav: Navigator,
    gun: Gunner,
    /// (round, side) the plan below was made for.
    plan_for: (i32, i32),
    /// Attack: plant point of the site we rush.
    site: (i32, i32),
    /// Attack: the point where defenders come into view of the site.
    watch: (i32, i32),
    /// Defence: last heard enemy position and the tick it was heard.
    heard: Option<((i32, i32), i32)>,
    /// Defence: index into the sweep route (attacker spawn, then each site).
    sweep: usize,
    /// The map's sites, in the order the engine lists them.
    sites: Vec<SiteInfo>,
    /// Side the two answers below were worked out for (the spawns swap with the sides).
    known_side: i32,
    /// Per site, the path length from our spawn to its plant point once it has been searched.
    reach: Vec<Option<i32>>,
    /// The site to rush and its watch point, once every site's distance is known.
    target: Option<((i32, i32), (i32, i32))>,
}

impl Bot for Rusher {
    fn new() -> Self {
        let core = core_rules();
        Rusher {
            core,
            map: map_info(),
            nav: Navigator::new(),
            gun: Gunner::new(),
            plan_for: (-1, -1),
            site: (0, 0),
            watch: (0, 0),
            heard: None,
            sweep: 0,
            sites: sites().collect(),
            known_side: -1,
            reach: Vec::new(),
            target: None,
        }
    }

    fn turn(&mut self) {
        let me = me();
        if !me.alive() {
            return;
        }
        let info = match_info();
        if info.phase == PHASE_DRAFT {
            return; // sides and spawns are not set yet
        }
        if self.plan_for != (info.round, info.my_side) {
            self.plan(&info);
        }
        match info.phase {
            PHASE_BUY => {
                shop(&me);
                // Last in the turn: if it runs out of fuel, nothing after it is lost.
                self.survey(&info);
            }
            PHASE_ACTION | PHASE_PLANTED => self.fight(&me, &info),
            _ => {}
        }
    }
}

impl Rusher {
    /// Once per round, cheap: reset the round's memory and take the site worked out for this side.
    fn plan(&mut self, info: &MatchInfo) {
        self.plan_for = (info.round, info.my_side);
        self.map = map_info(); // spawns swap with the sides
        self.heard = None;
        self.sweep = 0;
        if self.known_side != info.my_side {
            self.known_side = info.my_side;
            self.reach = vec![None; self.sites.len()];
            self.target = None;
        }
        // Until the survey has found the site (it takes a few buy ticks once per side), the first one.
        let first = self.sites.first().map(|s| (s.plant_point(), s.plant_point()));
        if let Some((site, watch)) = self.target.or(first) {
            (self.site, self.watch) = (site, watch);
        }
    }

    /// One path search per call until the site to rush is known: the distance from our spawn to each
    /// site, then the point where defenders come into view of the nearest one.
    fn survey(&mut self, info: &MatchInfo) {
        // A search can outlast its tick on a large map. Never start one on the last buy tick, so the
        // overrun costs a buy tick (the shopping is already done) and not the first tick of the round.
        if info.phase_ticks_left < 2 || self.target.is_some() || self.sites.is_empty() {
            return;
        }
        let spawn = (self.map.spawn_x, self.map.spawn_y);
        if let Some(i) = self.reach.iter().position(Option::is_none) {
            self.reach[i] = Some(self.nav.path_len(spawn, self.sites[i].plant_point()).unwrap_or(i32::MAX));
            return;
        }
        // The first of the nearest, as `min_by_key` picks it.
        let nearest = (0..self.sites.len()).min_by_key(|&i| self.reach[i]).unwrap_or(0);
        let site = self.sites[nearest].plant_point();
        let watch = approach_point(&mut self.nav, site, (self.map.enemy_spawn_x, self.map.enemy_spawn_y));
        self.target = Some((site, watch));
        (self.site, self.watch) = (site, watch);
    }

    fn fight(&mut self, me: &UnitInfo, info: &MatchInfo) {
        // A plant or defuse that is nearly done is worth more than one more shot.
        if me.has(S_PLANTING | S_DEFUSING) && me.channel <= FINISH_CHANNEL_TICKS {
            let _ = if me.has(S_PLANTING) { plant() } else { defuse() };
            return;
        }
        let speed = self.core.run_speed;
        let shot = self.gun.engage(me);
        let (fighting, hold) = (shot.is_some(), shot.is_some_and(|s| s.hold));
        let used_hands = if info.my_side == SIDE_ATTACK {
            self.attack(me, info, fighting, hold, speed)
        } else {
            self.defend(me, info, fighting, hold, speed)
        };
        if !fighting && !used_hands {
            self.gun.top_up(me);
        }
    }

    /// Returns true if the tick's one "hands" action (plant, pickup) was used.
    fn attack(&mut self, me: &UnitInfo, info: &MatchInfo, fighting: bool, hold: bool, speed: i32) -> bool {
        let charge = (info.charge_x, info.charge_y);
        let reach = self.core.pickup_reach * REACH_PCT / 100;
        let goal = if info.phase == PHASE_PLANTED {
            self.guard_spot(charge, info)
        } else if me.has(S_HAS_CHARGE) {
            if dist(me.pos(), self.site) <= TILE / 4 {
                // Standing on the site: plant unless someone is shooting at us.
                return !fighting && plant().is_ok();
            }
            self.site
        } else if info.charge_state == CHARGE_DROPPED && self.nearest_to(me, charge) {
            if dist(me.pos(), charge) <= reach {
                return !fighting && pickup().is_ok();
            }
            charge
        } else {
            self.guard_spot(self.site, info)
        };
        self.travel(me, goal, self.watch, fighting, hold, speed);
        false
    }

    /// Returns true if the tick's one "hands" action (defuse) was used.
    fn defend(&mut self, me: &UnitInfo, info: &MatchInfo, fighting: bool, hold: bool, speed: i32) -> bool {
        // One place to go at a time, kept until it is reached or goes stale: chasing every new sound would
        // start a new path search each tick, and one search can take a whole tick's fuel.
        if matches!(self.heard, Some((_, at)) if info.tick - at > SOUND_MEMORY_TICKS) {
            self.heard = None;
        }
        for s in sounds().filter(|s| s.team != me.team && s.kind != SOUND_DECOY) {
            self.heard.get_or_insert(((s.x, s.y), info.tick));
        }
        let charge = (info.charge_x, info.charge_y);
        let goal = if info.phase == PHASE_PLANTED {
            if dist(me.pos(), charge) <= self.core.interact_reach * REACH_PCT / 100 {
                return !fighting && defuse().is_ok();
            }
            charge
        } else if let Some((at, _)) = self.heard {
            at
        } else {
            self.sweep_target()
        };
        if self.travel(me, goal, goal, fighting, hold, speed) && info.phase != PHASE_PLANTED {
            // Nothing here: forget the sound, move on to the next sweep point.
            if self.heard.take().is_none() {
                self.sweep += 1;
            }
        }
        false
    }

    /// Run to `goal` (unless the gun wants us still); look along the way, then at `watch`. True on arrival.
    fn travel(&mut self, me: &UnitInfo, goal: (i32, i32), watch: (i32, i32), fighting: bool, hold: bool, speed: i32) -> bool {
        if hold {
            return false;
        }
        let arrived = self.nav.go(me, goal.0, goal.1, speed).unwrap_or(false);
        if !fighting {
            let look = if arrived { angle_to(watch.0 - me.x, watch.1 - me.y) } else { self.nav.heading };
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

    /// Am I the living teammate closest to `p`? (Ties go to the lower id.)
    fn nearest_to(&self, me: &UnitInfo, p: (i32, i32)) -> bool {
        let mine = (dist2(me.pos(), p), me.id);
        allies().filter(|a| a.alive()).all(|a| (dist2(a.pos(), p), a.id) > mine)
    }

    /// Defence sweep route: the attackers' spawn first, then every site in turn.
    fn sweep_target(&self) -> (i32, i32) {
        let stops = self.map.sites.max(0) as usize + 1;
        match self.sweep % stops {
            0 => (self.map.enemy_spawn_x, self.map.enemy_spawn_y),
            i => sites().nth(i - 1).map_or(self.site, |s| s.plant_point()),
        }
    }
}
