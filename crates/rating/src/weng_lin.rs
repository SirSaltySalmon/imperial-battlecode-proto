//! Weng & Lin, "A Bayesian Approximation Method for Online Ranking", JMLR 12 (2011) 267-300:
//! Algorithm 1 (Bradley-Terry, full pair), specialised to two teams of one rated entity each.
//! This is the model OpenSkill calls `BradleyTerryFull`.

use crate::{Outcome, Rating, RatingSystem};

/// Weng-Lin configuration. A rating is a Gaussian belief `N(mu, sigma^2)` over the team's skill.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WengLin {
    /// Initial mean. Paper section 6.1: 25.
    pub mu0: f64,
    /// Initial standard deviation. Paper section 6.1: 25/3.
    pub sigma0: f64,
    /// Performance noise: a game is decided by skill plus `N(0, beta^2)`. Paper section 6.1: 25/6.
    pub beta: f64,
    /// Lower bound of the variance shrink factor, eq. (28) and section 6.1: 0.0001.
    pub kappa: f64,
    /// Additive dynamics: `sigma^2 += tau^2` before every game, so sigma never collapses and a rating
    /// can still follow a team that changes. Not in the paper; this is OpenSkill's `tau`, default 25/300.
    pub tau: f64,
    /// Sigma growth per idle rating period: `sigma^2 += periods * inactivity_tau^2`, capped at sigma0.
    pub inactivity_tau: f64,
    /// A new submission sets `sigma = max(sigma, resubmission_sigma_fraction * sigma0)`.
    pub resubmission_sigma_fraction: f64,
    /// A rating is provisional while `sigma > provisional_sigma_fraction * sigma0`.
    pub provisional_sigma_fraction: f64,
    /// Display value of a team with `mu - display_z * sigma == mu0`.
    pub display_center: f64,
    /// Display points per unit of mu. The default makes a display gap of 400 mean 10:1 odds between
    /// two settled teams, like Elo: `400 / (sqrt(2) * beta * ln 10)`, about 29.48.
    pub display_points_per_mu: f64,
    /// Standard deviations subtracted before display. 0: the ladder shows the estimate itself, so a new
    /// team starts at `display_center`. TrueSkill's conservative 3 would show a new team near 760 and
    /// make every team climb for as long as its sigma shrinks, which a bot (fixed until its next upload)
    /// has no reason to do.
    pub display_z: f64,
}

impl Default for WengLin {
    fn default() -> Self {
        let beta = 25.0 / 6.0;
        WengLin {
            mu0: 25.0,
            sigma0: 25.0 / 3.0,
            beta,
            kappa: 0.0001,
            tau: 25.0 / 300.0,
            inactivity_tau: 25.0 / 300.0,
            resubmission_sigma_fraction: 0.85,
            provisional_sigma_fraction: 0.6,
            display_center: 1500.0,
            display_points_per_mu: 400.0
                / (std::f64::consts::SQRT_2 * beta * std::f64::consts::LN_10),
            display_z: 0.0,
        }
    }
}

impl WengLin {
    /// Eq. (49): `c_iq = sqrt(sigma_i^2 + sigma_q^2 + 2 beta^2)`.
    fn c(&self, var_a: f64, var_b: f64) -> f64 {
        (var_a + var_b + 2.0 * self.beta * self.beta).sqrt()
    }

    /// Eq. (49): `p_iq = e^(mu_i/c) / (e^(mu_i/c) + e^(mu_q/c))`, written as a logistic so that it
    /// cannot overflow.
    fn p(mu_a: f64, mu_b: f64, c: f64) -> f64 {
        1.0 / (1.0 + ((mu_b - mu_a) / c).exp())
    }

