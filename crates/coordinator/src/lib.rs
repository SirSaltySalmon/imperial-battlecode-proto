//! Imperial Battlecode coordinator: the job queue and lease protocol of `docs/JOBS.md` (part B), plus
//! teams, submissions, series, ratings and the ladder. One process, one data directory:
//! `<data>/coordinator.sqlite` (WAL) and `<data>/blobs/`.

pub mod accounts;
pub mod api;
pub mod blobs;
pub mod config;
pub mod db;
pub mod error;
pub mod maintenance;
pub mod model;
pub mod ratelimit;
pub mod patch;
pub mod rating;
pub mod util;
pub mod wasmcheck;

use blobs::{BlobStore, FsBlobs};
use config::Config;
use db::jobs::QueueCfg;
use db::Db;
use error::ApiResult;
use rating::RatingHook;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Process-lifetime counters exported on `/metrics` (they restart at 0, as Prometheus counters may).
#[derive(Default)]
pub struct Counters {
    pub cache_hits: AtomicU64,
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub dead: AtomicU64,
    pub leases_expired: AtomicU64,
    pub verify_confirmed: AtomicU64,
    pub verify_mismatch: AtomicU64,
    pub verify_conflict: AtomicU64,
}

pub fn bump(counter: &AtomicU64, by: usize) {
    counter.fetch_add(by as u64, Ordering::Relaxed);
}

pub struct App {
    pub cfg: Config,
    pub db: Db,
    pub blobs: Arc<dyn BlobStore>,
    pub rating: Arc<dyn RatingHook>,
    /// Woken whenever a job may have become leasable; long polls wait on it.
    pub queue_changed: tokio::sync::Notify,
    pub counters: Counters,
}

impl App {
    /// Assemble an app from its parts. The ratings in `db` must have been produced by `rating` (`open`
    /// checks that); use `open` unless you are plugging in another `RatingHook`.
    pub fn new(
        cfg: Config,
        db: Db,
        blobs: Arc<dyn BlobStore>,
        rating: Arc<dyn RatingHook>,
    ) -> Arc<App> {
        Arc::new(App {
            cfg,
            db,
            blobs,
            rating,
            queue_changed: tokio::sync::Notify::new(),
            counters: Counters::default(),
        })
    }

