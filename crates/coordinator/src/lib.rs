//! Imperial Battlecode coordinator: the job queue and lease protocol of `docs/JOBS.md` (part B), plus
//! teams, submissions, series, ratings and the ladder. One process, one data directory:
//! `<data>/coordinator.sqlite` (WAL) and `<data>/blobs/`.

pub mod api;
pub mod blobs;
pub mod config;
pub mod db;
pub mod error;
pub mod model;
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
    scheduler_round: AtomicU64,
}

impl App {
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
            scheduler_round: AtomicU64::new(0),
        })
    }

    /// Open (or create) a data directory with the built-in Elo. To use another rating system, build the
    /// pieces yourself and call `App::new` with its `RatingHook`.
    pub fn open(cfg: Config, data: &Path) -> anyhow::Result<Arc<App>> {
        std::fs::create_dir_all(data)?;
        let db = Db::open(&data.join("coordinator.sqlite"))?;
        let blobs = FsBlobs::open(&data.join("blobs"))?;
        Ok(App::new(
            cfg,
            db,
            Arc::new(blobs),
            Arc::new(rating::Elo::default()),
        ))
    }

    pub fn queue_cfg(&self) -> QueueCfg {
        QueueCfg {
            lease_ms: self.cfg.lease_ms,
            max_attempts: self.cfg.max_attempts,
            aging_ms: self.cfg.aging_ms,
            retry_grace_ms: self.cfg.retry_grace_ms,
        }
    }

    /// One pass of the sweeper: requeue (or dead-letter) expired leases, then close finished series.
    pub fn sweep(&self) -> ApiResult<()> {
        let now = util::now_ms();
        let (requeued, dead) = self.db.tx(|tx| {
            let swept = db::jobs::sweep_expired(tx, now, &self.queue_cfg())?;
            db::league::finalize_ready(
                tx,
                self.rating.as_ref(),
                now,
                self.cfg.ladder_snapshots_kept,
            )?;
            Ok(swept)
        })?;
        bump(&self.counters.leases_expired, requeued + dead);
        bump(&self.counters.dead, dead);
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
        bump(&self.counters.cache_hits, cache_hits);
        if cache_hits > 0 {
            db::league::finalize_ready(
                tx,
                self.rating.as_ref(),
                now,
                self.cfg.ladder_snapshots_kept,
            )?;
        }
        Ok(id)
    }

    /// One round of the ranked scheduler: pair rating-adjacent live teams and queue a series per pair.
    /// Returns the ids of the series created.
    pub fn ranked_round(&self, budget: usize, games: i64) -> ApiResult<Vec<String>> {
        let now = util::now_ms();
        let round = self.scheduler_round.fetch_add(1, Ordering::Relaxed);
        let created = self.db.tx(|tx| {
            let mut created = Vec::new();
            for (a, b) in db::league::auto_pairings(tx, self.rating.as_ref(), budget, round)? {
                created.push(self.new_series(tx, "ranked", &a, &b, games, None, now)?);
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
                match app.ranked_round(app.cfg.ranked_series_budget, app.cfg.ranked_games) {
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
    axum::serve(listener, api::router(app))
        .with_graceful_shutdown(shutdown)
        .await
}
