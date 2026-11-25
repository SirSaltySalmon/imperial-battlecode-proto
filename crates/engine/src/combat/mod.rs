//! Gunplay seam. The core calls only the functions marked SEAM; everything else is this module's business.
//! Foundation behaviour: wind-up, fanned pellets, movement/recoil spread, magazines, reload, falloff.
//! Model (all deterministic): a trigger pull at tick t starts a wind-up of `windup` ticks; when it ends the
//! weapon releases `pellets` bullets fanned S apart, S = spread + move_spread*speed/1024 + recoil_spread*recoil.
//! Movement and recoil also push the whole fan sideways by (S - spread)/2, alternating left/right per shot.

use crate::fixed;
use crate::world::{Projectile, World};
use crate::Fnv;
use ibc_abi::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct UnitCombat {
    pub winding: bool,
    pub windup: i32,
    pub aim: i32,
    pub cooldown: i32,
    pub reload: i32,
    pub recoil: i32,
    pub shots: i32,
}

impl UnitCombat {
    pub fn hash(&self, h: &mut Fnv) {
        for v in [self.winding as i32, self.windup, self.aim, self.cooldown, self.reload, self.recoil, self.shots] {
            h.i32(v);
        }
    }
    /// Ticks left in the current weapon channel, for UnitInfo.channel.
    pub fn channel(&self) -> i32 {
        if self.winding {
            self.windup + 1
        } else {
            self.reload
        }
    }
}

/// SEAM: could `start_fire` succeed right now? Used by `act` to give bots an early error code.
pub fn can_fire(w: &World, u: usize) -> i32 {
    let unit = &w.units[u];
    let c = &unit.combat;
    if unit.weapon() < 0 || c.winding || c.reload > 0 || c.cooldown > 0 || unit.ammo[unit.slot] <= 0 {
        return ERR_NOT_READY;
    }
    OK
}

/// SEAM: begin a wind-up towards `angle`.
pub fn start_fire(w: &mut World, u: usize, angle: i32) -> i32 {
    let r = can_fire(w, u);
    if r != OK {
        return r;
    }
    let wid = w.units[u].weapon() as usize;
    let windup = w.rules.weapons[wid].windup;
    let c = &mut w.units[u].combat;
    c.winding = true;
    c.windup = windup;
    c.aim = angle & 0xFFFF;
    OK
}

/// SEAM
pub fn reload(w: &mut World, u: usize) -> i32 {
    let unit = &w.units[u];
    if unit.weapon() < 0 {
        return ERR_NOT_READY;
    }
    let wr = w.rules.weapons[unit.weapon() as usize];
    if unit.ammo[unit.slot] >= wr.magazine || unit.combat.reload > 0 {
        return ERR_NOT_READY;
    }
    let c = &mut w.units[u].combat;
    c.winding = false;
    c.reload = wr.reload_ticks;
    OK
}

/// SEAM: switch weapon slot.
pub fn swap(w: &mut World, u: usize, slot: i32) -> i32 {
    if !(0..2).contains(&slot) || w.units[u].weapons[slot as usize] < 0 {
        return ERR_BAD_ARG;
    }
    cancel(w, u);
    w.units[u].slot = slot as usize;
    w.units[u].combat.cooldown = 1;
    OK
}

/// SEAM: abort wind-up and reload.
pub fn cancel(w: &mut World, u: usize) {
    let c = &mut w.units[u].combat;
    c.winding = false;
    c.windup = 0;
    c.reload = 0;
}

/// SEAM: called once per action tick after intents, before movement.
pub fn pre_step(w: &mut World) {
    for u in 0..w.units.len() {
        if !w.units[u].alive {
            continue;
        }
        let mut fired = false;
        {
            let unit = &mut w.units[u];
            if unit.combat.reload > 0 {
                unit.combat.reload -= 1;
                if unit.combat.reload == 0 && unit.weapon() >= 0 {
                    unit.ammo[unit.slot] = w.rules.weapons[unit.weapon() as usize].magazine;
                }
            }
            if unit.combat.cooldown > 0 {
                unit.combat.cooldown -= 1;
            }
        }
        if w.units[u].combat.winding {
            if w.units[u].combat.windup <= 0 {
                release(w, u);
                fired = true;
            } else {
                w.units[u].combat.windup -= 1;
            }
        }
        if !fired && w.units[u].weapon() >= 0 {
            let rec = w.rules.weapons[w.units[u].weapon() as usize].recoil_recovery;
            let c = &mut w.units[u].combat;
            c.recoil = (c.recoil - rec).max(0);
        }
    }
}

fn release(w: &mut World, u: usize) {
    let core = w.rules.core;
    let unit = w.units[u].clone();
    let wr = w.rules.weapons[unit.weapon() as usize];
    let speed = fixed::dist((0, 0), unit.vel) as i64;
    let extra = (wr.move_spread as i64 * speed / 1024 + wr.recoil_spread as i64 * unit.combat.recoil as i64) as i32;
    let s = wr.spread + extra;
    let bias = if unit.combat.shots % 2 == 0 { extra / 2 } else { -extra / 2 };
    // You can only shoot where you are looking.
    let off = fixed::angle_diff(unit.facing, unit.combat.aim).clamp(-core.fov_half, core.fov_half);
    let aim = unit.facing + off + bias;
    for i in 0..wr.pellets {
        let ang = (aim + (2 * i - (wr.pellets - 1)) * s / 2) & 0xFFFF;
        let muzzle = fixed::polar(ang, core.unit_radius);
        w.projectiles.push(Projectile {
            owner: unit.id,
            team: unit.team,
            weapon: wr.id,
            kind: 0,
            pos: (unit.pos.0 + muzzle.0, unit.pos.1 + muzzle.1),
            vel: fixed::polar(ang, wr.bullet_speed),
            travelled: 0,
            range: wr.range,
        });
    }
    {
        let m = &mut w.units[u];
        m.ammo[m.slot] -= 1;
        m.combat.winding = false;
        m.combat.recoil += wr.recoil_per_shot;
        m.combat.cooldown = wr.cooldown;
        m.combat.shots += 1;
        m.stats.shots += 1;
    }
    w.emit_sound(SOUND_SHOT, unit.pos, unit.team, core.shot_sound_radius);
}

/// SEAM: damage of a bullet that hit after travelling `p.travelled`.
pub fn bullet_damage(w: &World, p: &Projectile) -> i32 {
    let wr = match w.rules.weapons.get(p.weapon as usize) {
        Some(wr) => wr,
        None => return 0,
    };
    if p.travelled <= wr.falloff_start || wr.range <= wr.falloff_start {
        return wr.damage;
    }
    let over = (p.travelled - wr.falloff_start).min(wr.range - wr.falloff_start) as i64;
    let span = (wr.range - wr.falloff_start) as i64;
    let pct = 100 - (100 - wr.falloff_min_pct as i64) * over / span;
    (wr.damage as i64 * pct / 100) as i32
}

/// SEAM: reset per-round weapon state (ammo and loadout are the economy module's job).
pub fn on_round_start(w: &mut World) {
    for u in w.units.iter_mut() {
        u.combat = UnitCombat::default();
    }
}

/// SEAM: a unit died. (Weapon drops are not implemented in the foundation.)
pub fn on_death(w: &mut World, u: usize) {
    cancel(w, u);
}

/// SEAM: a non-charge drop was picked up.
pub fn on_pickup(_w: &mut World, _u: usize, _drop: DropInfo) {}

/// SEAM: A_DROP with DROP_WEAPON.
pub fn on_drop(_w: &mut World, _u: usize) {}
