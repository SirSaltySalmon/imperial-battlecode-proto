//! Abilities and kits. The core calls the SEAM functions; everything else is this module's business.
//! Exact behaviour and counters are documented in docs/rules/abilities.md; every number comes from
//! rules/abilities.toml.
//!
//! State: charges, cooldowns and a use in its startup live in `UnitAbilities`; a thrown device in flight is a
//! `World.projectiles` entry; everything placed in the world is a `World.effects` entry (all hashed by the core).
//! `Effect.data` packs the ability id (high bits) with 16 bits of per-effect state: a barrier's hit points, a
//! heal's target unit.
//!
//! A use waits out its startup (`AbilityRules.delay`, see `cast`), then happens: a thrown device takes off
//! (`thrown`) or the effect appears. An effect is active from the tick it appears for `linger` ticks, which is
//! `duration` (one tick for a flash, a sonar pulse and a concussion, which only pop).

mod cast;
mod control;
mod effects;
mod geom;
mod thrown;

use crate::fixed::{self, V2};
use crate::world::{Effect, Projectile, World};
use crate::{combat, Fnv};
use ibc_abi::*;

pub use cast::{cancel, casting, pre_step};
pub use effects::step;
pub use thrown::{fly, on_landed};

/// `AbilityRules.effect` of the dash, which leaves nothing in the world. The ABI has no constant for it yet:
/// this is the next free id of the `EFFECT_*` group.
pub use ibc_abi::EFFECT_DASH;

/// A thrown device that meets a wall or barrier comes to rest this far in front of it, so it never sits on
/// the boundary of the tile that stopped it. A geometric epsilon, not a balance number.
const THROW_STANDOFF: i64 = 64;

const AUX_BITS: i32 = 16;
const AUX_MASK: i32 = (1 << AUX_BITS) - 1;

#[derive(Clone, Copy, Debug, Default)]
pub struct UnitAbilities {
    pub charges: [i32; 2],
    pub cooldown: [i32; 2],
    /// Ticks left in the startup of a pending use; 0 = none.
    pub cast_left: i32,
    /// Slot and target point of the pending use.
    pub cast_slot: i32,
    pub cast_target: V2,
    /// Where the unit was when effects last ticked: the start of the path a trap sweeps.
    pub last_pos: V2,
}

impl UnitAbilities {
    pub fn hash(&self, h: &mut Fnv) {
        for v in [
            self.charges[0],
            self.charges[1],
            self.cooldown[0],
            self.cooldown[1],
            self.cast_left,
            self.cast_slot,
            self.cast_target.0,
            self.cast_target.1,
            self.last_pos.0,
            self.last_pos.1,
        ] {
            h.i32(v);
        }
    }
}

fn pack(ability: i32, aux: i32) -> i32 {
    (ability << AUX_BITS) | (aux & AUX_MASK)
}

fn aux_of(data: i32) -> i32 {
    data & AUX_MASK
}

/// Rules of the ability that made this effect.
fn rules_of(w: &World, e: &Effect) -> AbilityRules {
    w.rules.abilities.get((e.data >> AUX_BITS) as usize).copied().unwrap_or_default()
}

/// Ticks an effect of this ability stays active once armed.
fn linger(r: &AbilityRules) -> i32 {
    match r.effect {
        EFFECT_FLASH | EFFECT_SONAR | EFFECT_CONCUSSION => 1,
        _ => r.duration.max(0),
    }
}

/// Effects do not arm: whatever is in the world acts until its time runs out (the startup comes before).
fn is_active(e: &Effect) -> bool {
    e.info.ticks_left > 0
}

fn ends(e: &Effect) -> (V2, V2) {
    ((e.info.x, e.info.y), (e.info.x2, e.info.y2))
}

fn active_barriers(w: &World) -> impl Iterator<Item = (usize, &Effect)> {
    w.effects.iter().enumerate().filter(move |(_, e)| e.info.kind == EFFECT_BARRIER && is_active(e))
}

/// No wall and no barrier between the points (smoke does not count).
fn open_between(w: &World, a: V2, b: V2) -> bool {
    w.map.segment_wall(a, b).is_none() && blocks_segment(w, a, b).is_none()
}

