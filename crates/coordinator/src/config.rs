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
    /// A job is dead-lettered on this many timeouts (`fail` with `code: "timeout"`). Matches are
    /// deterministic, so the default 2 retries a timed-out match once, on another worker, and then gives up.
    pub max_timeouts: i64,
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

    /// Games of unranked series (scrims) a team may request per rolling 24 h.
    pub unranked_daily_games: i64,
    /// Most games in one series an organiser creates.
    pub max_series_games: i64,
    /// Most games in one scrim (`POST /v1/scrims`).
    pub max_scrim_games: i64,
    /// Period of the built-in ranked scheduler. 0 = off (use `POST /v1/admin/ranked-batch`).
    pub ranked_interval_ms: u64,
    /// Most series the scheduler creates per round. Below that, `ibc_rating::series_per_batch` sizes each
    /// round to what the live workers can play.
    pub ranked_series_budget: usize,
    /// Maps per ranked series, each a full match (sides swap at half-time inside a match, so any count
    /// works; an odd one cannot end level on maps unless a map is drawn).
    pub ranked_games: i64,
    /// `weng-lin` or `elo` (see `rating::by_name`). Changing it on an existing data directory recomputes
    /// every rating from the event ledger at start.
    pub rating_system: String,

    /// Language name -> fuel multiplier applied to submissions in that language.
    pub languages: Vec<(String, f64)>,
    /// A worker counts as live in `/metrics` if it called within this window.
    pub worker_live_ms: i64,
    /// Ladder snapshots kept (older ones are pruned; they can be recomputed from the series table).
    pub ladder_snapshots_kept: i64,
    /// A team with fewer rated series than this is provisional: no rating or tier on the ladder, listed
    /// after every rated team.
    pub provisional_series: i64,
    /// Lowest displayed rating of each tier above Iron, in `TIERS` order (Bronze .. Radiant), ascending.
    /// Display only: the matchmaker never looks at tiers.
    pub tier_floors: [f64; 8],
}

/// Ladder tiers, lowest first. Iron is everything below `tier_floors[0]`.
pub const TIERS: [&str; 9] = [
    "Iron",
    "Bronze",
    "Silver",
    "Gold",
    "Platinum",
    "Diamond",
    "Ascendant",
    "Immortal",
    "Radiant",
];

/// Default tier floors on the 1500-centred display scale. Weng-Lin shows the conservative estimate
/// `mu - 3 sigma`, so a settled average team sits around 1360 and a brand-new one at about 760; Elo shows
/// the rating itself (1500 start). 100 points is about 64 % expected score between settled teams.
pub const DEFAULT_TIER_FLOORS: [f64; 8] = [
    1100.0, 1200.0, 1300.0, 1400.0, 1500.0, 1600.0, 1700.0, 1800.0,
];

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
            max_timeouts: 2,
            sweep_ms: 2_000,
            verify_fraction: 0.05,
            max_wasm_bytes: 16 << 20,
            max_memory_bytes: 64 << 20,
            max_blob_bytes: 64 << 20,
            max_map_bytes: 1 << 20,
            unranked_daily_games: 100,
            max_series_games: 20,
            max_scrim_games: 5,
            ranked_interval_ms: 600_000,
            ranked_series_budget: 16,
            ranked_games: 3,
            rating_system: "weng-lin".to_string(),
            languages: vec![("rust".to_string(), 1.0)],
            worker_live_ms: 90_000,
            ladder_snapshots_kept: 200,
            provisional_series: 5,
            tier_floors: DEFAULT_TIER_FLOORS,
        }
    }
}

impl Config {
    /// The tier of a displayed rating.
    pub fn tier(&self, display: f64) -> &'static str {
        TIERS[self
            .tier_floors
            .iter()
            .filter(|&&floor| display >= floor)
            .count()]
    }

    pub fn fuel_multiplier(&self, language: &str) -> Option<f64> {
        self.languages
            .iter()
            .find(|(name, _)| name == language)
            .map(|&(_, m)| m)
    }
}
