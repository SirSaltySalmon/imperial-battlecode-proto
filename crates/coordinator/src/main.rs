//! `ibc-coordinator`: flags and environment in, one HTTP server out.

use clap::Parser;
use ibc_coordinator::config::Config;
use ibc_coordinator::App;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ibc-coordinator",
    version,
    about = "Imperial Battlecode coordinator: job queue, submissions, ladder",
    after_help = "Maintenance (docs/DEPLOY.md): ibc-coordinator backup DIR | \
                  ibc-coordinator prune-replays --older-than DAYS [--kinds unranked] [--dry-run]"
)]
struct Args {
    /// Data directory: `coordinator.sqlite` and `blobs/` live here
    #[arg(long, env = "IBC_DATA")]
    data: PathBuf,
    /// Address to listen on (put a TLS-terminating proxy in front for anything public)
    #[arg(long, env = "IBC_LISTEN", default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// Bearer token of the organisers
    #[arg(long, env = "IBC_ADMIN_TOKEN")]
    admin_token: String,
    /// Shared secret workers register with. Default: the admin token
    #[arg(long, env = "IBC_REGISTRATION_TOKEN")]
    registration_token: Option<String>,
    /// Registration secret of trusted workers (final results, run spot verifications)
    #[arg(long, env = "IBC_TRUSTED_REGISTRATION_TOKEN")]
    trusted_registration_token: Option<String>,
    /// Lease length in seconds (protocol v1: 180)
    #[arg(long, env = "IBC_LEASE_SECS", default_value_t = 180)]
    lease_secs: u64,
    /// Leases per job before it is dead-lettered (protocol v1: 3)
    #[arg(long, env = "IBC_MAX_ATTEMPTS", default_value_t = 3)]
    max_attempts: i64,
    /// A queued job is promoted one priority class per this many seconds waited; 0 = strict class order
    #[arg(long, env = "IBC_AGING_SECS", default_value_t = 600)]
    aging_secs: u64,
    /// Seconds before a job is handed back to the worker that just failed or released it
    #[arg(long, env = "IBC_RETRY_GRACE_SECS", default_value_t = 30)]
    retry_grace_secs: u64,
    /// Timeouts (`fail` with `code: "timeout"`) after which a job is dead-lettered
    #[arg(long, env = "IBC_MAX_TIMEOUTS", default_value_t = 2)]
    max_timeouts: i64,
    /// Rating system: weng-lin or elo. Changing it recomputes every rating from the ledger at start
    #[arg(long, env = "IBC_RATING_SYSTEM", default_value = "weng-lin")]
    rating_system: String,
    /// Fraction of untrusted results re-run by a trusted worker (tournament: all); 0 = off
    #[arg(long, env = "IBC_VERIFY_FRACTION", default_value_t = 0.05)]
    verify_fraction: f64,
    /// Unranked games a team may request per 24 hours
    #[arg(long, env = "IBC_UNRANKED_DAILY_GAMES", default_value_t = 100)]
    unranked_daily_games: i64,
    /// Seconds between rounds of the built-in ranked scheduler; 0 = off
    #[arg(long, env = "IBC_RANKED_INTERVAL_SECS", default_value_t = 600)]
    ranked_interval_secs: u64,
    /// Most ranked series created per scheduler round
    #[arg(long, env = "IBC_RANKED_SERIES_BUDGET", default_value_t = 16)]
    ranked_series_budget: usize,
    /// Maps per ranked series, each a full match
    #[arg(long, env = "IBC_RANKED_GAMES", default_value_t = 3)]
    ranked_games: i64,
    /// Between rounds, top the queue up with ranked series while it holds less than this many seconds
    /// of work for the live workers; 0 = rounds on the interval only
    #[arg(long, env = "IBC_RANKED_BACKLOG_SECS", default_value_t = 120)]
    ranked_backlog_secs: u64,
    /// Ranked series a team may start per cap window
    #[arg(long, env = "IBC_RANKED_CAP_SERIES", default_value_t = 5)]
    ranked_cap_series: u32,
    /// Series past its cap a team may be drawn into as an opponent, when every team near the searching
    /// team's rating is at its cap
    #[arg(long, env = "IBC_RANKED_CAP_OVERFLOW", default_value_t = 5)]
    ranked_cap_overflow: u32,
    /// Length of the cap window, in seconds
    #[arg(long, env = "IBC_RANKED_CAP_WINDOW_SECS", default_value_t = 900)]
    ranked_cap_window_secs: u64,
    /// Placement series a new live submission plays back to back, before its tier is shown; 0 = off
    #[arg(long, env = "IBC_PLACEMENT_SERIES", default_value_t = 3)]
    placement_series: i64,
    /// Most placement series started for one team per 24 hours
    #[arg(long, env = "IBC_PLACEMENT_DAILY_SERIES", default_value_t = 9)]
    placement_daily_series: i64,
    /// Displayed-rating floors of Bronze, Silver, Gold, Platinum, Diamond, Ascendant, Immortal and
    /// Radiant, comma-separated and ascending (Iron is below the first)
    #[arg(long, env = "IBC_TIER_FLOORS", value_parser = parse_tier_floors)]
    tier_floors: Option<[f64; 8]>,
    /// Largest accepted bot module, in bytes
    #[arg(long, env = "IBC_MAX_WASM_BYTES", default_value_t = 16 << 20)]
    max_wasm_bytes: usize,
    /// Largest accepted blob (replay), in bytes
    #[arg(long, env = "IBC_MAX_BLOB_BYTES", default_value_t = 64 << 20)]
    max_blob_bytes: usize,
    /// Seconds a tournament lock pauses ranked matchmaking (resume sooner with .../resume-ranked)
    #[arg(long, env = "IBC_TOURNAMENT_PAUSE_SECS", default_value_t = 3600)]
    tournament_pause_secs: u64,
    #[command(flatten)]
    auth: ibc_coordinator::accounts::AuthArgs,
    #[command(flatten)]
    moderation: ibc_coordinator::ratelimit::ModerationArgs,
    /// Built web platform served at `/`. Default: `web/dist` beside the binary, else under the working directory
    #[arg(long, env = "IBC_WEB_DIR")]
    web_dir: Option<PathBuf>,
    /// Built replay viewer served at `/viewer/`. Default: `viewer/dist` beside the binary, else under the
    /// working directory
    #[arg(long, env = "IBC_VIEWER_DIR")]
    viewer_dir: Option<PathBuf>,
    /// Language fuel multipliers, e.g. `--language rust=1.0 --language python=3.0`. Default: rust=1, cpp=1
    #[arg(long = "language", value_parser = parse_language)]
    languages: Vec<(String, f64)>,
    /// Seconds a live submission that stopped validating at a patch switch keeps playing ranked games
    #[arg(long, env = "IBC_INVALID_GRACE_SECS", default_value_t = 86_400)]
    invalid_grace_secs: u64,
    /// Seconds after a patch switch before jobs of the old patch still queued are dead-lettered
    #[arg(long, env = "IBC_PATCH_DRAIN_SECS", default_value_t = 3_600)]
    patch_drain_secs: u64,
    /// A balance patch widens every rating's uncertainty by this factor (>= 1)
    #[arg(long, env = "IBC_BALANCE_SIGMA_FACTOR", default_value_t = 1.5)]
    balance_sigma_factor: f64,
    /// `distribution.json` (from `cargo xtask dist`) served at /v1/distribution for `ibc update`
    #[arg(long, env = "IBC_DISTRIBUTION")]
    distribution: Option<PathBuf>,
}