    /// Open (or create) a data directory with the rating system named in `cfg.rating_system`. If the stored
    /// ratings were produced by another system, they are recomputed from the event ledger first.
    pub fn open(cfg: Config, data: &Path) -> anyhow::Result<Arc<App>> {
        let hook = rating::by_name(&cfg.rating_system).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown rating system {:?} (use weng-lin or elo)",
                cfg.rating_system
            )
        })?;
        std::fs::create_dir_all(data)?;
        let db = Db::open(&data.join("coordinator.sqlite"))?;
        let blobs = FsBlobs::open(&data.join("blobs"))?;
        db.tx(|tx| {
            db::ratings::adopt_system(tx, hook.as_ref(), util::now_ms(), cfg.ladder_snapshots_kept)
        })
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
        Ok(App::new(cfg, db, Arc::new(blobs), hook))
    }

    pub fn queue_cfg(&self) -> QueueCfg {
        QueueCfg {
            lease_ms: self.cfg.lease_ms,
            max_attempts: self.cfg.max_attempts,
            aging_ms: self.cfg.aging_ms,
            retry_grace_ms: self.cfg.retry_grace_ms,
            max_timeouts: self.cfg.max_timeouts,
        }
    }

    /// After a quarantine: if it reopened rated series, rebuild the ratings without their games and write a
    /// ladder snapshot (the re-runs rate them again when they finish).
    pub fn after_quarantine(
        &self,
        tx: &rusqlite::Transaction,
        q: &db::jobs::Quarantined,
        now: i64,
    ) -> ApiResult<()> {
        if q.reopened > 0 {
            db::ratings::recompute(tx, self.rating.as_ref(), now)?;
            db::ratings::write_snapshot(
                tx,
                self.rating.as_ref(),
                now,
                self.cfg.ladder_snapshots_kept,
            )?;
        }
        Ok(())
    }

    /// After a team's live submission changed: widen its rating uncertainty if a submission was replaced,
    /// and write a ladder snapshot (the ladder shows the live submission).
    pub fn after_live_change(
        &self,
        tx: &rusqlite::Transaction,
        team_id: &str,
        change: db::LiveChange,
        now: i64,
    ) -> ApiResult<()> {
        if change == db::LiveChange::Replaced {
            db::ratings::submission_changed(tx, self.rating.as_ref(), team_id, now)?;
        }
        if change != db::LiveChange::Unchanged {
            db::ratings::write_snapshot(
                tx,
                self.rating.as_ref(),
                now,
                self.cfg.ladder_snapshots_kept,
            )?;
        }
        Ok(())
    }

    /// One pass of the sweeper: requeue (or dead-letter) expired leases, dead-letter what superseded
    /// patches failed to drain, then close finished series.
    pub fn sweep(&self) -> ApiResult<()> {
        let now = util::now_ms();
        let (requeued, dead, drained) = self.db.tx(|tx| {
            let (requeued, dead) = db::jobs::sweep_expired(tx, now, &self.queue_cfg())?;
            let drained = patch::expire_drained(self, tx, now)?;
            db::league::finalize_ready(
                tx,
                self.rating.as_ref(),
                now,
                self.cfg.ladder_snapshots_kept,
            )?;
            Ok((requeued, dead, drained))
        })?;
        bump(&self.counters.leases_expired, requeued + dead);
        bump(&self.counters.dead, dead + drained);
        if requeued > 0 {
            self.queue_changed.notify_waiters();
        }
        Ok(())
    }

    /// Create a series, queue its games and close it at once if every game was a cache hit.
    pub fn new_series(
        &self,
        tx: &rusqlite::Transaction,
        kind: &str,
        a: &db::TeamRow,
        b: &db::TeamRow,
        games: i64,
        requested_by: Option<&str>,
        now: i64,
    ) -> ApiResult<String> {
        let (id, cache_hits) = db::league::create_series(tx, kind, a, b, games, requested_by, now)?;
        self.after_series_created(tx, cache_hits, now)?;
        Ok(id)
    }

    /// Create a series from an explicit spec (a scrim), as `new_series` does.
    pub fn new_series_with(
        &self,
        tx: &rusqlite::Transaction,
        spec: &db::league::SeriesSpec,
        now: i64,
    ) -> ApiResult<String> {
        let (id, cache_hits) = db::league::create_series_with(tx, spec, now)?;
        self.after_series_created(tx, cache_hits, now)?;
        Ok(id)
    }

    fn after_series_created(
        &self,
        tx: &rusqlite::Transaction,
        cache_hits: usize,
        now: i64,
    ) -> ApiResult<()> {
        bump(&self.counters.cache_hits, cache_hits);
        if cache_hits > 0 {
            db::league::finalize_ready(
                tx,
                self.rating.as_ref(),
                now,
                self.cfg.ladder_snapshots_kept,
            )?;
        }
        Ok(())
    }

    /// How many ranked series the live workers can play in one scheduler round
    /// (`ibc_rating::series_per_batch`): their cores, the measured cost of the matches of the last hour (the
    /// governor's prior until there are some), at most `ranked_series_budget`.
    pub fn ranked_budget(
        &self,
        tx: &rusqlite::Transaction,
        games: i64,
        now: i64,
    ) -> ApiResult<u32> {
        let cores = db::live_cores(tx, now - self.cfg.worker_live_ms)?;
        let mean = db::league::mean_game_seconds(tx, now - 3_600_000)?;
        let budget = ibc_rating::BudgetConfig {
            games_per_series: u32::try_from(games).unwrap_or(u32::MAX),
            batches_per_hour: if self.cfg.ranked_interval_ms > 0 {
                3_600_000.0 / self.cfg.ranked_interval_ms as f64
            } else {
                ibc_rating::BudgetConfig::default().batches_per_hour
            },
            max_series_per_batch: u32::try_from(self.cfg.ranked_series_budget).unwrap_or(u32::MAX),
            ..ibc_rating::BudgetConfig::default()
        };
        let series = ibc_rating::series_per_batch(cores as f64 * 3600.0, mean, &budget);
        // Nothing measured yet and the prior does not fit one series in a round (a short interval, a
        // small fleet): play one anyway, so the governor gets a measurement. Otherwise the scheduler would
        // wait for an organiser to start the ladder by hand.
        Ok(if series == 0 && cores > 0 && mean.is_none() {
            1
        } else {
            series
        })
    }

    /// One round of the ranked scheduler: `ibc_rating::plan_batch` pairs live teams that have no ranked
    /// series running (under-played teams first, opponents near in rating), at most `budget` series
    /// (`None`: what `ranked_budget` says the workers can play). Returns the ids of the series created;
    /// none while a tournament lock pauses ranked matchmaking.
    pub fn ranked_round(&self, budget: Option<u32>, games: i64) -> ApiResult<Vec<String>> {
        let now = util::now_ms();
        let seed = u64::from_str_radix(&util::random_hex(8), 16).expect("16 hex digits");
        let created = self.db.tx(|tx| {
            if db::tournaments::ranked_paused(tx, now)?.is_some() {
                return Ok(Vec::new());
            }
            let budget = match budget {
                Some(b) => b,
                None => self.ranked_budget(tx, games, now)?,
            };
            let pool = db::league::matchmaking_pool(tx, self.rating.as_ref(), now)?;
            // A team whose live submission fails on the current patch past its grace is not paired.
            let excluded = patch::excluded_teams(self, tx, now)?;
            let states: Vec<ibc_rating::TeamState> = pool
                .iter()
                .map(|(t, s)| ibc_rating::TeamState {
                    live: s.live && !excluded.contains(&t.id),
                    ..s.clone()
                })
                .collect();
            let pairings = ibc_rating::plan_batch(
                self.rating.system(),
                &states,
                (now.max(0) / 1000) as u64,
                budget,
                &ibc_rating::MatchmakingConfig::default(),
                seed,
            );
            let mut created = Vec::new();
            for p in pairings {
                let (a, b) = (&pool[p.team_a as usize].0, &pool[p.team_b as usize].0);
                created.push(self.new_series(tx, "ranked", a, b, games, None, now)?);
            }
            if !created.is_empty() {
                db::league::record_batch(tx, seed, budget, created.len(), now)?;
            }
            Ok(created)
        })?;
        if !created.is_empty() {
            self.queue_changed.notify_waiters();
        }
        Ok(created)
    }
}

