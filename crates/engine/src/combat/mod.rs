//! Gunplay seam. The core calls only the functions marked SEAM; everything else is this module's business.
//! The exact formulas, the weapon table and measured time-to-kill figures are in `docs/rules/combat.md`.
//!
//! Model (all deterministic): a trigger pull at tick t starts a wind-up of `windup` ticks; when it ends the
//! weapon releases `pellets` bullets fanned S apart, S = spread + move_spread*speed/1024 + recoil_spread*recoil.
//! Movement and recoil also push the whole fan sideways by (S - spread)/2, alternating left/right per shot.
//!
//! Split by decision:
//! - `state`: what a unit's weapon is doing (wind-up, cooldown, reload, recoil) and who may start what
//! - `firing`: where the bullets of one trigger pull go
//! - `ballistics`: what a bullet does on the way (muzzle, falloff)
//! - `drops`: weapons on the ground (death, `A_DROP`, `A_PICKUP`)

mod ballistics;
mod drops;
mod firing;
mod state;

pub use ballistics::{bullet_damage, damage_at};
pub use drops::{on_death, on_drop, on_pickup};
pub use firing::{extra_spread, fan, Fan};
pub use state::{can_fire, cancel, on_round_start, pre_step, reload, start_fire, swap, UnitCombat};
