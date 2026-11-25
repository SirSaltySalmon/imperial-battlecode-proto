//! Plain Elo (A. Elo, "The Rating of Chessplayers, Past and Present", 1978) with a provisional K boost.

use crate::{Outcome, Rating, RatingSystem};

/// Elo configuration. `Rating::sigma` holds the fraction of the provisional window still open.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Elo {
    /// Rating of a new team. 1500, as in Cambridge Battlecode.
    pub start: f64,
    /// K-factor of an established team. 24, as in MIT Battlecode.
    pub k: f64,
    /// K-factor while provisional.
    pub k_provisional: f64,
    /// Length of the provisional window in games. Cambridge Battlecode used 10 matches.
    pub provisional_games: u32,
    /// Rating difference that means 10:1 odds. 400 is the chess convention.
    pub scale: f64,
    /// Fraction of the provisional window reopened by a new submission.
    pub resubmission_window: f64,
}

impl Default for Elo {
    fn default() -> Self {
        Elo {
            start: 1500.0,
            k: 24.0,
            k_provisional: 48.0,
            provisional_games: 10,
            scale: 400.0,
            resubmission_window: 0.5,
        }
    }
}

impl Elo {
    fn k_of(&self, r: &Rating) -> f64 {
        if r.sigma > 0.0 {
            self.k_provisional
        } else {
            self.k
        }
    }

    fn after_game(&self, r: &Rating, delta: f64) -> Rating {
        let step = 1.0 / f64::from(self.provisional_games.max(1));
        let left = r.sigma - step;
        Rating {
            mu: r.mu + delta,
            // Snap to zero so ten steps of 0.1 close the window exactly.
            sigma: if left < step * 0.5 { 0.0 } else { left },
            games: r.games.saturating_add(1),
        }
    }
}

impl RatingSystem for Elo {
    fn name(&self) -> &'static str {
        "elo"
    }

    fn initial(&self) -> Rating {
        let sigma = if self.provisional_games == 0 {
            0.0
        } else {
            1.0
        };
        Rating {
            mu: self.start,
            sigma,
            games: 0,
        }
    }

    /// `R' = R + K (S - E)`. Both teams move by the same amount in opposite directions, using the mean
    /// of their two K-factors, so the sum of all ratings never changes (no inflation from newcomers).
    fn update(&self, a: &Rating, b: &Rating, outcome: Outcome) -> (Rating, Rating) {
        let k = 0.5 * (self.k_of(a) + self.k_of(b));
        let delta = k * (outcome.score_a() - self.expected_score(a, b));
        (self.after_game(a, delta), self.after_game(b, -delta))
    }

    /// `E_A = 1 / (1 + 10^((R_B - R_A) / scale))`.
    fn expected_score(&self, a: &Rating, b: &Rating) -> f64 {
        1.0 / (1.0 + 10f64.powf((b.mu - a.mu) / self.scale))
    }

    fn display(&self, r: &Rating) -> f64 {
        r.mu
    }

    fn is_provisional(&self, r: &Rating) -> bool {
        r.sigma > 0.0
    }

    fn uncertainty(&self, r: &Rating) -> f64 {
        r.sigma.clamp(0.0, 1.0)
    }

    fn on_new_submission(&self, r: &Rating) -> Rating {
        let reopened = if self.provisional_games == 0 {
            0.0
        } else {
            self.resubmission_window
        };
        Rating {
            sigma: r.sigma.max(reopened.clamp(0.0, 1.0)),
            ..*r
        }
    }

    /// Elo has no variance to inflate, so a factor `f` reopens `f - 1` of the provisional window
    /// (1.5 reopens half of it), capped at the whole window.
    fn on_balance_patch(&self, r: &Rating, factor: f64) -> Rating {
        // A reopening shorter than half a game is nothing: a 1 % patch must not double K.
        let half_game = 0.5 / f64::from(self.provisional_games.max(1));
        if self.provisional_games == 0 || factor.is_nan() || factor - 1.0 < half_game {
            return *r;
        }
        Rating {
            sigma: r.sigma.max((factor - 1.0).min(1.0)),
            ..*r
        }
    }

    /// No-op: plain Elo does not model time.
    fn on_inactive(&self, r: &Rating, _periods: u32) -> Rating {
        *r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textbook_numbers() {
        let elo = Elo {
            provisional_games: 0,
            ..Elo::default()
        };
        let a = Rating {
            mu: 1700.0,
            ..elo.initial()
        };
        let b = Rating {
            mu: 1500.0,
            ..elo.initial()
        };
        let e = elo.expected_score(&a, &b);
        assert!((e - 0.759_746_9).abs() < 1e-6, "{e}");
        let (na, nb) = elo.update(&a, &b, Outcome::BWins);
        assert!((na.mu - (1700.0 - 24.0 * e)).abs() < 1e-9);
        assert!((na.mu + nb.mu - 3200.0).abs() < 1e-9);
        let (da, db) = elo.update(&a, &a, Outcome::Draw);
        assert_eq!((da.mu, db.mu), (1700.0, 1700.0));
    }

    #[test]
    fn provisional_window_closes_after_n_games() {
        let elo = Elo::default();
        let (mut a, mut b) = (elo.initial(), elo.initial());
        let (first, _) = elo.update(&a, &b, Outcome::AWins);
        assert!((first.mu - 1524.0).abs() < 1e-9, "provisional K is 48");
        for g in 0..10 {
            assert!(elo.is_provisional(&a), "game {g}");
            (a, b) = elo.update(&a, &b, Outcome::Draw);
        }
        assert!(!elo.is_provisional(&a));
        assert_eq!(a.games, 10);
        let re = elo.on_new_submission(&a);
        assert_eq!((re.mu, re.sigma, re.games), (a.mu, 0.5, 10));
        assert_eq!(elo.on_balance_patch(&a, 1.25).sigma, 0.25);
        assert_eq!(elo.on_balance_patch(&re, 1.25).sigma, 0.5);
        assert_eq!(
            elo.on_balance_patch(&a, 1.01),
            a,
            "a 1 % patch does not reopen the window"
        );
        assert_eq!(elo.on_inactive(&a, 9), a);
    }
}
