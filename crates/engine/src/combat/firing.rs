//! Where the bullets of one trigger pull go. Nothing here is random: `fan` is a pure function of values the
//! shooter can read (`T_WEAPON`, its own facing and `UnitInfo.recoil`), so a bot can compute its own bullets
//! exactly. Movement adds no spread (it is paid for in startup time, `steady`), and nothing pushes the fan off
//! the aim: a single-pellet weapon always flies exactly along the clamped aim.

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

/// Spread added between pellets by `recoil` heat: `min(recoil_spread * recoil, 32768)`.
pub fn extra_spread(wr: &WeaponRules, recoil: i32) -> i32 {
    let e = wr.recoil_spread as i64 * recoil.max(0) as i64;
    e.clamp(0, 32768) as i32
}

/// The fan released by a unit looking along `facing` that asked for `aim`, with `recoil` heat.
///
/// - The aim is clamped to the view cone: `facing +/- fov_half`, and the fan is centred on it.
/// - Pellets are `spread + extra_spread(..)` apart, so sustained fire widens multi-pellet fans only.
pub fn fan(wr: &WeaponRules, facing: i32, aim: i32, fov_half: i32, recoil: i32) -> Fan {
    // You can only shoot where you are looking.
    let off = fixed::angle_diff(facing, aim).clamp(-fov_half, fov_half);
    Fan {
        centre: (facing + off) & 0xFFFF,
        step: wr.spread + extra_spread(wr, recoil),
        pellets: wr.pellets,
    }
}

/// The startup of unit `u` has finished: bullets leave, one round is used, recoil and cooldown start, and the
/// unit is braced.
pub(super) fn release(w: &mut World, u: usize) {
    let core = w.rules.core;
    let unit = w.units[u].clone();
    let wr = w.rules.weapons[unit.weapon() as usize];
    let fan = fan(
        &wr,
        unit.facing,
        unit.combat.aim,
        core.fov_half,
        unit.combat.recoil,
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
        m.combat.braced = true;
        m.combat.recoil += wr.recoil_per_shot;
        m.combat.cooldown = wr.cooldown;
        m.combat.shots += 1;
        m.stats.shots += 1;
    }
    w.emit_sound(SOUND_SHOT, unit.pos, unit.team, core.shot_sound_radius);
}
