//! The seam where a rating system plugs in. The coordinator needs "two ratings and a game result in, two
//! ratings out", the ladder number, the provisional flag, the resubmission event and, for the ranked
//! matchmaker, the underlying `ibc_rating::RatingSystem`. Every system of `ibc-rating` provides all of it;
//! `by_name` picks one from the configuration.

use std::sync::Arc;

pub use ibc_rating::{Outcome, Rating, RatingSystem};

pub trait RatingHook: Send + Sync {
    /// Stored with the ratings: `mu`/`sigma` of two systems are not interchangeable.
    fn name(&self) -> &str;
    /// The rating of a team that has not played.
    fn initial(&self) -> Rating;
    /// New ratings of A and B after one game.
    fn update(&self, a: &Rating, b: &Rating, outcome: Outcome) -> (Rating, Rating);
    /// The number shown on the ladder, which is sorted by it.
    fn display(&self, r: &Rating) -> f64;
    /// Too uncertain to be shown as a settled number.
    fn is_provisional(&self, r: &Rating) -> bool;
    /// The team's live submission changed: keep the estimate, widen the uncertainty.
    fn on_new_submission(&self, r: &Rating) -> Rating;
    /// A balance patch changed the game: keep the estimate, widen the uncertainty by `factor` (>= 1).
    fn on_balance_patch(&self, r: &Rating, factor: f64) -> Rating;
    /// The system itself, for `ibc_rating::plan_batch`.
    fn system(&self) -> &dyn RatingSystem;
}

impl<S: RatingSystem + Send + Sync> RatingHook for S {
    fn name(&self) -> &str {
        RatingSystem::name(self)
    }
    fn initial(&self) -> Rating {
        RatingSystem::initial(self)
    }
    fn update(&self, a: &Rating, b: &Rating, outcome: Outcome) -> (Rating, Rating) {
        RatingSystem::update(self, a, b, outcome)
    }
    fn display(&self, r: &Rating) -> f64 {
        RatingSystem::display(self, r)
    }
    fn is_provisional(&self, r: &Rating) -> bool {
        RatingSystem::is_provisional(self, r)
    }
    fn on_new_submission(&self, r: &Rating) -> Rating {
        RatingSystem::on_new_submission(self, r)
    }
    fn on_balance_patch(&self, r: &Rating, factor: f64) -> Rating {
        RatingSystem::on_balance_patch(self, r, factor)
    }
    fn system(&self) -> &dyn RatingSystem {
        self
    }
}

/// The configured systems: `weng-lin` (the default, Weng & Lin 2011) and `elo`, with `ibc-rating`'s defaults.
pub fn by_name(name: &str) -> Option<Arc<dyn RatingHook>> {
    match name {
        "weng-lin" => Some(Arc::new(ibc_rating::WengLin::default())),
        "elo" => Some(Arc::new(ibc_rating::Elo::default())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_systems_are_selectable_and_a_win_moves_the_right_way() {
        for name in ["weng-lin", "elo"] {
            let hook = by_name(name).unwrap();
            let (a, b) = hook.update(&hook.initial(), &hook.initial(), Outcome::AWins);
            assert!(a.mu > b.mu, "{name}");
            assert_eq!((a.games, b.games), (1, 1));
            assert!(hook.is_provisional(&hook.initial()));
        }
        assert!(by_name("glicko").is_none());
    }
}