/// Start the sweeper and, if configured, the ranked scheduler. They run until the runtime shuts down.
pub fn spawn_background(app: Arc<App>) {
    let sweeper = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(sweeper.cfg.sweep_ms.max(10)));
        loop {
            tick.tick().await;
            if let Err(e) = sweeper.sweep() {
                eprintln!("sweeper: {}", e.message);
            }
        }
    });
    if app.cfg.ranked_interval_ms > 0 {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(app.cfg.ranked_interval_ms));
            loop {
                tick.tick().await;
                match app.ranked_round(None, app.cfg.ranked_games) {
                    Ok(created) if !created.is_empty() => {
                        eprintln!("scheduler: queued {} ranked series", created.len())
                    }
                    Ok(_) => {}
                    // No patch, no maps or fewer than two live teams yet: nothing to schedule.
                    Err(e) if e.status == axum::http::StatusCode::CONFLICT => {}
                    Err(e) => eprintln!("scheduler: {}", e.message),
                }
            }
        });
    }
}

/// Serve the API on `listener` until `shutdown` resolves.
pub async fn serve(
    app: Arc<App>,
    listener: tokio::net::TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    spawn_background(app.clone());
    // The peer address keys the per-IP rate limits.
    let service = api::router(app).into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, service)
        .with_graceful_shutdown(shutdown)
        .await
}