fn in_map(w: &World, p: V2) -> bool {
    (0..=w.map.width * TILE).contains(&p.0) && (0..=w.map.height * TILE).contains(&p.1)
}

/// The ability in a unit's kit slot.
fn slot_ability(w: &World, u: usize, slot: i32) -> Result<AbilityRules, i32> {
    if !(0..2).contains(&slot) {
        return Err(ERR_BAD_ARG);
    }
    let id = w.rules.kits.get(w.units[u].kit as usize).map_or(-1, |k| k.abilities[slot as usize]);
    if id < 0 {
        return Err(ERR_UNSUPPORTED);
    }
    w.rules.abilities.get(id as usize).copied().ok_or(ERR_UNSUPPORTED)
}

/// What a use would do, decided from the current state.
enum Plan {
    /// A device that appears at once where it lands (`speed` 0): smoke, flash, incendiary, recon, decoy,
    /// trap, sonar, concussion.
    Device(V2),
    /// A device that flies towards this point (`speed` > 0): at most `range` from the thrower.
    Throw(V2),
    Barrier(V2, V2),
    Dash(V2),
    /// Index of the unit to heal.
    Heal(usize),
}

/// Where a device thrown from `from` at `target` lands: at most `range` away, and short of the first wall or
/// barrier on the way.
fn throw_point(w: &World, from: V2, target: V2, range: i32) -> V2 {
    let aim = reach(from, target, range);
    if aim == from {
        return from;
    }
    let len = fixed::dist(from, aim).max(1) as i64;
    let mut stop = w.map.segment_wall(from, aim).map(|hit| fixed::dist(from, hit) as i64);
    if let Some(t) = blocks_segment(w, from, aim) {
        let s = len * t / fixed::ONE;
        stop = Some(stop.map_or(s, |x| x.min(s)));
    }
    match stop {
        Some(s) => geom::lerp(from, aim, (s - THROW_STANDOFF).clamp(0, len), len),
        None => aim,
    }
}

/// `target`, or the point `range` away from `from` on the way to it if it is further.
fn reach(from: V2, target: V2, range: i32) -> V2 {
    let d = fixed::dist(from, target) as i64;
    if d == 0 {
        return from;
    }
    geom::lerp(from, target, d.min(range.max(0) as i64), d)
}

/// Where a dash from `from` towards `target` ends: the last free position on the straight line, at most
/// `range` away. Sampled every half unit radius, which is too fine to step over a wall or a barrier.
fn dash_end(w: &World, from: V2, target: V2, range: i32) -> V2 {
    let r = w.rules.core.unit_radius;
    let d = fixed::dist(from, target) as i64;
    let len = d.min(range.max(0) as i64);
    if len == 0 {
        return from;
    }
    let slices = len / (r as i64 / 2).max(1) + 1;
    let mut at = from;
    for k in 1..=slices {
        let p = geom::lerp(from, target, len * k / slices, d);
        if w.map.circle_blocked(p, r) || blocks_move(w, p, r) {
            break;
        }
        at = p;
    }
    at
}

/// A barrier across the caster's line to the landing point `c`: `radius` to each side, cut short by walls.
fn barrier_ends(w: &World, from: V2, c: V2, half: i32) -> (V2, V2) {
    let across = fixed::polar(fixed::atan2(c.1 - from.1, c.0 - from.0) + FULL_TURN / 4, half.max(0));
    let clip = |end: V2| w.map.segment_wall(c, end).map_or(end, |hit| off_wall(w, hit));
    (clip((c.0 + across.0, c.1 + across.1)), clip((c.0 - across.0, c.1 - across.1)))
}

/// `segment_wall` reports the point where a segment enters a wall tile, which lies on that tile's boundary and,
/// on its low sides, counts as inside it. Move such a point one unit back onto the floor, so the end of a
/// barrier is a point units can have line of sight to.
fn off_wall(w: &World, p: V2) -> V2 {
    let floor = |q: V2| w.map.tile(q.0.div_euclid(TILE), q.1.div_euclid(TILE)) != TILE_WALL;
    [p, (p.0 - 1, p.1), (p.0, p.1 - 1), (p.0 - 1, p.1 - 1)].into_iter().find(|&q| floor(q)).unwrap_or(p)
}

