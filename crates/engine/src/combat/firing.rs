//! Where the bullets of one trigger pull go. Nothing here is random: `fan` is a pure function of values the
//! shooter can read (`T_WEAPON`, its own facing, velocity and `UnitInfo.recoil`), so a bot can compute its
//! own bullets exactly.

use super::ballistics;
use crate::fixed;
use crate::world::{Projectile, World};
use ibc_abi::*;

/// The bullets of one trigger pull: `pellets` directions `step` apart, centred on `centre`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fan {
    pub centre: i32,
    pub step: i32,
    pub pellets: i32,
}

impl Fan {
    /// Direction of pellet `i` (0-based), 0..65535. Pellets are symmetric about `centre`: an odd count puts
    /// one bullet on it, an even count straddles it by `step / 2`.
    pub fn angle(&self, i: i32) -> i32 {
        (self.centre + (2 * i - (self.pellets - 1)) * self.step / 2) & 0xFFFF
    }

    pub fn angles(&self) -> impl Iterator<Item = i32> + '_ {
        (0..self.pellets).map(|i| self.angle(i))
    }
}

/// Spread added by moving at `speed` (units/tick) with `recoil` heat: `move_spread * speed / 1024 +
/// recoil_spread * recoil`.
pub fn extra_spread(wr: &WeaponRules, speed: i32, recoil: i32) -> i32 {
    let e = wr.move_spread as i64 * speed.max(0) as i64 / 1024
        + wr.recoil_spread as i64 * recoil.max(0) as i64;
    e.clamp(0, 32768) as i32
}

/// The fan released by a unit looking along `facing` that asked for `aim`, moving at `speed` with `recoil`
/// heat, having fired `shots` times before this round.
///
/// - The aim is clamped to the view cone: `facing +/- fov_half`.
/// - Pellets are `S = spread + extra` apart, `extra = extra_spread(..)`.
/// - The whole fan is shifted by `extra / 2`: towards larger angles on even `shots`, smaller on odd.
pub fn fan(
    wr: &WeaponRules,
    facing: i32,
    aim: i32,
    fov_half: i32,
    speed: i32,
    recoil: i32,
    shots: i32,
) -> Fan {
    let extra = extra_spread(wr, speed, recoil);
    let bias = if shots % 2 == 0 {
        extra / 2
    } else {
        -(extra / 2)
    };
    // You can only shoot where you are looking.
    let off = fixed::angle_diff(facing, aim).clamp(-fov_half, fov_half);
    Fan {
        centre: (facing + off + bias) & 0xFFFF,
        step: wr.spread + extra,
        pellets: wr.pellets,
    }
}

/// The wind-up of unit `u` has finished: bullets leave, one round is used, recoil and cooldown start.
pub(super) fn release(w: &mut World, u: usize) {
    let core = w.rules.core;
    let unit = w.units[u].clone();
    let wr = w.rules.weapons[unit.weapon() as usize];
    // Speed on the tick the bullets leave (velocity was set from this tick's intent just before).
    let speed = fixed::dist((0, 0), unit.vel);
    let fan = fan(
        &wr,
        unit.facing,
        unit.combat.aim,
        core.fov_half,
        speed,
        unit.combat.recoil,
        unit.combat.shots,
    );
    for ang in fan.angles() {
        if let Some(pos) = ballistics::muzzle(w, unit.pos, ang) {
            w.projectiles.push(Projectile {
                owner: unit.id,
                team: unit.team,
                weapon: wr.id,
                kind: 0,
                pos,
                vel: fixed::polar(ang, wr.bullet_speed),
                travelled: 0,
                range: wr.range,
            });
        }
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