fn parse_language(s: &str) -> Result<(String, f64), String> {
    let (name, mult) = s.split_once('=').ok_or("expected NAME=MULTIPLIER")?;
    let mult: f64 = mult
        .parse()
        .map_err(|_| format!("bad multiplier {mult:?}"))?;
    let mult = ibc_coordinator::model::normalize_fuel_mult(mult)
        .ok_or("multiplier out of range (0.001 ..= 1000)")?;
    Ok((name.to_string(), mult))
}

fn parse_tier_floors(s: &str) -> Result<[f64; 8], String> {
    let floors: Vec<f64> = s
        .split(',')
        .map(|f| f.trim().parse().map_err(|_| format!("bad number {f:?}")))
        .collect::<Result<_, _>>()?;
    let floors: [f64; 8] = floors
        .try_into()
        .map_err(|v: Vec<f64>| format!("expected 8 floors, got {}", v.len()))?;
    if floors.iter().any(|f| !f.is_finite()) || floors.windows(2).any(|w| w[0] >= w[1]) {
        return Err("floors must be finite and strictly ascending".into());
    }
    Ok(floors)
}
/// Longest grace period or drain window accepted.
const MAX_WINDOW_SECS: u64 = 366 * 86_400;
/// A page directory: the one given, else `<name>/dist` (or `<name>`) beside the binary, else `<name>/dist` under
/// the working directory. Only a directory that holds an `index.html` counts.
fn page_dir(given: &Option<PathBuf>, name: &str) -> anyhow::Result<Option<PathBuf>> {
    if let Some(dir) = given {
        if !dir.join("index.html").is_file() {
            anyhow::bail!("{} has no index.html", dir.display());
        }
        return Ok(Some(dir.clone()));
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()));
    let mut candidates = Vec::new();
    if let Some(bin) = beside {
        candidates.push(bin.join(name).join("dist"));
        candidates.push(bin.join(name));
    }
    candidates.push(PathBuf::from(name).join("dist"));
    Ok(candidates
        .into_iter()
        .find(|dir| dir.join("index.html").is_file()))
}

