//! Rating a finished series in one step.
//!
//! The ladder moves on maps won and lost and on the rating gap, never on the round score inside a map or
//! on the order the maps finished in. Every map's update is computed from the **pre-series** ratings and
//! the mean shifts are summed, so a 3-0 moves more than a 2-1, a drawn map counts a half (the rating
//! system's own draw score), and any permutation of the same maps gives the same result.

use crate::{Outcome, Rating, RatingSystem};

/// New ratings of (A, B) after a series whose maps ended with `outcomes` (seen from A, any order).
///
/// * `mu`: the prior plus the sum, over maps, of the shift that map alone would cause from the prior.
/// * `sigma`: the uncertainty one map at a time, as if every map were a draw, with `mu` held at the
///   prior. In both systems the uncertainty step does not depend on who won (Weng-Lin's variance shrink
///   depends only on the expected score; Elo closes its provisional window one game at a time), so this is
///   the per-game uncertainty after `outcomes.len()` games, and order-independent by construction.
/// * `games`: the prior plus `outcomes.len()`.
///
/// An empty series returns the priors unchanged.
pub fn rate_series(
    system: &dyn RatingSystem,
    a: &Rating,
    b: &Rating,
    outcomes: &[Outcome],
) -> (Rating, Rating) {
    let (mut shift_a, mut shift_b) = (0.0, 0.0);
    let (mut unc_a, mut unc_b) = (*a, *b);
    for &outcome in outcomes {
        let (na, nb) = system.update(a, b, outcome);
        shift_a += na.mu - a.mu;
        shift_b += nb.mu - b.mu;
        let held_a = Rating { mu: a.mu, ..unc_a };
        let held_b = Rating { mu: b.mu, ..unc_b };
        (unc_a, unc_b) = system.update(&held_a, &held_b, Outcome::Draw);
    }
    let finish = |prior: &Rating, shift: f64, unc: Rating| Rating {
        mu: prior.mu + shift,
        sigma: unc.sigma,
        games: unc.games,
    };
    (finish(a, shift_a, unc_a), finish(b, shift_b, unc_b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Elo, WengLin};
    use Outcome::{AWins, BWins, Draw};

    fn close(x: f64, y: f64) -> bool {
        (x - y).abs() < 1e-9
    }

    fn settled(mu: f64) -> Rating {
        Rating {
            mu,
            sigma: 2.0,
            games: 40,
        }
    }

    fn systems() -> [Box<dyn RatingSystem>; 2] {
        [Box::new(WengLin::default()), Box::new(Elo::default())]
    }

    #[test]
    fn the_order_of_the_maps_does_not_matter() {
        for system in systems() {
            let s = system.as_ref();
            let (a, b) = (s.initial(), settled(s.initial().mu + 3.0));
            let base = rate_series(s, &a, &b, &[AWins, BWins, Draw, AWins]);
            for order in [
                [BWins, AWins, AWins, Draw],
                [Draw, AWins, BWins, AWins],
                [AWins, AWins, Draw, BWins],
            ] {
                let other = rate_series(s, &a, &b, &order);
                assert!(close(base.0.mu, other.0.mu) && close(base.1.mu, other.1.mu));
                assert!(close(base.0.sigma, other.0.sigma) && close(base.1.sigma, other.1.sigma));
            }
            assert_eq!((base.0.games, base.1.games), (4, 44));
        }
    }

    #[test]
    fn three_nil_moves_more_than_two_one() {
        for system in systems() {
            let s = system.as_ref();
            let (a, b) = (s.initial(), s.initial());
            let sweep = rate_series(s, &a, &b, &[AWins, AWins, AWins]);
            let close_win = rate_series(s, &a, &b, &[AWins, BWins, AWins]);
            assert!(sweep.0.mu > close_win.0.mu && close_win.0.mu > a.mu);
            assert!(sweep.1.mu < close_win.1.mu && close_win.1.mu < b.mu);
            // Between even teams a 2-1 is one net map: the same mean shift as a 1-0.
            let one_nil = rate_series(s, &a, &b, &[AWins]);
            assert!(close(close_win.0.mu, one_nil.0.mu));
        }
    }

    #[test]
    fn a_drawn_map_is_half_and_the_shift_uses_the_prior() {
        let s = WengLin::default();
        let (a, b) = (settled(25.0), settled(29.0));
        let (win, _) = s.update(&a, &b, AWins);
        let (loss, _) = s.update(&a, &b, BWins);
        let (draw, _) = s.update(&a, &b, Draw);
        let (series, _) = rate_series(&s, &a, &b, &[AWins, Draw, BWins]);
        let expected = (win.mu - a.mu) + (draw.mu - a.mu) + (loss.mu - a.mu);
        assert!(close(series.mu - a.mu, expected));
        // A draw sits half-way between a win and a loss.
        assert!(close(draw.mu - loss.mu, win.mu - draw.mu));
        // An upset draw against a stronger team still gains.
        assert!(rate_series(&s, &a, &b, &[Draw]).0.mu > a.mu);
    }

    #[test]
    fn uncertainty_shrinks_per_map_and_an_empty_series_changes_nothing() {
        let s = WengLin::default();
        let r = s.initial();
        let one = rate_series(&s, &r, &r, &[AWins]).0.sigma;
        let three = rate_series(&s, &r, &r, &[AWins, AWins, BWins]).0.sigma;
        assert!(three < one && one < r.sigma);
        assert_eq!(rate_series(&s, &r, &r, &[]), (r, r));
        // Elo: the sum is conserved and the provisional window closes one game at a time.
        let elo = Elo::default();
        let (x, y) = rate_series(&elo, &elo.initial(), &settled(1600.0), &[AWins, AWins]);
        assert!(close(x.mu + y.mu, 1500.0 + 1600.0));
        assert!(close(x.sigma, 0.8));
    }
}