    /// Steps 3.1 and 3.2 of Algorithm 1 for one side (`i` = this team, `q` = the opponent).
    /// `var` values already include the `tau^2` inflation.
    fn side(&self, r: &Rating, var_i: f64, mu_q: f64, var_q: f64, s: f64) -> Rating {
        let c = self.c(var_i, var_q);
        let p_iq = Self::p(r.mu, mu_q, c);
        let p_qi = 1.0 - p_iq;
        // Eq. (50): delta_q = sigma_i^2 / c_iq * (s - p_iq), with s = 1 (win), 1/2 (draw), 0 (loss).
        let delta = var_i / c * (s - p_iq);
        // Section 6.1: gamma_q = sigma_i / c_iq for BT-full.
        let gamma = var_i.sqrt() / c;
        // Eq. (50): eta_q = gamma_q * (sigma_i / c_iq)^2 * p_iq * p_qi.
        let eta = gamma * (var_i / (c * c)) * p_iq * p_qi;
        // Step 3.1.2 with a single opponent: Omega_i = delta_q, Delta_i = eta_q.
        // Step 3.2 with a single member (sigma_ij^2 / sigma_i^2 = 1):
        //   mu <- mu + Omega_i,   sigma^2 <- sigma^2 * max(1 - Delta_i, kappa).
        Rating {
            mu: r.mu + delta,
            sigma: (var_i * (1.0 - eta).max(self.kappa)).sqrt(),
            games: r.games.saturating_add(1),
        }
    }

    /// The displayed estimate `mu - z * sigma` (`mu` itself by default).
    fn shown(&self, r: &Rating) -> f64 {
        r.mu - self.display_z * r.sigma
    }
}

