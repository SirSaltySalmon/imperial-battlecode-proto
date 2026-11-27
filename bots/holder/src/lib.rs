//! Holder: the patient baseline.
//!
//! Defence: units split across the sites. Each picks a post near its site with a long sightline onto
//! the point where attackers come into view, stands still there (standing shots are accurate) and
//! shoots what appears. Sightings are shared through team memory; spare defenders rotate towards them.
//! When the charge is planted everyone converges on it and defuses.
//! Attack: the team agrees on a site through team memory, walks there silently (running only when the
//! clock demands it), plants, then holds posts around the charge facing the defenders' approach.
//!
//! Fuel: the expensive questions (where does the enemy come into view of a site, how far is it) are asked
//! one per buy tick, as the last thing in the turn, and their answers are kept for the match. Until its
//! post is known a unit stands on its site; nothing in the action phase runs an unbounded search.
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
/// A distance field to a site's plant point is grown in this many survey steps.
const FIELD_STEPS: u8 = 3;

// Team memory layout (little-endian i32s).
/// [tick + 1, x, y] of the last enemy any teammate saw.
const MEM_SIGHT: u32 = 0;
/// [round + 1, site index] the attackers go for this round, written by the first of them to act in the action
/// phase (see `agree_site`).
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
    /// This unit's place among the teammates at its site (picks which post it takes).
    rank: usize,
    /// An attacker that has not yet checked its site against team memory this round.
    agree_pending: bool,
    /// Where this unit stands, and the point it watches from there.
    post: (i32, i32),
    watch: (i32, i32),
    /// Per site, the post and watch point worked out so far for the side in `posts_side`.
    posts: Vec<Option<Post>>,
    posts_side: i32,
    /// Distance fields to the sites' plant points, kept apart from the navigator's so walking never evicts them.
    far: Pathfinder,
    /// Per site: how many of the steps that build its distance field in `far` are done.
    fielded: Vec<u8>,
}

/// Where a unit stands at a site, and the point it watches from there.
#[derive(Clone, Copy)]
struct Post {
    post: (i32, i32),
    watch: (i32, i32),
}

impl Bot for Holder {
    fn new() -> Self {
        let core = core_rules();
        let sites: Vec<SiteInfo> = sites().collect();
        Holder {
            core,
            map: map_info(),
            nav: Navigator::new(),
            gun: Gunner::new(),
            fielded: vec![0; sites.len()],
            sites,
            plan_for: (-1, -1),
            site: 0,
            rank: 0,
            agree_pending: false,
            post: (0, 0),
            watch: (0, 0),
            posts: Vec::new(),
            posts_side: -1,
            far: Pathfinder::new(),
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
            self.plan(&info);
        }
        match info.phase {
            PHASE_BUY => {
                shop(&me);
                // Last in the turn: if it runs out of fuel, nothing after it is lost.
                self.survey(&info);
            }
            PHASE_ACTION | PHASE_PLANTED => {
                if self.agree_pending {
                    self.agree_site(&info);
                }
                self.fight(&me, &info)
            }
            _ => {}
        }
    }
}

impl Holder {
    /// Where this team plants on `site`, and the point its posts are picked around.
    fn plant_point(&self, site: usize) -> (i32, i32) {
        self.sites[site].plant_point()
    }

    /// Once per round, cheap: choose a site and this unit's rank there, and take the post known for it.
    fn plan(&mut self, info: &MatchInfo) {
        self.plan_for = (info.round, info.my_side);
        self.map = map_info(); // spawns swap with the sides
        let n = self.sites.len();
        if self.posts_side != info.my_side {
            // The enemy comes from the other spawn now: every post is stale.
            self.posts_side = info.my_side;
            self.posts = vec![None; n];
        }
        let slot = info.unit_slot.max(0) as usize;
        self.agree_pending = info.my_side == SIDE_ATTACK;
        self.rank = if self.agree_pending {
            // Team memory is zeroed at round start and read-only until the action phase, so every attacker
            // picks here, all of them the same way; `agree_site` shares the pick once memory is writable.
            self.site = (info.seed_lo.unsigned_abs() as usize + info.round.max(0) as usize) % n;
            slot
        } else {
            self.site = slot % n;
            slot / n
        };
        self.stand();
    }

