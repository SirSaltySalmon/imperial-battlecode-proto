//! Statistics used by the reports. Published methods only: Wilson score interval, exact sign test,
//! Wald's SPRT with the normal (GSPRT) approximation used by chess engine testing, Bradley-Terry by
//! minorisation-maximisation, cluster-robust ratio intervals. Floats are fine here: nothing in this crate
//! feeds back into a simulation.

use serde::{Deserialize, Serialize};

/// Two-sided 95 % normal quantile.
pub const Z95: f64 = 1.959963984540054;

/// A point estimate with an optional 95 % interval. `lo`/`hi` are `None` when the sample cannot support one.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Est {
    pub value: f64,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// Independent observations behind the interval (games, pairs or matches; never rounds).
    pub n: u64,
}

impl Est {
    pub fn point(value: f64, n: u64) -> Est {
        Est {
            value,
            lo: None,
            hi: None,
            n,
        }
    }
    pub fn half_width(&self) -> Option<f64> {
        Some((self.hi? - self.lo?) / 2.0)
    }
    /// True when `x` lies strictly outside the interval. Without an interval nothing can be excluded.
    pub fn excludes(&self, x: f64) -> bool {
        matches!((self.lo, self.hi), (Some(lo), Some(hi)) if x < lo || x > hi)
    }
    /// Clamp the interval (not the value) to a range, e.g. 0..=1 for a rate.
    pub fn clamped(mut self, min: f64, max: f64) -> Est {
        self.lo = self.lo.map(|v| v.clamp(min, max));
        self.hi = self.hi.map(|v| v.clamp(min, max));
        self
    }
}