fn config(args: &Args) -> anyhow::Result<Config> {
    if args.admin_token.is_empty() {
        anyhow::bail!("--admin-token must not be empty");
    }
    if !(0.0..=1.0).contains(&args.verify_fraction) {
        anyhow::bail!("--verify-fraction must be between 0 and 1");
    }
    if !(1.0..=100.0).contains(&args.balance_sigma_factor) {
        anyhow::bail!("--balance-sigma-factor must be between 1 and 100");
    }
    if args.invalid_grace_secs > MAX_WINDOW_SECS
        || args.patch_drain_secs > MAX_WINDOW_SECS
        || args.ranked_backlog_secs > MAX_WINDOW_SECS
        || args.ranked_cap_window_secs > MAX_WINDOW_SECS
    {
        anyhow::bail!(
            "--invalid-grace-secs, --patch-drain-secs, --ranked-backlog-secs and --ranked-cap-window-secs \
             must be at most {MAX_WINDOW_SECS} (a year)"
        );
    }
    if args.lease_secs == 0
        || args.max_attempts < 1
        || args.max_timeouts < 1
        || args.ranked_games < 1
    {
        anyhow::bail!(
            "--lease-secs, --max-attempts, --max-timeouts and --ranked-games must be at least 1"
        );
    }
    let defaults = Config::default();
    let lease_ms = (args.lease_secs * 1000) as i64;
    Ok(Config {
        admin_token: args.admin_token.clone(),
        registration_token: args
            .registration_token
            .clone()
            .unwrap_or_else(|| args.admin_token.clone()),
        trusted_registration_token: args.trusted_registration_token.clone(),
        lease_ms,
        max_attempts: args.max_attempts,
        aging_ms: (args.aging_secs * 1000) as i64,
        retry_grace_ms: (args.retry_grace_secs * 1000) as i64,
        max_timeouts: args.max_timeouts,
        rating_system: args.rating_system.clone(),
        // Notice an expired lease within a small fraction of its length.
        sweep_ms: (lease_ms as u64 / 20).clamp(250, 5_000),
        verify_fraction: args.verify_fraction,
        max_wasm_bytes: args.max_wasm_bytes,
        max_blob_bytes: args.max_blob_bytes,
        unranked_daily_games: args.unranked_daily_games,
        ranked_interval_ms: args.ranked_interval_secs * 1000,
        ranked_series_budget: args.ranked_series_budget,
        ranked_games: args.ranked_games,
        ranked_backlog_ms: (args.ranked_backlog_secs * 1000) as i64,
        ranked_cap_series: i64::from(args.ranked_cap_series),
        ranked_cap_overflow: i64::from(args.ranked_cap_overflow),
        ranked_cap_window_ms: (args.ranked_cap_window_secs * 1000) as i64,
        placement_series: args.placement_series.max(0),
        placement_daily_series: args.placement_daily_series.max(0),
        tier_floors: args.tier_floors.unwrap_or(defaults.tier_floors),
        tournament_pause_ms: i64::try_from(args.tournament_pause_secs.saturating_mul(1000))
            .unwrap_or(i64::MAX),
        languages: if args.languages.is_empty() {
            defaults.languages.clone()
        } else {
            args.languages.clone()
        },
        auth: args.auth.config(args.listen)?,
        moderation: args.moderation.to_config(),
        invalid_grace_ms: (args.invalid_grace_secs * 1000) as i64,
        patch_drain_ms: (args.patch_drain_secs * 1000) as i64,
        // Both are at most `MAX_WINDOW_SECS` (checked above), so the products fit.
        balance_sigma_factor: args.balance_sigma_factor,
        web_dir: page_dir(&args.web_dir, "web")?,
        viewer_dir: page_dir(&args.viewer_dir, "viewer")?,
        distribution_file: args
            .distribution
            .clone()
            .filter(|p| !p.as_os_str().is_empty()),
        ..defaults
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Some(done) = ibc_coordinator::maintenance::from_args(std::env::args_os().collect()) {
        return done;
    }
    let args = Args::parse();
    let app = App::open(config(&args)?, &args.data)?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    eprintln!(
        "ibc-coordinator listening on http://{} (data: {})",
        listener.local_addr()?,
        args.data.display()
    );
    for (what, dir) in [("web", &app.cfg.web_dir), ("viewer", &app.cfg.viewer_dir)] {
        match dir {
            Some(dir) => eprintln!("serving {what} pages from {}", dir.display()),
            None => eprintln!(
                "no {what} pages found (set IBC_{}_DIR)",
                what.to_uppercase()
            ),
        }
    }
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("shutting down");
    };
    ibc_coordinator::serve(app, listener, shutdown).await?;
    Ok(())
}
