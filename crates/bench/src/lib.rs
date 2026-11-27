//! Bench and balance harness for Imperial Battlecode.
//!
//! The point of this crate is honest numbers: every rate comes with its sample size and interval, paired
//! designs remove map and side variance, exact repeats are never counted as new evidence, and a result
//! inside the noise is called that. See `docs/hillclimb.md` for the workflow.

pub mod ab;
pub mod backend;
pub mod balance;
mod balance_render;
pub mod balance_weapons;
pub mod compare;
pub mod dump;
pub mod league;
pub mod metrics;
pub mod model;
pub mod noise;
pub mod report;
pub mod stats;