/// Wilson score interval for a proportion `p_hat` observed over `n` trials.
pub fn wilson(p_hat: f64, n: f64, z: f64) -> (f64, f64) {
    if n <= 0.0 {
        return (0.0, 1.0);
    }
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let centre = (p_hat + z2 / (2.0 * n)) / denom;
    let half = z * ((p_hat * (1.0 - p_hat) + z2 / (4.0 * n)) / n).sqrt() / denom;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

fn ln_choose(n: u64, k: u64) -> f64 {
    let k = k.min(n - k);
    (0..k)
        .map(|i| ((n - i) as f64).ln() - ((i + 1) as f64).ln())
        .sum()
}

/// Exact two-sided sign test: probability, under a fair coin, of a split at least as lopsided as
/// `wins` out of `n` (ties must be dropped by the caller). 1.0 for `n == 0`.
pub fn sign_test_p(wins: u64, n: u64) -> f64 {
    if n == 0 {
        return 1.0;
    }
    let k = wins.min(n - wins.min(n));
    let ln_half_n = -(n as f64) * std::f64::consts::LN_2;
    let tail: f64 = (0..=k).map(|i| (ln_choose(n, i) + ln_half_n).exp()).sum();
    (2.0 * tail).min(1.0)
}

fn erf(x: f64) -> f64 {
    if x < 0.0 {
        return -erf(-x);
    }
    if x < 3.0 {
        // Maclaurin series; accurate to about 1e-12 on this range.
        let (mut term, mut sum, mut n) = (x, x, 0.0f64);
        while term.abs() > 1e-17 && n < 200.0 {
            n += 1.0;
            term *= -x * x / n;
            sum += term / (2.0 * n + 1.0);
        }
        return sum * 2.0 / std::f64::consts::PI.sqrt();
    }
    // Asymptotic expansion of erfc; absolute error below 1e-7 from x = 3.
    let x2 = x * x;
    let series = 1.0 - 1.0 / (2.0 * x2) + 3.0 / (4.0 * x2 * x2) - 15.0 / (8.0 * x2 * x2 * x2);
    1.0 - (-x2).exp() / (x * std::f64::consts::PI.sqrt()) * series
}

pub fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

/// Inverse normal CDF (Acklam's rational approximation, relative error about 1e-9).
pub fn normal_quantile(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383577518672690e+02,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];
    let p = p.clamp(1e-300, 1.0 - 1e-16);
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < 0.02425 {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - 0.02425 {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

/// Two-sided 95 % critical value of Student's t with `df` degrees of freedom.
pub fn t95(df: u64) -> f64 {
    const SMALL: [f64; 5] = [12.706, 4.303, 3.182, 2.776, 2.571];
    if df == 0 {
        return f64::INFINITY;
    }
    if let Some(&t) = SMALL.get(df as usize - 1) {
        return t;
    }
    // Cornish-Fisher expansion around the normal quantile.
    let (z, v) = (Z95, df as f64);
    let (z3, z5, z7) = (z.powi(3), z.powi(5), z.powi(7));
    z + (z3 + z) / (4.0 * v)
        + (5.0 * z5 + 16.0 * z3 + 3.0 * z) / (96.0 * v * v)
        + (3.0 * z7 + 19.0 * z5 + 17.0 * z3 - 15.0 * z) / (384.0 * v * v * v)
}

/// Mean and unbiased sample variance (0 for fewer than two values).
pub fn mean_var(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    if xs.is_empty() {
        return (0.0, 0.0);
    }
    let mean = xs.iter().sum::<f64>() / n;
    let var = if xs.len() < 2 {
        0.0
    } else {
        xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)
    };
    (mean, var)
}

/// Mean of independent observations with a t interval. No interval for fewer than two observations.
pub fn mean_ci(xs: &[f64]) -> Est {
    let (mean, var) = mean_var(xs);
    let n = xs.len() as u64;
    if n < 2 {
        return Est::point(mean, n);
    }
    let half = t95(n - 1) * (var / n as f64).sqrt();
    Est {
        value: mean,
        lo: Some(mean - half),
        hi: Some(mean + half),
        n,
    }
}

/// Ratio of sums `sum(num) / sum(den)` over independent clusters (one `(num, den)` per match), with the
/// linearisation (delta method) interval. This is how a per-round rate gets an honest interval: rounds
/// inside one match are not independent, matches are.
pub fn ratio_ci(clusters: &[(f64, f64)]) -> Option<Est> {
    let den: f64 = clusters.iter().map(|c| c.1).sum();
    if den <= 0.0 {
        return None;
    }
    let r = clusters.iter().map(|c| c.0).sum::<f64>() / den;
    let n = clusters.len() as u64;
    if n < 2 {
        return Some(Est::point(r, n));
    }
    let ss: f64 = clusters.iter().map(|c| (c.0 - r * c.1).powi(2)).sum();
    let se = (ss * n as f64 / (n as f64 - 1.0)).sqrt() / den;
    let half = t95(n - 1) * se;
    Some(Est {
        value: r,
        lo: Some(r - half),
        hi: Some(r + half),
        n,
    })
}

/// Linear-interpolated percentile (`p` in 0..=100) of an ascending slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = (p / 100.0).clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let (i, frac) = (pos.floor() as usize, pos.fract());
    let next = sorted[(i + 1).min(sorted.len() - 1)];
    sorted[i] + (next - sorted[i]) * frac
}

/// Expected score of a player `elo` points above the opponent (logistic model).
pub fn elo_to_score(elo: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf(-elo / 400.0))
}

/// Inverse of `elo_to_score`. Scores are clamped so a clean sweep reads as about +/-1200, not infinity.
pub fn score_to_elo(score: f64) -> f64 {
    let s = score.clamp(0.001, 0.999);
    -400.0 * (1.0 / s - 1.0).log10()
}

