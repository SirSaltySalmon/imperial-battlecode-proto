//! Everything tunable, in one struct. `main.rs` fills it from flags / environment; tests build it directly.

#[derive(Clone, Debug)]
pub struct Config {
    /// Bearer token of the organisers (`/v1/admin/*`, and admin overrides on team endpoints).
    pub admin_token: String,
    /// Shared secret workers present to `POST /v1/workers/register`.
    pub registration_token: String,
    /// A second registration secret: workers registering with it are trusted (they get the `trusted` label,
    /// their results are final and they run spot verifications). `None`: trust is granted by the admin API only.
    pub trusted_registration_token: Option<String>,

    /// Lease length, from grant and from every heartbeat. Protocol v1 says 180 s.
    pub lease_ms: i64,
    /// Upper bound of a `lease` long poll. Protocol v1 says 25 s.
    pub max_wait_ms: u64,
    /// Leases a job may consume before it is dead-lettered. Protocol v1 says 3.
    pub max_attempts: i64,
    /// A queued job is promoted by one priority class for every `aging_ms` it has waited, so lower classes
    /// (bench) cannot starve behind a permanently busy higher class. 0 = strict class order.
    pub aging_ms: i64,
    /// After a `retryable` failure the job is not handed back to the same worker for this long (a worker on
    /// an old patch would otherwise burn all attempts of a job within milliseconds).
    pub retry_grace_ms: i64,
    /// Period of the task that requeues expired leases and closes finished series.
    pub sweep_ms: u64,
    /// Fraction of results from untrusted workers that are re-run by a trusted worker. `tournament` jobs are
    /// always re-run when this is > 0. 0 disables spot verification.
    pub verify_fraction: f64,

    pub max_wasm_bytes: usize,
    pub max_memory_bytes: u64,
    /// Largest accepted blob upload (replays) and request body in general.
    pub max_blob_bytes: usize,
    pub max_map_bytes: usize,

    /// Games of unranked series a team may request per rolling 24 h.
    pub unranked_daily_games: i64,
    /// Most games in one series.
    pub max_series_games: i64,
    /// Period of the built-in ranked scheduler. 0 = off (use `POST /v1/admin/ranked-batch`).
    pub ranked_interval_ms: u64,
    /// Most series the scheduler creates per round.
    pub ranked_series_budget: usize,
    /// Games per ranked series (sides alternate, so keep it even).
    pub ranked_games: i64,

    /// Language name -> fuel multiplier applied to submissions in that language.
    pub languages: Vec<(String, f64)>,
    /// A worker counts as live in `/metrics` if it called within this window.
    pub worker_live_ms: i64,
    /// Ladder snapshots kept (older ones are pruned; they can be recomputed from the series table).
    pub ladder_snapshots_kept: i64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            admin_token: String::new(),
            registration_token: String::new(),
            trusted_registration_token: None,
            lease_ms: 180_000,
            max_wait_ms: 25_000,
            max_attempts: 3,
            aging_ms: 600_000,
            retry_grace_ms: 30_000,
            sweep_ms: 2_000,
            verify_fraction: 0.05,
            max_wasm_bytes: 16 << 20,
            max_memory_bytes: 64 << 20,
            max_blob_bytes: 64 << 20,
            max_map_bytes: 1 << 20,
            unranked_daily_games: 100,
            max_series_games: 20,
            ranked_interval_ms: 0,
            ranked_series_budget: 16,
            ranked_games: 2,
            languages: vec![("rust".to_string(), 1.0)],
            worker_live_ms: 90_000,
            ladder_snapshots_kept: 200,
        }
    }
}

impl Config {
    pub fn fuel_multiplier(&self, language: &str) -> Option<f64> {
        self.languages
            .iter()
            .find(|(name, _)| name == language)
            .map(|&(_, m)| m)
    }
}