impl RatingSystem for WengLin {
    fn name(&self) -> &'static str {
        "weng-lin-bt"
    }

    fn initial(&self) -> Rating {
        Rating {
            mu: self.mu0,
            sigma: self.sigma0,
            games: 0,
        }
    }

    fn update(&self, a: &Rating, b: &Rating, outcome: Outcome) -> (Rating, Rating) {
        let tau2 = self.tau * self.tau;
        let var_a = a.sigma * a.sigma + tau2;
        let var_b = b.sigma * b.sigma + tau2;
        let s_a = outcome.score_a();
        (
            self.side(a, var_a, b.mu, var_b, s_a),
            self.side(b, var_b, a.mu, var_a, 1.0 - s_a),
        )
    }

    /// Eq. (48)/(49) with the current beliefs. Under eq. (51) a draw is the geometric mean of the two
    /// win probabilities, which is symmetric, so this is also the expected score.
    fn expected_score(&self, a: &Rating, b: &Rating) -> f64 {
        Self::p(a.mu, b.mu, self.c(a.sigma * a.sigma, b.sigma * b.sigma))
    }

    /// `display = center + points_per_mu * ((mu - z * sigma) - mu0)`.
    fn display(&self, r: &Rating) -> f64 {
        self.display_center + self.display_points_per_mu * (self.shown(r) - self.mu0)
    }

    fn is_provisional(&self, r: &Rating) -> bool {
        r.sigma > self.provisional_sigma_fraction * self.sigma0
    }

    fn uncertainty(&self, r: &Rating) -> f64 {
        (r.sigma / self.sigma0).clamp(0.0, 1.0)
    }

    fn on_new_submission(&self, r: &Rating) -> Rating {
        Rating {
            sigma: r.sigma.max(self.resubmission_sigma_fraction * self.sigma0),
            ..*r
        }
    }

    /// `sigma <- min(sigma * factor, sigma0)`; a sigma already above sigma0 is left alone.
    fn on_balance_patch(&self, r: &Rating, factor: f64) -> Rating {
        if factor.is_nan() || factor <= 1.0 {
            return *r;
        }
        Rating {
            sigma: (r.sigma * factor).min(self.sigma0.max(r.sigma)),
            ..*r
        }
    }

    /// `sigma^2 <- min(sigma^2 + periods * inactivity_tau^2, sigma0^2)`.
    fn on_inactive(&self, r: &Rating, periods: u32) -> Rating {
        let grown = (r.sigma * r.sigma
            + f64::from(periods) * self.inactivity_tau * self.inactivity_tau)
            .sqrt();
        Rating {
            sigma: grown.min(self.sigma0.max(r.sigma)),
            ..*r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(x: f64, y: f64) -> bool {
        (x - y).abs() < 1e-9
    }

    /// Hand evaluation of Algorithm 1 for two new teams with tau = 0:
    /// c = sqrt(2 * (25/3)^2 + 2 * (25/6)^2), p = 1/2, delta = sigma^2 / (2c),
    /// eta = (sigma/c)^3 / 4.
    #[test]
    fn first_game_matches_algorithm_1() {
        let wl = WengLin {
            tau: 0.0,
            ..WengLin::default()
        };
        let r = wl.initial();
        let var = r.sigma * r.sigma;
        let c = (2.0 * var + 2.0 * wl.beta * wl.beta).sqrt();
        let (a, b) = wl.update(&r, &r, Outcome::AWins);
        assert!(close(a.mu, 25.0 + var / (2.0 * c)));
        assert!(close(b.mu, 25.0 - var / (2.0 * c)));
        let shrink = 1.0 - (r.sigma / c).powi(3) / 4.0;
        assert!(close(a.sigma, (var * shrink).sqrt()));
        assert!(close(a.sigma, b.sigma));
        // The same numbers written out: 25 + 2.6352, and 8.3333 * sqrt(1 - 0.063246).
        assert!((a.mu - 27.635_23).abs() < 1e-4, "{}", a.mu);
        assert!((a.sigma - 8.065_51).abs() < 1e-4, "{}", a.sigma);
    }

    #[test]
    fn draws_pull_together_and_upsets_move_more() {
        let wl = WengLin::default();
        let strong = Rating {
            mu: 30.0,
            sigma: 2.0,
            games: 50,
        };
        let weak = Rating {
            mu: 20.0,
            sigma: 2.0,
            games: 50,
        };
        let (s, w) = wl.update(&strong, &weak, Outcome::Draw);
        assert!(s.mu < strong.mu && w.mu > weak.mu);
        let (exp_s, _) = wl.update(&strong, &weak, Outcome::AWins);
        let (ups_s, _) = wl.update(&strong, &weak, Outcome::BWins);
        assert!((strong.mu - ups_s.mu) > (exp_s.mu - strong.mu));
        assert!(wl.expected_score(&strong, &weak) > 0.8);
        assert!(close(
            wl.expected_score(&strong, &weak) + wl.expected_score(&weak, &strong),
            1.0
        ));
    }

    #[test]
    fn sigma_events() {
        let wl = WengLin::default();
        let settled = Rating {
            mu: 31.0,
            sigma: 1.5,
            games: 300,
        };
        let re = wl.on_new_submission(&settled);
        assert_eq!(re.mu, 31.0);
        assert!(close(re.sigma, 0.85 * wl.sigma0));
        assert_eq!(
            wl.on_new_submission(&wl.initial()),
            wl.initial(),
            "never lowers sigma"
        );
        assert!(close(wl.on_balance_patch(&settled, 1.5).sigma, 2.25));
        assert!(close(
            wl.on_balance_patch(&wl.initial(), 1.5).sigma,
            wl.sigma0
        ));
        assert_eq!(wl.on_balance_patch(&settled, 0.5), settled);
        let idle = wl.on_inactive(&settled, 4);
        assert!(close(idle.sigma * idle.sigma, 2.25 + 4.0 * wl.tau * wl.tau));
        assert!(wl.is_provisional(&wl.initial()) && !wl.is_provisional(&settled));
        assert!(
            wl.is_provisional(&re),
            "a new bot is uncertain again until it has played"
        );
    }

    #[test]
    fn display_mapping() {
        let wl = WengLin::default();
        let certain_mean = Rating {
            mu: 25.0,
            sigma: 0.0,
            games: 0,
        };
        assert!(close(wl.display(&certain_mean), 1500.0));
        // 400 display points = 10:1 odds between two settled teams.
        let better = Rating {
            mu: 25.0 + 400.0 / wl.display_points_per_mu,
            sigma: 0.0,
            games: 0,
        };
        assert!(close(
            wl.expected_score(&better, &certain_mean),
            10.0 / 11.0
        ));
        assert!(
            close(wl.display(&wl.initial()), 1500.0),
            "a new team starts at the centre"
        );
        let conservative = WengLin {
            display_z: 3.0,
            ..WengLin::default()
        };
        assert!(conservative.display(&wl.initial()) < 800.0);
    }
}
