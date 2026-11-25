//! The seam where a rating system plugs in. The coordinator only needs "two ratings and a game result in, two
//! ratings out"; `ibc-rating` (a separate crate) implements `RatingHook` and is passed to `App::new`.
//! The built-in `Elo` keeps the ladder working without it.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rating {
    /// The skill estimate; the ladder is sorted by `RatingHook::display`.
    pub mu: f64,
    /// Uncertainty, for systems that track one (Glicko, TrueSkill). Elo leaves it at 0.
    pub sigma: f64,
    /// Rated games played.
    pub games: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    WinA,
    WinB,
    Draw,
}

pub trait RatingHook: Send + Sync {
    fn name(&self) -> &str;
    /// The rating of a team that has not played.
    fn initial(&self) -> Rating;
    /// New ratings of A and B after one game.
    fn update(&self, a: &Rating, b: &Rating, outcome: Outcome) -> (Rating, Rating);
    /// The number shown on the ladder and used to pair rating-adjacent teams.
    fn display(&self, r: &Rating) -> f64 {
        r.mu
    }
}

/// Plain Elo: logistic expectation on a 400-point scale, fixed K.
pub struct Elo {
    pub k: f64,
    pub start: f64,
}

impl Default for Elo {
    fn default() -> Elo {
        Elo {
            k: 24.0,
            start: 1500.0,
        }
    }
}

impl RatingHook for Elo {
    fn name(&self) -> &str {
        "elo"
    }

    fn initial(&self) -> Rating {
        Rating {
            mu: self.start,
            sigma: 0.0,
            games: 0,
        }
    }

    fn update(&self, a: &Rating, b: &Rating, outcome: Outcome) -> (Rating, Rating) {
        let expected_a = 1.0 / (1.0 + 10f64.powf((b.mu - a.mu) / 400.0));
        let score_a = match outcome {
            Outcome::WinA => 1.0,
            Outcome::WinB => 0.0,
            Outcome::Draw => 0.5,
        };
        let delta = self.k * (score_a - expected_a);
        (
            Rating {
                mu: a.mu + delta,
                sigma: a.sigma,
                games: a.games + 1,
            },
            Rating {
                mu: b.mu - delta,
                sigma: b.sigma,
                games: b.games + 1,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elo_is_zero_sum_and_moves_the_right_way() {
        let elo = Elo::default();
        let (a, b) = elo.update(&elo.initial(), &elo.initial(), Outcome::WinA);
        assert_eq!(a.mu, 1512.0);
        assert_eq!(b.mu, 1488.0);
        assert_eq!((a.games, b.games), (1, 1));
        let (a2, b2) = elo.update(&a, &b, Outcome::Draw);
        assert!(a2.mu < a.mu && b2.mu > b.mu);
        assert!((a2.mu + b2.mu - 3000.0).abs() < 1e-9);
    }
}
