//! Once per round: which site and which job. Once per side: where to stand at each site.
use crate::{Attack, Defence, Squad, LONG_AGO};
use ibc::tactics::{approach_point, hold_spot};
use ibc::*;

/// Posts are picked within this many tiles of the site's plant point.
const POST_RADIUS_TILES: i32 = 3;
/// How many attackers take the long way round in a split.
const LURKERS: i32 = 2;
/// A distance field is grown in this many survey steps.
pub(crate) const FIELD_STEPS: u8 = 3;

/// Where a unit stands at a site, and the point it watches from there.
#[derive(Clone, Copy)]
pub(crate) struct Post {
    pub post: (i32, i32),
    pub watch: (i32, i32),
}

/// A number every unit of a team agrees on, that changes with the seed, the round and `salt`.
pub(crate) fn roll(info: &MatchInfo, salt: u32) -> usize {
    let mut x = (info.seed_lo as u32)
        ^ (info.round as u32).wrapping_mul(0x9E37_79B9)
        ^ (salt + info.my_team as u32 * 31).wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x as usize
}

impl Squad {
    /// The cheap part of the round plan: site, job and flags. Runs on the first tick of the round.
    pub(crate) fn assign(&mut self, me: &UnitInfo, info: &MatchInfo) {
        self.plan_for = (info.round, info.my_side);
        self.map = map_info(); // spawns swap with the sides
        self.kit.load(me);
        self.heard = None;
        self.defuse_heard = LONG_AGO;
        (self.detour, self.spare, self.forward) = (None, false, false);
        let n = self.sites.len();
        let slot = info.unit_slot.max(0) as usize;
        self.fielded.resize(n, 0);
        if self.posts_side != info.my_side {
            // The enemy comes from the other spawn now: every post is stale.
            self.posts_side = info.my_side;
            self.posts = vec![None; n];
        }
        self.rank = if info.my_side == SIDE_ATTACK {
            self.site = roll(info, 1) % n;
            // The charge starts on slot 0, so the last slots are the ones that can leave the group.
            let lurks = self.style.attack == Attack::Split && n > 1 && info.unit_slot >= info.team_size - LURKERS;
            if lurks {
                let other = (self.site + 1 + roll(info, 2) % (n - 1)) % n;
                self.detour = Some(self.plant_point(other));
            }
            slot
        } else {
            match self.style.defence {
                Defence::Retake if slot >= n => {
                    self.site = roll(info, 3) % n;
                    self.spare = true;
                    slot - n + 1
                }
                Defence::Retake => {
                    self.site = slot;
                    0
                }
                Defence::Hold | Defence::Push => {
                    self.site = slot % n;
                    self.spare = slot >= n;
                    self.forward = self.style.defence == Defence::Push && slot % 2 == 0;
                    slot / n
                }
            }
        };
        self.stand();
    }

    /// Take the post worked out for the current site. Until it is known, stand on the site.
    pub(crate) fn stand(&mut self) {
        let plant = self.plant_point(self.site);
        let p = self.posts[self.site].unwrap_or(Post { post: plant, watch: plant });
        (self.post, self.watch) = (p.post, p.watch);
    }

    /// The expensive part, one step per call: a post for a site that has none yet (the site this unit
    /// plays first), then a distance field to a plant point. Each answer is kept, so after the first
    /// rounds this does nothing.
    pub(crate) fn survey(&mut self, info: &MatchInfo) {
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
            let plant = self.plant_point(i);
            let mut points = self.far_points();
            points.sort_by_key(|&p| dist2(p, plant));
            let share = points.len() / FIELD_STEPS as usize;
            let from = self.fielded[i] as usize * share;
            for &p in &points[from..from + share] {
                let _ = self.far.path_len(&self.nav.grid, p, plant);
            }
            self.fielded[i] += 1;
        }
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

    /// Near `site`, a spot with a long line onto the point where the enemy comes into view. A forward
    /// defender stands on that point instead and watches the next one out.
    fn scout(&mut self, site: usize) -> Post {
        let anchor = self.plant_point(site);
        let origin = (self.map.enemy_spawn_x, self.map.enemy_spawn_y);
        let watch = approach_point(&mut self.nav, anchor, origin);
        if self.forward {
            Post { post: watch, watch: approach_point(&mut self.nav, watch, origin) }
        } else {
            Post { post: hold_spot(&self.nav, anchor, watch, POST_RADIUS_TILES, self.rank), watch }
        }
    }
}
