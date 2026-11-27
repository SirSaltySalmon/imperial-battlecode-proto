//! Optional building blocks shared by the reference bots: shooting, shopping, reading approaches.
//! Nothing here is privileged: it is all written against the public SDK, copy and change it freely.
use crate::geom::{angle_to, dist, dist2, lead};
use crate::nav::{tile_centre, Navigator};
use crate::*;

/// Angle units per radian (65536 / 2 pi).
const UNITS_PER_RADIAN: i64 = 10430;

/// Half the angle a unit of `radius` covers when seen from `distance`.
pub fn target_half_angle(radius: i32, distance: i32) -> i32 {
    (radius as i64 * UNITS_PER_RADIAN / distance.max(1) as i64) as i32
}

/// Ticks from a pull to the bullets leaving for a unit that is not braced, holding `w` with `unsteadiness`
/// (`UnitInfo::unsteadiness`): `windup + round(unsteadiness * startup_per_point / 100)`, halves rounded up.
/// A braced unit (`S_BRACED`) fires on the tick it pulls.
pub fn startup_ticks(w: &WeaponRules, unsteadiness: i32) -> i32 {
    w.windup + ((unsteadiness.max(0) as i64 * w.startup_per_point as i64 + 50) / 100) as i32
}

/// The result of `Gunner::engage`.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub enemy: UnitInfo,
    /// Lead-corrected aim angle.
    pub aim: i32,
    pub distance: i32,
    /// Shooting: starting up, braced, or pulling this tick. Moving now would cancel the startup or break the
    /// brace (and a pull made on a tick the unit moves is refused): stand still this tick.
    pub hold: bool,
}

/// Aims and shoots at what this unit can see. Caches the held weapon's rules.
pub struct Gunner {
    weapon: WeaponRules,
    /// `UnitInfo` does not show the weapon cooldown, so it is counted here: the first tick on which the
    /// weapon accepts `fire` again after the last shot that left. A braced unit must pull on exactly this
    /// tick, or it lets go of the brace.
    ready_at: i32,
    /// A pull that was made: (the `ready_at` it gives if its bullets leave, magazine when it was made).
    /// It has left once the magazine is seen to be smaller; a cancelled startup never gets there.
    pending: Option<(i32, i32)>,
}

impl Gunner {
    pub fn new() -> Gunner {
        Gunner { weapon: WeaponRules { id: -1, ..Default::default() }, ready_at: 0, pending: None }
    }

    /// Rules of the weapon `me` holds (one host call when it changes).
    pub fn weapon(&mut self, me: &UnitInfo) -> WeaponRules {
        if self.weapon.id != me.weapon {
            self.pending = None;
            self.weapon = crate::weapon(me.weapon).unwrap_or(WeaponRules { id: me.weapon, ..Default::default() });
        }
        self.weapon
    }

    /// Face the nearest enemy in sight with no wall in between and shoot it whenever the weapon is ready and
    /// the enemy in range (reload if it is empty). `T_ENEMIES` is the team's sight, so an enemy only a
    /// teammate sees behind a wall is skipped: there is no shot at it from here. Returns `None` when there is
    /// no such enemy. Shooting roots the unit: while `Shot::hold` is set, do not move.
    pub fn engage(&mut self, me: &UnitInfo) -> Option<Shot> {
        let from = me.pos();
        let enemy = enemies()
            .filter(|e| e.alive() && los(from, e.pos()))
            .min_by_key(|e| dist2(from, e.pos()))?;
        let w = self.weapon(me);
        let braced = me.has(S_BRACED);
        let startup = if braced { 0 } else { startup_ticks(&w, me.unsteadiness) };
        let target = lead(from, enemy.pos(), (enemy.vx, enemy.vy), w.bullet_speed, startup);
        let aim = angle_to(target.0 - me.x, target.1 - me.y);
        let distance = dist(from, enemy.pos());
        // Bullets leave along the facing (within the view cone), so keep turning while starting up.
        let _ = face(aim);
        let mut hold = me.has(S_WINDUP) || braced;
        if !me.has(S_WINDUP | S_RELOADING) {
            if me.ammo <= 0 {
                let _ = reload();
            } else if distance <= w.range {
                hold |= self.fire_when_ready(me, &w, aim, startup);
            }
        }
        Some(Shot { enemy, aim, distance, hold })
    }

    /// `fire`, unless the weapon is known to be cooling down (the call would only return an error). True if
    /// the pull was accepted.
    fn fire_when_ready(&mut self, me: &UnitInfo, w: &WeaponRules, aim: i32, startup: i32) -> bool {
        let tick = crate::match_info().tick;
        if let Some((ready, magazine)) = self.pending {
            if me.ammo < magazine {
                self.ready_at = ready;
                self.pending = None;
            }
        }
        if tick < self.ready_at || fire(aim).is_err() {
            return false;
        }
        // Bullets leave `startup` ticks from now; the weapon then cools down for `cooldown` ticks.
        self.pending = Some((tick + startup + 1 + w.cooldown.max(0), me.ammo));
        true
    }