fn touches_unit(w: &World, a: V2, b: V2) -> bool {
    let r = w.rules.core.unit_radius as i64;
    w.units.iter().any(|u| u.alive && geom::point_segment_dist2(u.pos, a, b) < r * r)
}

/// The living teammate (the caster included) nearest to `target`, if one stands within `radius` of it.
fn heal_target(w: &World, u: usize, target: V2, radius: i32) -> Option<usize> {
    let r2 = radius as i64 * radius as i64;
    (0..w.units.len())
        .filter(|&j| w.units[j].alive && w.units[j].team == w.units[u].team)
        .map(|j| (fixed::dist2(w.units[j].pos, target), j))
        .filter(|&(d2, _)| d2 <= r2)
        .min()
        .map(|(_, j)| j)
}

fn plan(w: &World, u: usize, slot: i32, target: V2) -> Result<(AbilityRules, Plan), i32> {
    let rules = slot_ability(w, u, slot)?;
    let unit = &w.units[u];
    if !unit.alive || !w.in_action() {
        return Err(ERR_NOT_ALLOWED);
    }
    let used = unit.abil.charges[slot as usize] <= 0 || unit.abil.cooldown[slot as usize] > 0;
    if used || casting(w, u) {
        return Err(ERR_NOT_READY);
    }
    if !in_map(w, target) {
        return Err(ERR_BAD_ARG);
    }
    let plan = match rules.effect {
        EFFECT_SMOKE | EFFECT_FLASH | EFFECT_FIRE | EFFECT_RECON | EFFECT_DECOY | EFFECT_TRAP | EFFECT_SONAR
        | EFFECT_CONCUSSION
            if rules.speed > 0 =>
        {
            Plan::Throw(reach(unit.pos, target, rules.range))
        }
        EFFECT_SMOKE | EFFECT_FLASH | EFFECT_FIRE | EFFECT_RECON | EFFECT_DECOY | EFFECT_TRAP | EFFECT_SONAR
        | EFFECT_CONCUSSION => {
            Plan::Device(throw_point(w, unit.pos, target, rules.range))
        }
        EFFECT_BARRIER => {
            let c = throw_point(w, unit.pos, target, rules.range);
            if c == unit.pos {
                return Err(ERR_OUT_OF_RANGE);
            }
            let (a, b) = barrier_ends(w, unit.pos, c, rules.radius);
            if a == b || touches_unit(w, a, b) {
                return Err(ERR_NOT_ALLOWED);
            }
            Plan::Barrier(a, b)
        }
        EFFECT_DASH => {
            let to = dash_end(w, unit.pos, target, rules.range);
            if to == unit.pos {
                return Err(ERR_OUT_OF_RANGE);
            }
            Plan::Dash(to)
        }
        EFFECT_HEAL => {
            let j = heal_target(w, u, target, rules.radius).ok_or(ERR_OUT_OF_RANGE)?;
            let other = &w.units[j];
            let range = rules.range as i64;
            if fixed::dist2(unit.pos, other.pos) > range * range {
                return Err(ERR_OUT_OF_RANGE);
            }
            if other.hp >= max_hp(w, j) || !open_between(w, unit.pos, other.pos) {
                return Err(ERR_NOT_ALLOWED);
            }
            Plan::Heal(j)
        }
        _ => return Err(ERR_UNSUPPORTED),
    };
    Ok((rules, plan))
}

fn max_hp(w: &World, u: usize) -> i32 {
    w.rules.kits.get(w.units[u].kit as usize).map_or(w.rules.core.max_hp, |k| k.hp)
}

/// SEAM: A_DRAFT and A_DRAFT_FOR in the draft phase: unit `u` gets `kit`, applied at once. Kits are unique
/// within a team, except kit 0, which every unit that does not draft plays. Charges are handed out when the
/// round starts.
pub fn draft(w: &mut World, u: usize, kit: i32) -> i32 {
    if kit < 0 || kit as usize >= w.rules.kits.len() {
        return ERR_BAD_ARG;
    }
    let team = w.units[u].team;
    let taken = w.units.iter().enumerate().any(|(j, x)| j != u && x.team == team && x.kit == kit);
    if kit != 0 && taken {
        return ERR_NOT_ALLOWED;
    }
    w.units[u].kit = kit;
    OK
}