    /// On an attacker's first action tick, the first phase in which team memory is writable: the first
    /// attacker to act this round writes its site, and the others go to the site written there.
    fn agree_site(&mut self, info: &MatchInfo) {
        self.agree_pending = false;
        let [stamp, chosen] = team_mem_get::<2>(MEM_SITE);
        if stamp != info.round + 1 {
            let _ = team_mem_set(MEM_SITE, [info.round + 1, self.site as i32]);
            return;
        }
        let site = chosen.max(0) as usize % self.sites.len();
        if site != self.site {
            self.site = site;
            self.stand();
        }
    }

    /// Take the post worked out for the current site. Until it is known, stand on the site.
    fn stand(&mut self) {
        let point = self.plant_point(self.site);
        let p = self.posts[self.site].unwrap_or(Post { post: point, watch: point });
        (self.post, self.watch) = (p.post, p.watch);
    }

    /// The expensive part, one step per call: a post for a site that has none yet (the site this unit
    /// plays first), then a distance field to a site's plant point. Each answer is kept, so after the first
    /// rounds of each side this does nothing.
    fn survey(&mut self, info: &MatchInfo) {
        // A step can outlast its tick on a large map. Never start one on the last buy tick, so the
        // overrun costs a buy tick (the shopping is already done) and not the first step of the round.
        if info.phase_ticks_left < 2 {
            return;
        }
        let n = self.sites.len();
        let own_first = (0..n).map(|k| (self.site + k) % n);
        if let Some(i) = own_first.clone().find(|&i| self.posts[i].is_none()) {
            self.posts[i] = Some(self.scout(i));
            self.stand();
        } else if let Some(i) = own_first.clone().find(|&i| self.fielded[i] < FIELD_STEPS) {
            // Grow the field over the whole map, so every later question is a lookup. The field grows
            // outwards from the site, so asking for the nearest points first splits the work evenly.
            let point = self.plant_point(i);
            let mut points = self.far_points();
            points.sort_by_key(|&p| dist2(p, point));
            let share = points.len() / FIELD_STEPS as usize;
            let from = self.fielded[i] as usize * share;
            for &p in &points[from..from + share] {
                let _ = self.far.path_len(&self.nav.grid, p, point);
            }
            self.fielded[i] += 1;
        }
    }

    /// Near `site`, a spot with a long line onto the point where the enemy comes into view.
    fn scout(&mut self, site: usize) -> Post {
        let anchor = self.plant_point(site);
        let watch = approach_point(&mut self.nav, anchor, (self.map.enemy_spawn_x, self.map.enemy_spawn_y));
        Post { post: hold_spot(&self.nav, anchor, watch, POST_RADIUS_TILES, self.rank), watch }
    }

    /// Both spawns and the standable tile nearest each map corner (six points): a distance field that
    /// reaches all of these covers the map.
    fn far_points(&self) -> Vec<(i32, i32)> {
        let grid = &self.nav.grid;
        let (w, h) = (grid.width, grid.height);
        let spawns = [(self.map.spawn_x, self.map.spawn_y), (self.map.enemy_spawn_x, self.map.enemy_spawn_y)];
        // Walk outwards from the corner along diagonals until a tile can be stood on (a few steps: the
        // border is wall).
        let corner = |right: bool, bottom: bool| {
            let at = |d: i32, k: i32| (if right { w - 1 - k } else { k }, if bottom { h - 1 - (d - k) } else { d - k });
            let mut diagonals = (0..w + h).flat_map(|d| (0..=d).map(move |k| at(d, k)));
            let open = diagonals.find(|&(tx, ty)| tx >= 0 && ty >= 0 && tx < w && ty < h && !grid.is_blocked(tx, ty));
            open.map_or(spawns[0], |(tx, ty)| tile_centre(tx, ty))
        };
        vec![spawns[0], spawns[1], corner(false, false), corner(true, false), corner(false, true), corner(true, true)]
    }

    /// Path length from me to the plant point of `site`: exact once its field is built, else a guess from the
    /// straight line.
    fn far_len(&mut self, me: &UnitInfo, site: usize) -> i32 {
        let point = self.plant_point(site);
        let straight = dist(me.pos(), point);
        if self.fielded[site] < FIELD_STEPS {
            return straight * 3 / 2;
        }
        self.far.path_len(&self.nav.grid, me.pos(), point).unwrap_or(straight)
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
        let walk_ticks = self.far_len(me, self.site) / self.core.walk_speed.max(1);
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
        if let (Some(seen), true) = (sighting, self.rank > 0) {
            let hot = (0..self.sites.len()).min_by_key(|&i| dist2(self.plant_point(i), seen)).unwrap_or(self.site);
            if hot != self.site {
                self.site = hot;
                self.stand();
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
