//! Squad: one team brain, several styles.
//!
//! The reference bots `sapper`, `retaker`, `lurker`, `saver` and `forcer` are this crate with a different
//! `Style`. They share everything a competent team needs (shooting, the plant, the defuse, kits and
//! abilities) and differ in the three decisions a style names: how to attack, how to defend, how to spend.
//!
//! Every unit runs its own copy. Decisions that the whole team must agree on (which site to hit, where to
//! stack) are a function of the match seed, the round and the team, so teammates reach the same answer
//! without talking, and a different seed gives a different game. Team memory carries the one thing that
//! cannot be computed: where an enemy was last seen.
//!
//! Fuel: the expensive questions (where does the enemy come into view of a site, how far is it) are asked
//! one per buy tick, as the last thing in the turn, and their answers are kept for the match. Nothing in
//! the action phase runs a whole-map search.
use ibc::tactics::Gunner;
use ibc::*;

mod buy;
mod fight;
mod kit;
mod plan;

/// How the team attacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attack {
    /// Everyone runs at the chosen site together and plants as soon as the carrier is covered.
    Execute,
    /// Walk in silently, running only when the clock demands it.
    Slow,
    /// Three run at the site; two walk the long way round past another site and arrive behind the rotation.
    Split,
}

/// How the team defends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Defence {
    /// Spread over the sites and hold angles; spare defenders rotate to a sighting.
    Hold,
    /// One anchor per site, the rest stacked on a site chosen per round. After a plant the team gathers
    /// short of the charge and goes in together.
    Retake,
    /// Half the team holds the chokes in front of the sites and hunts what it hears.
    Push,
}

/// How the team spends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Buy {
    /// The best weapon and armour it can afford, every round.
    Best,
    /// Full buy or nothing: save when the full loadout is out of reach, unless the round must be won.
    Eco,
    /// Cheap close-range weapons and light armour every round.
    Force,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub attack: Attack,
    pub defence: Defence,
    pub buy: Buy,
    /// Arm unit slots differently (rifles, a machine gun, a marksman rifle, a shotgun) instead of all alike.
    pub mixed_arms: bool,
}

/// Team memory: [tick + 1, x, y] of the last enemy any teammate saw.
const MEM_SIGHT: u32 = 0;
/// A shared sighting is trusted for this many ticks.
const SIGHT_TTL: i32 = 15;
/// A heard enemy position is chased for this many ticks.
const SOUND_TTL: i32 = 12;
/// "Never": a tick far enough in the past that no time-to-live covers it.
const LONG_AGO: i32 = i32::MIN / 2;

pub struct Squad {
    style: Style,
    core: CoreRules,
    map: MapInfo,
    nav: Navigator,
    gun: Gunner,
    sites: Vec<SiteInfo>,
    kit: kit::Kit,
    /// (round, side) the plan below was made for.
    plan_for: (i32, i32),
    /// Index of the site this unit attacks or defends.
    site: usize,
    /// This unit's place among the teammates at its site (picks which post it takes).
    rank: usize,
    /// Where this unit stands, and the point it watches from there.
    post: (i32, i32),
    watch: (i32, i32),
    /// Per site, the post and watch point worked out so far for the side in `posts_side`.
    posts: Vec<Option<plan::Post>>,
    posts_side: i32,
    /// Distance fields to the site centres, kept apart from the navigator's so walking never evicts them.
    far: Pathfinder,
    /// Per site: how many of the steps that build its distance field in `far` are done.
    fielded: Vec<u8>,
    /// Attack: a place to walk through first (the long way round of a split).
    detour: Option<(i32, i32)>,
    /// Defence: not the first defender of its site, so free to rotate.
    spare: bool,
    /// Defence: holds the choke in front of the site and hunts sounds.
    forward: bool,
    /// Last heard enemy position and its tick.
    heard: Option<((i32, i32), i32)>,
    /// Tick of the last defuse sound.
    defuse_heard: i32,
}

impl Squad {
    /// Call from `Bot::new`.
    pub fn new(style: Style) -> Squad {
        let core = core_rules();
        Squad {
            style,
            core,
            map: map_info(),
            nav: Navigator::new(),
            gun: Gunner::new(&core),
            sites: sites().collect(),
            kit: kit::Kit::default(),
            plan_for: (-1, -1),
            site: 0,
            rank: 0,
            post: (0, 0),
            watch: (0, 0),
            posts: Vec::new(),
            posts_side: -1,
            far: Pathfinder::new(),
            fielded: Vec::new(),
            detour: None,
            spare: false,
            forward: false,
            heard: None,
            defuse_heard: LONG_AGO,
        }
    }

    /// Call from `Bot::turn`.
    pub fn turn(&mut self) {
        let info = match_info();
        if info.phase == PHASE_DRAFT {
            kit::draft(&info); // units are not on the map yet, so this comes before the alive check
            return;
        }
        let me = me();
        if !me.alive() || self.sites.is_empty() {
            return;
        }
        if self.plan_for != (info.round, info.my_side) {
            self.assign(&me, &info);
        }
        match info.phase {
            PHASE_BUY => {
                buy::shop(&self.style, &me, &info, &self.core, &self.kit);
                // Last in the turn: if it runs out of fuel, nothing after it is lost.
                self.survey(&info);
            }
            PHASE_ACTION | PHASE_PLANTED => self.fight(&me, &info),
            _ => {}
        }
    }

    fn centre(&self, site: usize) -> (i32, i32) {
        (self.sites[site].centre_x, self.sites[site].centre_y)
    }

    /// Path length from me to the centre of `site`: exact once its field is built, else a guess from the
    /// straight line.
    fn far_len(&mut self, me: &UnitInfo, site: usize) -> i32 {
        let centre = self.centre(site);
        let straight = dist(me.pos(), centre);
        if self.fielded[site] < plan::FIELD_STEPS {
            return straight * 3 / 2;
        }
        self.far.path_len(&self.nav.grid, me.pos(), centre).unwrap_or(straight)
    }

    /// The last enemy position a teammate reported, if it is recent.
    fn sighting(&self, info: &MatchInfo) -> Option<(i32, i32)> {
        let [stamp, x, y] = team_mem_get::<3>(MEM_SIGHT);
        (stamp > 0 && info.tick + 1 - stamp <= SIGHT_TTL).then_some((x, y))
    }

    /// Am I the living teammate closest to `p`? (Ties go to the lower id.)
    fn nearest_to(&self, me: &UnitInfo, p: (i32, i32)) -> bool {
        let mine = (dist2(me.pos(), p), me.id);
        allies().filter(|a| a.alive()).all(|a| (dist2(a.pos(), p), a.id) > mine)
    }

    /// Living teammates within `radius` of me.
    fn allies_within(&self, me: &UnitInfo, radius: i32) -> usize {
        allies().filter(|a| a.alive() && dist(a.pos(), me.pos()) <= radius).count()
    }
}