/// SEAM: early validation for `act`.
pub fn can_use(w: &World, u: usize, slot: i32, target: V2) -> i32 {
    plan(w, u, slot, target).err().unwrap_or(OK)
}

/// SEAM: A_ABILITY resolved at the start of the tick: the use starts its startup, or happens at once if the
/// ability has none. It interrupts a plant or defuse channel. A use that is no longer possible costs nothing.
pub fn use_ability(w: &mut World, u: usize, slot: i32, target: V2) -> i32 {
    let (rules, plan) = match plan(w, u, slot, target) {
        Ok(p) => p,
        Err(e) => return e,
    };
    w.cancel_channel(u);
    if rules.delay > 0 {
        cast::start(w, u, slot, target, rules.delay);
    } else {
        perform(w, u, slot, &rules, plan);
    }
    OK
}

/// The ability happens: this is where the charge is spent.
fn perform(w: &mut World, u: usize, slot: i32, rules: &AbilityRules, plan: Plan) {
    let (id, team, from) = (w.units[u].id, w.units[u].team, w.units[u].pos);
    match plan {
        Plan::Device(at) => place(w, rules, id, team, (at, (0, 0)), rules.radius, 0),
        Plan::Throw(aim) => thrown::throw(w, u, rules, aim),
        Plan::Barrier(a, b) => place(w, rules, id, team, (a, b), 0, rules.power.clamp(1, AUX_MASK)),
        Plan::Dash(to) => w.units[u].pos = to,
        Plan::Heal(j) => {
            let at = w.units[j].pos;
            place(w, rules, id, team, (at, (0, 0)), 0, j as i32)
        }
    }
    let abil = &mut w.units[u].abil;
    abil.charges[slot as usize] -= 1;
    // `step` counts it down once later this tick, leaving `cooldown` whole ticks without the ability.
    abil.cooldown[slot as usize] = rules.cooldown.max(0) + 1;
    w.emit_sound(SOUND_ABILITY, from, team, w.rules.core.step_sound_radius);
}

/// Put an effect of this ability in the world, active at once for its whole `linger`.
fn place(w: &mut World, rules: &AbilityRules, owner: u32, team: i32, ends: (V2, V2), radius: i32, aux: i32) {
    let ((x, y), (x2, y2)) = ends;
    let info = EffectInfo { kind: rules.effect, team, x, y, radius, ticks_left: linger(rules), x2, y2 };
    w.effects.push(Effect { info, owner, data: pack(rules.id, aux) });
}

/// SEAM: buying an ability charge (ITEM_KIND_ABILITY_CHARGE) for a kit slot. The shop item is the same for
/// every kit, so the ability's own `price` is charged here, on top of whatever the item itself costs: the
/// economy module takes the item's price after this returns, so the unit must be able to pay both.
pub fn add_charge(w: &mut World, u: usize, slot: i32) -> i32 {
    let rules = match slot_ability(w, u, slot) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let charge_item = |it: &&ItemRules| it.kind == ITEM_KIND_ABILITY_CHARGE && it.value == slot;
    let item_price = w.rules.items.iter().filter(charge_item).map(|it| it.price).max().unwrap_or(0).max(0);
    let unit = &mut w.units[u];
    if unit.abil.charges[slot as usize] >= rules.max_charges {
        return ERR_NOT_ALLOWED;
    }
    if unit.credits < rules.price + item_price {
        return ERR_INSUFFICIENT_FUNDS;
    }
    // The shop (economy::buy) takes the credits; this only checks and grants.
    unit.abil.charges[slot as usize] += 1;
    OK
}

/// SEAM: does a dynamic effect block sight from `a` to `b`? Smoke hides everything behind or inside it and a
/// barrier is opaque, but neither hides itself: `T_EFFECTS` tests sight to the reference point of an effect
/// (a cloud's centre, a barrier's ends), so sight to exactly that point ignores that effect. A unit standing
/// on that very point is hidden like any other.
pub fn blocks_vision(w: &World, a: V2, b: V2) -> bool {
    sight_blocked(w, a, b, true)
}

/// No wall, smoke or barrier between an effect at one end and a unit at the other.
fn clear_line(w: &World, a: V2, b: V2) -> bool {
    w.map.segment_wall(a, b).is_none() && !sight_blocked(w, a, b, false)
}

