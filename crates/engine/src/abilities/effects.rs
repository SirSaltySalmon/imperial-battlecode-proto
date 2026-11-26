//! What active effects do each tick. Runs once per action tick, after movement and before the core counts the
//! unit statuses (`blind`, `revealed`) down, so a status meant to last n bot turns is set to n + 1.

use super::{aux_of, clear_line, is_active, linger, max_hp, open_between, rules_of, touches_unit};
use crate::fixed::{self, V2};
use crate::world::{Effect, World};
use ibc_abi::*;

/// SEAM: once per action tick after movement: cooldowns, active effects, expiry.
pub fn step(w: &mut World) {
    for u in w.units.iter_mut() {
        for c in u.abil.cooldown.iter_mut() {
            *c = (*c - 1).max(0);
        }
    }
    for i in 0..w.effects.len() {
        let e = w.effects[i];
        if !is_active(w, &e) {
            continue;
        }
        let r = rules_of(w, &e);
        match e.info.kind {
            EFFECT_FIRE => burn(w, &e, &r),
            EFFECT_FLASH => pop(w, &e, &r),
            EFFECT_RECON => ping(w, &e, &r),
            EFFECT_DECOY => fake_sounds(w, &e, &r),
            EFFECT_HEAL => mend(w, i, &r),
            _ => {}
        }
    }
    for i in 0..w.effects.len() {
        w.effects[i].info.ticks_left -= 1;
        let e = w.effects[i];
        // A barrier that finishes arming on top of a unit fails instead of trapping it.
        if e.info.kind == EFFECT_BARRIER && e.info.ticks_left == linger(&rules_of(w, &e)) {
            let (a, b) = super::ends(&e);
            if touches_unit(w, a, b) {
                w.effects[i].info.ticks_left = 0;
            }
        }
    }
    w.effects.retain(|e| e.info.ticks_left > 0);
}

fn centre(e: &Effect) -> V2 {
    (e.info.x, e.info.y)
}

fn within(a: V2, b: V2, radius: i32) -> bool {
    fixed::dist2(a, b) <= radius as i64 * radius as i64
}

/// Incendiary: `power` damage to every unit in the circle that a wall or barrier does not shelter.
/// Fire does not pick sides; only damage to enemies is credited to the thrower.
fn burn(w: &mut World, e: &Effect, r: &AbilityRules) {
    let at = centre(e);
    let victims: Vec<usize> = (0..w.units.len())
        .filter(|&j| w.units[j].alive && within(w.units[j].pos, at, e.info.radius) && open_between(w, at, w.units[j].pos))
        .collect();
    for j in victims {
        let attacker = (w.units[j].team != e.info.team).then_some(e.owner as usize);
        w.damage(j, r.power, attacker, -1);
    }
}

/// Flash: blinds every unit, friend or foe, that has the pop inside its vision cone with a clear line to it.
/// Facing it head-on blinds for `duration` turns, falling linearly to `power` percent at the cone's edge.
fn pop(w: &mut World, e: &Effect, r: &AbilityRules) {
    let at = centre(e);
    let fov = w.rules.core.fov_half.max(1) as i64;
    let edge = r.power.clamp(0, 100) as i64;
    for j in 0..w.units.len() {
        let u = &w.units[j];
        if !u.alive || !within(u.pos, at, e.info.radius) {
            continue;
        }
        let off = if u.pos == at {
            0
        } else {
            fixed::angle_diff(u.facing, fixed::atan2(at.1 - u.pos.1, at.0 - u.pos.0)).abs() as i64
        };
        if off > fov || !clear_line(w, at, u.pos) {
            continue;
        }
        let pct = edge + (100 - edge) * (fov - off) / fov;
        let turns = (r.duration as i64 * pct / 100).max(1) as i32;
        let u = &mut w.units[j];
        u.blind = u.blind.max(turns + 1);
    }
}

/// Recon: marks every enemy the beacon has a clear line to within its radius for `power` turns.
fn ping(w: &mut World, e: &Effect, r: &AbilityRules) {
    let at = centre(e);
    for j in 0..w.units.len() {
        let u = &w.units[j];
        if u.alive && u.team != e.info.team && within(u.pos, at, e.info.radius) && clear_line(w, at, u.pos) {
            let u = &mut w.units[j];
            u.revealed = u.revealed.max(r.power.max(0) + 1);
        }
    }
}

/// Decoy: enemies hear a footstep every tick and a shot every `power` ticks, exactly as real ones sound.
/// Inside `radius` the device is recognisable and is heard as `SOUND_DECOY` instead. Hearing is shared by the
/// team, so once any listener is that close the whole team knows it is a decoy.
fn fake_sounds(w: &mut World, e: &Effect, r: &AbilityRules) {
    let at = centre(e);
    let core = w.rules.core;
    let shot = r.power > 0 && e.info.ticks_left % r.power == 0;
    let source = e.info.team;
    let listener = 1 - source;
    let near =
        |w: &World, radius: i32| w.units.iter().any(|u| u.alive && u.team == listener && within(u.pos, at, radius));
    if near(w, e.info.radius) {
        w.hear(listener, SOUND_DECOY, at, source);
        return;
    }
    if near(w, core.step_sound_radius) {
        w.hear(listener, SOUND_STEP, at, source);
    }
    if shot && near(w, core.shot_sound_radius) {
        w.hear(listener, SOUND_SHOT, at, source);
    }
}

/// Heal: `power` hit points per tick to the target, up to its kit's maximum. The effect follows the target
/// and ends with it.
fn mend(w: &mut World, i: usize, r: &AbilityRules) {
    let j = aux_of(w.effects[i].data) as usize;
    if j >= w.units.len() || !w.units[j].alive {
        w.effects[i].info.ticks_left = 0;
        return;
    }
    let cap = max_hp(w, j);
    let u = &mut w.units[j];
    u.hp = (u.hp + r.power.max(0)).min(cap.max(u.hp));
    let at = u.pos;
    w.effects[i].info.x = at.0;
    w.effects[i].info.y = at.1;
}
