//! The budget governor: how many ranked series fit in a batch on the hardware that is actually there.

/// How the ranked ladder may spend the fleet.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BudgetConfig {
    pub games_per_series: u32,
    /// Ranked batches scheduled per hour (6 = one every ten minutes).
    pub batches_per_hour: f64,
    /// Share of the available core-seconds the ladder may plan for. The rest is headroom for retries,
    /// spot verification, variance in game length and the other job classes.
    pub utilisation: f64,
    /// Hard ceiling per batch, whatever the hardware (keeps storage growth bounded).
    pub max_series_per_batch: u32,
    /// Cost assumed for a game while nothing has been measured yet (a new season). Set it on the
    /// high side: the first batches are then small, and the measured mean takes over.
    pub prior_core_seconds_per_game: f64,
    /// Floor under the measured mean, so that a mean polluted by a few instantly-failing jobs cannot
    /// make games look free.
    pub min_core_seconds_per_game: f64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig {
            games_per_series: 5,
            batches_per_hour: 6.0,
            utilisation: 0.7,
            max_series_per_batch: 500,
            prior_core_seconds_per_game: 10.0,
            min_core_seconds_per_game: 0.25,
        }
    }
}

/// Series to schedule in the next batch:
///
/// `floor(core_seconds_per_hour * utilisation / batches_per_hour / (games_per_series * cost_per_game))`,
/// capped by `max_series_per_batch`.
///
/// `core_seconds_per_hour` is what the registered workers can deliver (cores x 3600 for an always-on
/// machine). `mean_core_seconds_per_game` is a running mean of the measured cost of finished games
/// (`wall_ms`); pass `None` while there is no measurement. `cost_per_game` is that mean, or the
/// configured prior when it is `None`, zero, negative or not finite, and never less than the
/// configured floor.
///
/// Rounding is always down, and a capacity or configuration that is zero, negative or not finite
/// yields 0, so the ladder stalls rather than outrunning the hardware. Pass the result to
/// `plan_batch` as `budget_series`.
pub fn series_per_batch(
    core_seconds_per_hour: f64,
    mean_core_seconds_per_game: Option<f64>,
    cfg: &BudgetConfig,
) -> u32 {
    let positive = |x: f64| x.is_finite() && x > 0.0;
    let cost = mean_core_seconds_per_game
        .filter(|&m| positive(m))
        .unwrap_or(cfg.prior_core_seconds_per_game)
        .max(cfg.min_core_seconds_per_game);
    let per_batch = core_seconds_per_hour * cfg.utilisation.clamp(0.0, 1.0) / cfg.batches_per_hour;
    let per_series = f64::from(cfg.games_per_series) * cost;
    if !positive(per_batch) || !positive(per_series) {
        return 0;
    }
    let series = (per_batch / per_series).floor();
    if series >= f64::from(cfg.max_series_per_batch) {
        cfg.max_series_per_batch
    } else {
        series as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spends_at_most_the_budget() {
        let cfg = BudgetConfig::default();
        // 8 cores for an hour = 28 800 core-seconds; 70 % over 6 batches = 3 360 per batch;
        // a 5-game series at 4 core-seconds per game costs 20.
        assert_eq!(series_per_batch(28_800.0, Some(4.0), &cfg), 168);
        assert_eq!(series_per_batch(28_800.0, Some(4.01), &cfg), 167);
        assert_eq!(series_per_batch(100.0, Some(4.0), &cfg), 0);
        assert_eq!(series_per_batch(1e12, Some(4.0), &cfg), 500);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(series_per_batch(bad, Some(4.0), &cfg), 0, "{bad}");
        }
        let no_batches = BudgetConfig {
            batches_per_hour: 0.0,
            ..cfg
        };
        assert_eq!(series_per_batch(28_800.0, Some(4.0), &no_batches), 0);
        let no_games = BudgetConfig {
            games_per_series: 0,
            ..cfg
        };
        assert_eq!(series_per_batch(28_800.0, Some(4.0), &no_games), 0);
    }

    #[test]
    fn unmeasured_or_implausible_cost() {
        let cfg = BudgetConfig::default();
        // No measurement yet: the prior (10 core-seconds) lets a new ladder start. 3 360 / 50 = 67.
        assert_eq!(series_per_batch(28_800.0, None, &cfg), 67);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(series_per_batch(28_800.0, Some(bad), &cfg), 67, "{bad}");
        }
        // A mean dragged down by instant failures is floored at 0.25: 3 360 / 1.25 = 2 688, not 500.
        let roomy = BudgetConfig {
            max_series_per_batch: 100_000,
            ..cfg
        };
        assert_eq!(series_per_batch(28_800.0, Some(0.001), &roomy), 2688);
        let no_prior = BudgetConfig {
            prior_core_seconds_per_game: 0.0,
            min_core_seconds_per_game: 0.0,
            ..cfg
        };
        assert_eq!(series_per_batch(28_800.0, None, &no_prior), 0);
    }
}
