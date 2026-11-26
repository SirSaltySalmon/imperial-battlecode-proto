//! Kits and abilities: the draft, and when each ability is worth this tick's action.
//!
//! Abilities are used for the objective first: smoke on the line the enemy arrives on (or on the charge,
//! to defuse inside it), fire on a defuse in progress, a barrier in front of a plant or defuse, recon on
//! the site before going in, a trap where the enemy arrives or on our planted charge, sonar on the site, a
//! concussion on a plant or defuse. Flash, heal and dash are used as the fight asks.
use ibc::*;

/// Heal a teammate, or raise a barrier, below this share of 100 hit points.
const HURT_HP: i32 = 60;
/// Do not flash an enemy closer than this: we would be inside the pop as well.
const FLASH_MIN_DIST: i32 = 3 * TILE;
/// Objective abilities are used within this distance of the site or the charge.
const NEAR: i32 = 7 * TILE;
/// A defender smokes the approach when an enemy was reported this close to it.
const THREAT: i32 = 6 * TILE;
/// After a refused use, leave that slot alone for this many ticks.
const RETRY_TICKS: i32 = 6;

/// What a unit knows this tick that decides which ability is worth using.
pub(crate) struct Scene {
    pub attacking: bool,
    pub planted: bool,
    /// Nearest visible enemy.
    pub enemy: Option<UnitInfo>,
    /// The planted charge, or the centre of the site this unit plays.
    pub objective: (i32, i32),
    /// Where the enemy comes into view of the objective.
    pub watch: (i32, i32),
    pub sighting: Option<(i32, i32)>,
    pub other_site: Option<(i32, i32)>,
    pub defuse_heard: bool,
    /// The round is a few ticks old, so the team is on its way.
    pub opened: bool,
    /// This unit is about to plant or defuse.
    pub channel_soon: bool,
}

#[derive(Default)]
pub(crate) struct Kit {
    pub abilities: [Option<AbilityRules>; 2],
    /// Keep facing this angle until this tick: our own flash is about to pop.
    look_away: Option<(i32, i32)>,
    /// Per slot: do not try again before this tick.
    retry_at: [i32; 2],
}

/// Kit 0 is the plain one. Slot 0 drafts for the whole team, spreading the others so no two teammates share
/// a kit while there are enough to go round (kits are unique within a team); the seed decides who gets which.
pub(crate) fn draft(info: &MatchInfo) {
    let kits = core_rules().kits;
    if kits <= 1 || info.unit_slot != 0 {
        return;
    }
    let shift = (info.seed_lo.unsigned_abs() % (kits as u32 - 1)) as i32;
    for slot in 0..info.team_size {
        let _ = ibc::draft_for(1 + (slot + shift) % (kits - 1), slot);
    }
}

impl Kit {
    /// Read the abilities of the kit `me` plays (once per round).
    pub fn load(&mut self, me: &UnitInfo) {
        let slots = kit(me.kit).map_or([-1, -1], |k| k.abilities);
        self.abilities = slots.map(ability);
        self.look_away = None;
        self.retry_at = [0, 0];
    }

    /// True while this unit must keep its eyes off its own flash (it has been turned away).
    pub fn looking_away(&mut self, info: &MatchInfo) -> bool {
        match self.look_away {
            Some((angle, until)) if info.tick <= until => {
                let _ = face(angle);
                true
            }
            _ => {
                self.look_away = None;
                false
            }
        }
    }

    /// Use at most one ability. It replaces this tick's primary action (a shot, a reload).
    pub fn cast(&mut self, me: &UnitInfo, info: &MatchInfo, s: &Scene) -> bool {
        for slot in 0..2 {
            let a = match self.abilities[slot] {
                Some(a) if me.charges[slot] > 0 && me.cooldown[slot] == 0 && info.tick >= self.retry_at[slot] => a,
                _ => continue,
            };
            let Some(t) = target(&a, me, s) else { continue };
            if use_ability(slot as i32, t.0, t.1).is_err() {
                self.retry_at[slot] = info.tick + RETRY_TICKS;
                continue;
            }
            if a.effect == EFFECT_FLASH {
                let away = angle_to(me.x - t.0, me.y - t.1);
                let _ = face(away);
                self.look_away = Some((away, info.tick + tactics::ticks_to_effect(&a, me.pos(), t)));
            }
            return true;
        }
        false
    }
}

/// A living teammate stands within `radius` of `p` (abilities hit our own team too).
fn ally_near(p: (i32, i32), radius: i32) -> bool {
    allies().any(|u| u.alive() && dist(u.pos(), p) <= radius)
}

fn smoked(p: (i32, i32), radius: i32) -> bool {
    effects().any(|e| e.kind == EFFECT_SMOKE && dist((e.x, e.y), p) <= radius)
}