fn sight_blocked(w: &World, a: V2, b: V2, effects_show: bool) -> bool {
    w.effects.iter().any(|e| {
        let (p, q) = ends(e);
        let crosses = match e.info.kind {
            EFFECT_SMOKE => fixed::segment_circle(a, b, p, e.info.radius).is_some(),
            EFFECT_BARRIER => geom::segment_segment(a, b, p, q).is_some(),
            _ => return false,
        };
        let own_point = b == p || (e.info.kind == EFFECT_BARRIER && b == q);
        let shown = effects_show && own_point && !w.units.iter().any(|u| u.alive && u.pos == b);
        crosses && !shown && is_active(e)
    })
}

/// SEAM: does `team` get effect `i` in `T_EFFECTS` (vision is shared by the team, `vision.rs`)? Its own
/// effects always; an enemy effect when a living unit of the team sees its reference point. An enemy trap
/// only to a unit that sees it from within `sense_radius`, even in plain view.
pub fn effect_visible(w: &World, team: i32, i: usize) -> bool {
    let e = &w.effects[i].info;
    if e.team == team {
        return true;
    }
    let at = (e.x, e.y);
    if e.kind != EFFECT_TRAP {
        return w.team_sees_point(team, at);
    }
    let sense = w.rules.core.sense_radius as i64;
    (0..w.units.len()).any(|j| {
        let u = &w.units[j];
        u.team == team && fixed::dist2(u.pos, at) <= sense * sense && w.can_see_point(j, at)
    })
}

/// SEAM: does a dynamic effect block a circle at `p`?
pub fn blocks_move(w: &World, p: V2, r: i32) -> bool {
    let r2 = r as i64 * r as i64;
    active_barriers(w).any(|(_, e)| {
        let (a, b) = ends(e);
        geom::point_segment_dist2(p, a, b) < r2
    })
}

/// The first active barrier a->b crosses: (effect index, Q14 fraction along the segment).
fn first_barrier(w: &World, a: V2, b: V2) -> Option<(usize, i64)> {
    active_barriers(w)
        .filter_map(|(i, e)| {
            let (p, q) = ends(e);
            geom::segment_segment(a, b, p, q).map(|t| (i, t))
        })
        .min_by_key(|&(i, t)| (t, i))
}

/// SEAM: does a dynamic effect stop a projectile on a->b? Returns the Q14 fraction along the segment.
pub fn blocks_segment(w: &World, a: V2, b: V2) -> Option<i64> {
    first_barrier(w, a, b).map(|(_, t)| t)
}

/// SEAM: the projectiles `blocks_segment` stopped in one sub-step, each with the point it was heading for.
/// Every barrier takes the damage of the bullets it stopped, all at once, and breaks when its hit points run
/// out.
pub fn on_blocked(w: &mut World, stopped: &[(Projectile, V2)]) {
    let mut taken = vec![0i32; w.effects.len()];
    for (p, to) in stopped {
        if let Some((i, _)) = first_barrier(w, p.pos, *to) {
            taken[i] = taken[i].saturating_add(combat::bullet_damage(w, p).max(0));
        }
    }
    for (e, dmg) in w.effects.iter_mut().zip(&taken) {
        if *dmg > 0 {
            let hp = aux_of(e.data) - dmg;
            e.data = pack(e.data >> AUX_BITS, hp.max(0));
            if hp <= 0 {
                e.info.ticks_left = 0;
            }
        }
    }
    w.effects.retain(|e| e.info.ticks_left > 0);
}

/// SEAM: cooldowns reset, and abilities that cost nothing are refilled. Bought charges carry over.
pub fn on_round_start(w: &mut World) {
    for u in 0..w.units.len() {
        w.units[u].abil.cooldown = [0, 0];
        w.units[u].abil.cast_left = 0;
        w.units[u].abil.last_pos = w.units[u].pos;
        for slot in 0..2 {
            match slot_ability(w, u, slot) {
                Ok(r) if r.price <= 0 => w.units[u].abil.charges[slot as usize] = r.max_charges.max(0),
                Ok(_) => {}
                Err(_) => w.units[u].abil.charges[slot as usize] = 0,
            }
        }
    }
}