/// Sequential probability ratio test of H0 "elo = elo0" against H1 "elo = elo1".
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sprt {
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SprtDecision {
    AcceptH0,
    AcceptH1,
    Continue,
}

impl Sprt {
    /// Wald's stopping bounds on the log-likelihood ratio: (lower, upper).
    pub fn bounds(&self) -> (f64, f64) {
        (
            (self.beta / (1.0 - self.alpha)).ln(),
            ((1.0 - self.beta) / self.alpha).ln(),
        )
    }

    /// Log-likelihood ratio of the observed scores (each in 0..=1, one per independent pair) under the
    /// normal approximation: `n (s1 - s0) (2 mean - s0 - s1) / (2 var)`. The variance is floored at
    /// `0.25 / n` so a short run of identical results cannot produce unbounded evidence.
    pub fn llr(&self, scores: &[f64]) -> f64 {
        let n = scores.len() as f64;
        if scores.len() < 2 {
            return 0.0;
        }
        let mean = scores.iter().sum::<f64>() / n;
        let var = (scores.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).max(0.25 / n);
        let (s0, s1) = (elo_to_score(self.elo0), elo_to_score(self.elo1));
        n * (s1 - s0) * (2.0 * mean - s0 - s1) / (2.0 * var)
    }

    pub fn decide(&self, llr: f64) -> SprtDecision {
        let (lower, upper) = self.bounds();
        if llr >= upper {
            SprtDecision::AcceptH1
        } else if llr <= lower {
            SprtDecision::AcceptH0
        } else {
            SprtDecision::Continue
        }
    }
}

/// SplitMix64: a tiny seeded generator so bootstrap intervals are reproducible.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// Bradley-Terry strengths by the MM algorithm, returned as Elo-scaled ratings centred on 0.
/// `wins[i][j]` = points `i` scored against `j` (a draw is half a point each). One virtual draw between
/// every pair keeps the ratings finite when a bot wins or loses everything.
pub fn bradley_terry(wins: &[Vec<f64>]) -> Vec<f64> {
    let n = wins.len();
    if n == 0 {
        return Vec::new();
    }
    let w = |i: usize, j: usize| wins[i][j] + 0.5;
    let mut p = vec![1.0f64; n];
    for _ in 0..500 {
        let mut next = vec![0.0; n];
        for i in 0..n {
            let (mut won, mut denom) = (0.0, 0.0);
            for j in (0..n).filter(|&j| j != i) {
                won += w(i, j);
                denom += (w(i, j) + w(j, i)) / (p[i] + p[j]);
            }
            next[i] = if denom > 0.0 { won / denom } else { 1.0 };
        }
        let geo = (next.iter().map(|x| x.ln()).sum::<f64>() / n as f64).exp();
        let moved = next
            .iter()
            .zip(&p)
            .map(|(a, b)| (a / geo - b).abs())
            .fold(0.0, f64::max);
        p = next.iter().map(|x| x / geo).collect();
        if moved < 1e-10 {
            break;
        }
    }
    p.iter().map(|x| 400.0 * x.log10()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn wilson_known_values() {
        // 7 of 10 at 95 %: the textbook Wilson interval is [0.3968, 0.8922].
        let (lo, hi) = wilson(0.7, 10.0, Z95);
        assert!(
            close(lo, 0.3968, 5e-4) && close(hi, 0.8922, 5e-4),
            "{lo} {hi}"
        );
        // 0 of 10: lower bound is exactly 0, upper is z^2 / (n + z^2) = 0.2775.
        let (lo, hi) = wilson(0.0, 10.0, Z95);
        assert!(lo == 0.0 && close(hi, 0.2775, 5e-4), "{lo} {hi}");
        assert_eq!(wilson(0.5, 0.0, Z95), (0.0, 1.0));
    }

    #[test]
    fn sign_test_known_values() {
        assert!(close(sign_test_p(1, 10), 22.0 / 1024.0, 1e-12));
        assert!(close(sign_test_p(9, 10), 22.0 / 1024.0, 1e-12));
        assert!(close(sign_test_p(0, 5), 2.0 / 32.0, 1e-12));
        assert_eq!(sign_test_p(5, 10), 1.0);
        assert_eq!(sign_test_p(0, 0), 1.0);
        // 60 of 100: the exact two-sided p is 0.0569.
        assert!(close(sign_test_p(60, 100), 0.0569, 5e-4));
    }

    #[test]
    fn normal_and_t() {
        assert!(close(normal_cdf(0.0), 0.5, 1e-12));
        assert!(close(normal_cdf(1.96), 0.975002, 1e-6));
        assert!(close(normal_cdf(-3.5), 0.000232629, 1e-7));
        assert!(close(normal_quantile(0.975), Z95, 1e-7));
        assert!(close(normal_quantile(0.01), -2.326348, 1e-6));
        assert!(close(t95(9), 2.262, 2e-3));
        assert!(close(t95(30), 2.042, 2e-3));
        assert!(close(t95(2), 4.303, 1e-9));
    }

    #[test]
    fn sprt_boundaries_and_decisions() {
        let s = Sprt {
            elo0: 0.0,
            elo1: 20.0,
            alpha: 0.05,
            beta: 0.05,
        };
        let (lo, hi) = s.bounds();
        assert!(close(lo, -(19.0f64).ln(), 1e-12) && close(hi, (19.0f64).ln(), 1e-12));
        assert!(close(hi, 2.944439, 1e-6));
        let s2 = Sprt {
            elo0: 0.0,
            elo1: 10.0,
            alpha: 0.05,
            beta: 0.2,
        };
        assert!(
            close(s2.bounds().0, (0.2f64 / 0.95).ln(), 1e-12)
                && close(s2.bounds().1, 16f64.ln(), 1e-12)
        );

        assert_eq!(s.decide(3.0), SprtDecision::AcceptH1);
        assert_eq!(s.decide(-3.0), SprtDecision::AcceptH0);
        assert_eq!(s.decide(0.0), SprtDecision::Continue);
        // Exactly between the hypotheses there is no evidence either way.
        let mid = (elo_to_score(0.0) + elo_to_score(20.0)) / 2.0;
        assert!(s.llr(&[mid, mid, mid, mid]).abs() < 1e-12);
        // A strong run of wins accepts H1, a run of losses accepts H0.
        assert_eq!(s.decide(s.llr(&[1.0; 12])), SprtDecision::AcceptH1);
        assert_eq!(s.decide(s.llr(&[0.0; 12])), SprtDecision::AcceptH0);
        // Hand-computed: n = 4, mean = 0.75, var = 0.0625, s0 = 0.5, s1 = 0.528751.
        let llr = s.llr(&[1.0, 0.5, 1.0, 0.5]);
        let s1 = elo_to_score(20.0);
        assert!(close(
            llr,
            4.0 * (s1 - 0.5) * (1.5 - 0.5 - s1) / (2.0 * 0.0625),
            1e-12
        ));
    }

    #[test]
    fn elo_round_trip() {
        assert!(close(elo_to_score(0.0), 0.5, 1e-12));
        assert!(close(elo_to_score(400.0), 10.0 / 11.0, 1e-12));
        assert!(close(score_to_elo(elo_to_score(123.0)), 123.0, 1e-9));
    }

    #[test]
    fn ratio_interval() {
        // Identical clusters: zero-width interval at the ratio.
        let e = ratio_ci(&[(3.0, 6.0), (3.0, 6.0), (3.0, 6.0)]).unwrap();
        assert!(close(e.value, 0.5, 1e-12) && close(e.half_width().unwrap(), 0.0, 1e-12));
        // With equal denominators the ratio interval is the t interval of the per-cluster rates.
        let rates = [0.2, 0.5, 0.8, 0.5];
        let e = ratio_ci(&rates.map(|r| (r * 10.0, 10.0))).unwrap();
        let m = mean_ci(&rates);
        assert!(close(e.value, m.value, 1e-12) && close(e.lo.unwrap(), m.lo.unwrap(), 1e-12));
        assert!(ratio_ci(&[(0.0, 0.0)]).is_none());
        assert!(ratio_ci(&[(1.0, 2.0)]).unwrap().lo.is_none());
    }

    #[test]
    fn bradley_terry_recovers_order_and_ratio() {
        // 0 beats 1 three times out of four (plus the virtual draw): rating gap = 400 log10(3.5 / 1.5).
        let r = bradley_terry(&[vec![0.0, 3.0], vec![1.0, 0.0]]);
        assert!(
            close(r[0] - r[1], 400.0 * (3.5f64 / 1.5).log10(), 1e-6),
            "{r:?}"
        );
        assert!(close(r[0] + r[1], 0.0, 1e-6));
        let r = bradley_terry(&[
            vec![0.0, 8.0, 10.0],
            vec![2.0, 0.0, 7.0],
            vec![0.0, 3.0, 0.0],
        ]);
        assert!(r[0] > r[1] && r[1] > r[2], "{r:?}");
    }

    #[test]
    fn percentile_interpolates() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&v, 50.0), 3.0);
        assert_eq!(percentile(&v, 100.0), 5.0);
        assert!(close(percentile(&v, 90.0), 4.6, 1e-12));
    }
}
