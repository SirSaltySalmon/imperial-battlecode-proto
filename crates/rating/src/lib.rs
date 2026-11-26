//! Ratings, ladder matchmaking, series plans and the match budget governor for Imperial Battlecode.
//!
//! Everything here is a pure function: no I/O, no clocks, no async, and every random choice comes from an
//! explicit seed. The coordinator owns the state (ratings, history, map usage) and calls in:
//!
//! 1. [`series_per_batch`] turns spare hardware into a number of series for the next ranked batch.
//! 2. [`plan_batch`] picks who plays whom ([`Pairing`]); between batches, [`plan_placement`] picks the
//!    opponent of each placement series of a team with a new submission.
//! 3. [`plan_series`] expands a pairing into games: maps, engine seeds and A/B slots ([`SeriesPlan`]).
//! 4. As each game result arrives, [`SeriesPlan::result`] turns the engine's [`Winner`] slot into a
//!    [`GameResult`]. When the series is over, [`rate_series`] updates the two ratings once, from every
//!    map's result, with the chosen [`RatingSystem`] ([`Elo`] or [`WengLin`]).
//!
//! See the crate README for the maths and the reasons behind the defaults.

pub mod bracket;
mod budget;
mod elo;
mod matchmaking;
pub mod rng;
mod series;
mod series_rating;
mod weng_lin;

pub use budget::{series_per_batch, BudgetConfig};
pub use elo::Elo;
pub use matchmaking::{plan_batch, plan_placement, MatchmakingConfig, Pairing, TeamState};
pub use series::{game_seed, plan_series, PlannedGame, SeriesPlan, Winner};
pub use series_rating::rate_series;
pub use weng_lin::WengLin;

/// Stable identifier of a team, chosen by the coordinator.
pub type TeamId = u64;

/// One team's rating. The meaning of `mu` and `sigma` belongs to the [`RatingSystem`] that produced it;
/// never mix ratings from two systems.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rating {
    /// Skill estimate. Elo: the rating itself (1500 start). Weng-Lin: the mean (25 start).
    pub mu: f64,
    /// Uncertainty. Weng-Lin: the standard deviation of the skill belief. Elo has no variance, so it
    /// stores the fraction of the provisional window still open, from 1 (new) down to 0.
    pub sigma: f64,
    /// Rated games played, over all submissions.
    pub games: u32,
}

/// Result of one game from the point of view of the series' team A (not the engine slot; see
/// [`SeriesPlan::result`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Outcome {
    AWins,
    BWins,
    Draw,
}

impl Outcome {
    /// Score of team A: 1, 0 or 1/2.
    pub fn score_a(self) -> f64 {
        match self {
            Outcome::AWins => 1.0,
            Outcome::BWins => 0.0,
            Outcome::Draw => 0.5,
        }
    }
}

/// One finished game of a series, ready to be rated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GameResult {
    pub series_id: u64,
    pub game_index: u32,
    pub team_a: TeamId,
    pub team_b: TeamId,
    pub outcome: Outcome,
}

/// A two-team rating system. Implementations are plain configuration: all state lives in [`Rating`].
pub trait RatingSystem {
    /// Short stable name, for logs and for tagging stored ratings.
    fn name(&self) -> &'static str;

    /// Rating of a team that has never played.
    fn initial(&self) -> Rating;

    /// New ratings of (A, B) after one game. The ladder combines these per series with [`rate_series`].
    fn update(&self, a: &Rating, b: &Rating, outcome: Outcome) -> (Rating, Rating);

    /// Expected score of A against B (win probability plus half the draw probability).
    fn expected_score(&self, a: &Rating, b: &Rating) -> f64;

    /// Leaderboard number on a 1500-centred Elo-like scale.
    fn display(&self, r: &Rating) -> f64;

    /// Whether the rating is still too uncertain to be shown without a "provisional" mark.
    fn is_provisional(&self, r: &Rating) -> bool;

    /// Uncertainty normalised to `[0, 1]` (1 = never played). The matchmaker uses it to give
    /// uncertain teams more games.
    fn uncertainty(&self, r: &Rating) -> f64;

    /// The team uploaded a new bot: keep the estimate, reopen part of the uncertainty.
    fn on_new_submission(&self, r: &Rating) -> Rating;

    /// A balance patch changed the game: widen every team's uncertainty by `factor` (>= 1).
    fn on_balance_patch(&self, r: &Rating, factor: f64) -> Rating;

    /// The team had no rated game for `periods` rating periods.
    fn on_inactive(&self, r: &Rating, periods: u32) -> Rating;
}

/// Rate one game in place (simulations; the ladder rates whole series with [`rate_series`]).
pub fn apply_game(system: &dyn RatingSystem, a: &mut Rating, b: &mut Rating, outcome: Outcome) {
    let (na, nb) = system.update(a, b, outcome);
    *a = na;
    *b = nb;
}