/// Where to use this ability right now, if anywhere.
fn target(a: &AbilityRules, me: &UnitInfo, s: &Scene) -> Option<(i32, i32)> {
    let pos = me.pos();
    let quiet = s.enemy.is_none();
    let in_range = |p: (i32, i32)| dist(pos, p) <= a.range;
    let near = dist(pos, s.objective) <= NEAR;
    match a.effect {
        EFFECT_SMOKE => {
            let p = match (s.attacking, s.planted) {
                (true, false) if near => s.watch,
                (false, true) if near => s.objective,
                (false, false) if s.sighting.is_some_and(|seen| dist(seen, s.watch) <= THREAT) => s.watch,
                _ => return None,
            };
            (quiet && in_range(p) && los(pos, p) && !smoked(p, a.radius)).then_some(p)
        }
        EFFECT_FLASH => {
            let e = s.enemy.filter(|e| dist(pos, e.pos()) >= FLASH_MIN_DIST && in_range(e.pos()))?;
            Some(e.pos())
        }
        EFFECT_FIRE => {
            let clear = |p: (i32, i32)| in_range(p) && dist(pos, p) > a.radius + TILE;
            if s.attacking && s.planted && s.defuse_heard && clear(s.objective) && los(pos, s.objective) {
                return Some(s.objective);
            }
            // Burn whoever cannot step out of it: someone planting, defusing or standing still.
            let e = s.enemy.filter(|e| e.has(S_PLANTING | S_DEFUSING) || (e.vx, e.vy) == (0, 0))?;
            clear(e.pos()).then(|| e.pos())
        }
        EFFECT_BARRIER => {
            let towards = match s.enemy {
                _ if s.channel_soon && quiet => s.watch,
                Some(e) if me.hp <= HURT_HP => e.pos(),
                _ => return None,
            };
            let reach = a.range.min(dist(pos, towards) / 2);
            Some(offset(me.x, me.y, angle_to(towards.0 - me.x, towards.1 - me.y), reach))
        }
        EFFECT_RECON => {
            let p = if s.attacking || s.planted { s.objective } else { s.sighting? };
            (quiet && s.opened && in_range(p)).then_some(p)
        }
        EFFECT_DECOY => {
            // Sound like an attack on the site we are not hitting.
            let other = s.other_site.filter(|_| s.attacking && !s.planted && s.opened && quiet)?;
            let reach = a.range.min(dist(pos, other));
            Some(offset(me.x, me.y, angle_to(other.0 - me.x, other.1 - me.y), reach))
        }
        EFFECT_DASH => {
            // Along the way we are already running; never into a fight, and not past the objective.
            let speed = len(me.vx, me.vy);
            let map = map_info();
            (quiet && s.opened && speed > 0 && dist(pos, s.objective) > a.range + TILE)
                .then(|| (me.x + me.vx * a.range / speed, me.y + me.vy * a.range / speed))
                .filter(|p| p.0 >= 0 && p.1 >= 0 && p.0 <= map.width * TILE && p.1 <= map.height * TILE)
        }
        EFFECT_TRAP => {
            // Defending: on the way in. Attacking with the charge down: on the charge, where a defuser stands.
            let p = match (s.attacking, s.planted) {
                (false, false) if near => {
                    let reach = a.range.min(dist(pos, s.watch));
                    offset(me.x, me.y, angle_to(s.watch.0 - me.x, s.watch.1 - me.y), reach)
                }
                (true, true) if in_range(s.objective) => s.objective,
                _ => return None,
            };
            let clear = dist(pos, p) > a.radius + TILE && !ally_near(p, a.radius + TILE);
            (quiet && s.opened && clear).then_some(p)
        }
        EFFECT_SONAR => {
            // Before going onto the site, or before retaking it.
            let ready = (s.attacking && !s.planted) || (!s.attacking && s.planted);
            let p = s.objective;
            let clear = dist(pos, p) > a.radius + TILE && !ally_near(p, a.radius + TILE);
            (ready && quiet && s.opened && in_range(p) && clear).then_some(p)
        }
        EFFECT_CONCUSSION => {
            let clear =
                |p: (i32, i32)| in_range(p) && dist(pos, p) > a.radius + TILE && !ally_near(p, a.radius + TILE);
            if s.attacking && s.planted && s.defuse_heard && clear(s.objective) && los(pos, s.objective) {
                return Some(s.objective);
            }
            let e = s.enemy.filter(|e| e.has(S_PLANTING | S_DEFUSING))?;
            clear(e.pos()).then(|| e.pos())
        }
        EFFECT_HEAL => {
            let hurt = |u: &UnitInfo| u.alive() && u.hp <= HURT_HP && dist(pos, u.pos()) <= a.range;
            if hurt(me) {
                return Some(pos);
            }
            allies().filter(hurt).min_by_key(|u| u.hp).map(|u| u.pos())
        }
        _ => None,
    }
}
