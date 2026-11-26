//! Gunplay seam. The core calls only the functions marked SEAM; everything else is this module's business.
//! The exact formulas, the weapon table and measured time-to-kill figures are in `docs/rules/combat.md`.
//!
//! Model (all deterministic): a trigger pull by a unit that is not braced starts a startup of
//! `windup + unsteadiness * startup_per_point / 100` ticks (unsteadiness builds while the unit moves and
//! decays while it stands). When it ends the weapon releases `pellets` bullets fanned S apart,
//! S = spread + recoil_spread * recoil, centred on the aim, and the unit is braced: while it stands and keeps
//! pulling, every pull leaves on its own tick, one per `1 + cooldown` ticks. Moving never adds spread.
//!
//! Split by decision:
//! - `state`: what a unit's weapon is doing (startup, brace, cooldown, reload, recoil) and who may start what
//! - `steady`: how unsteady a unit is and how long its startup takes
//! - `firing`: where the bullets of one trigger pull go
//! - `ballistics`: what a bullet does on the way (muzzle, falloff)
//! - `drops`: weapons on the ground (death, `A_DROP`, `A_PICKUP`)

mod ballistics;
mod drops;
mod firing;
mod state;
mod steady;

pub use ballistics::{bullet_damage, damage_at};
pub use drops::{on_death, on_drop, on_pickup};
pub use firing::{extra_spread, fan, Fan};
pub use state::{
    can_fire, cancel, on_cast, on_round_start, pre_step, reload, start_fire, swap, UnitCombat,
};
pub use steady::{after_move, startup_ticks, unsteadiness_after};