    /// Call when no enemy is in sight: reloads once the magazine is at most half full.
    pub fn top_up(&mut self, me: &UnitInfo) {
        let w = self.weapon(me);
        if !me.busy() && me.ammo * 2 <= w.magazine && me.ammo < w.magazine {
            let _ = reload();
        }
    }
}

impl Default for Gunner {
    fn default() -> Gunner {
        Gunner::new()
    }
}

/// Buy one thing: the most expensive affordable weapon better than the one held, else the best
/// affordable armour, else (defenders) a defuse kit. Returns the item bought. Call once per buy tick.
pub fn shop(me: &UnitInfo) -> Option<i32> {
    let held_price = weapon(me.weapon).map_or(0, |w| w.price);
    let mut pick: Option<(i32, ItemRules)> = None; // (priority, item)
    for it in items().filter(|it| it.price <= me.credits) {
        let priority = match it.kind {
            ITEM_KIND_WEAPON if it.value != me.weapon && it.price > held_price => 2_000_000 + it.price,
            ITEM_KIND_ARMOR if it.value > me.armor => 1_000_000 + it.value,
            ITEM_KIND_DEFUSE_KIT if me.side == SIDE_DEFEND && !me.has(S_HAS_KIT) => 1,
            _ => continue,
        };
        if pick.is_none_or(|(p, _)| priority > p) {
            pick = Some((priority, it));
        }
    }
    let (_, it) = pick?;
    buy(it.id).ok().map(|_| it.id)
}

/// Where will someone walking from `origin` come into view of `site`? Walks the shortest path from
/// `site` towards `origin` and returns the last point on it that `site` still has line of sight to.
/// A unit standing near `site` should watch this point. Grows the field towards `origin` until it reaches
/// `site` (about 200 fuel per floor tile, up to 81,000 from a spawn on the pool's maps; nothing more if
/// `Navigator::complete` built it), then 3,000 to 5,000 for the walk: call it in the draft or buy phase.
pub fn approach_point(nav: &mut Navigator, site: (i32, i32), origin: (i32, i32)) -> (i32, i32) {
    // Finish the search towards `origin` now, so the walk below never has to wait for it.
    if nav.path_len(site, origin).is_none() {
        return site;
    }
    let (mut at, mut seen) = (site, site);
    for _ in 0..64 {
        let next = match nav.paths.next_waypoint(&nav.grid, at, origin) {
            Some(n) if n != at => n,
            _ => break,
        };
        if !los(site, next) {
            break;
        }
        seen = next;
        at = next;
    }
    seen
}

/// The tile centre within `radius_tiles` of `anchor` with the longest clear line through `watch`,
/// i.e. a spot to hold an angle from. `skip` picks the n-th best so teammates spread out. Two `los` and a
/// `raycast` per tile of the square: 11,000 to 38,000 fuel at radius 3, so call it in the buy phase.
pub fn hold_spot(nav: &Navigator, anchor: (i32, i32), watch: (i32, i32), radius_tiles: i32, skip: usize) -> (i32, i32) {
    let (ax, ay) = tile_of(anchor.0, anchor.1);
    let mut best: Vec<(i32, (i32, i32))> = Vec::new();
    for ty in ay - radius_tiles..=ay + radius_tiles {
        for tx in ax - radius_tiles..=ax + radius_tiles {
            let p = tile_centre(tx, ty);
            if nav.grid.is_blocked(tx, ty) || p == watch || !los(p, anchor) || !los(p, watch) {
                continue;
            }
            // Long sightline past the watched point, but do not stand in the open next to it.
            let sight = raycast(p.0, p.1, angle_to(watch.0 - p.0, watch.1 - p.1), 64 * TILE);
            best.push((sight.min(dist(p, watch) * 2), p));
        }
    }
    best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    if best.is_empty() {
        anchor
    } else {
        best[skip % best.len().min(4)].1
    }
}

/// Ticks from using ability `a` at `from` towards `target` until it takes effect: its startup, then for a
/// thrown device the flight (it acts in the tick it lands). Walls on the way only make it sooner.
pub fn ticks_to_effect(a: &AbilityRules, from: (i32, i32), target: (i32, i32)) -> i32 {
    if a.speed <= 0 {
        return a.delay;
    }
    let flight = dist(from, target).min(a.range).max(1);
    a.delay + (flight + a.speed - 1) / a.speed - 1
}
