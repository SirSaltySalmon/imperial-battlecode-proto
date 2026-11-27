//! The worker's life: register, lease, run, heartbeat, drain.

use crate::backoff::{sleep_unless, Backoff};
use crate::cache::BlobCache;
use crate::client::{ApiError, Client};
use crate::config::Config;
use crate::protocol::Lease;
use crate::runner::{RunnerPool, RunnerSet};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Shutdown requests, shared with signal handlers. Drain: stop leasing and finish what is running.
/// Abort: also kill running matches and release them (`fail`, retryable).
#[derive(Default)]
pub struct Shutdown {
    drain: AtomicBool,
    abort: AtomicBool,
    /// When the drain was first requested: the drain timeout counts from here, not from when the lease
    /// loop noticed.
    drain_since: Mutex<Option<Instant>>,
}

impl Shutdown {
    pub fn request_drain(&self) {
        self.drain_since
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Instant::now);
        self.drain.store(true, Ordering::SeqCst);
    }
    pub fn request_abort(&self) {
        self.request_drain();
        self.abort.store(true, Ordering::SeqCst);
    }
    fn drain_since(&self) -> Instant {
        self.drain_since
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or_else(Instant::now)
    }
    pub fn is_draining(&self) -> bool {
        self.drain.load(Ordering::SeqCst)
    }
    pub fn is_aborting(&self) -> bool {
        self.abort.load(Ordering::SeqCst)
    }
}

/// What a run did. `released` jobs were given back without spending an attempt (not this worker's patch
/// or kind, or a shutdown). `dropped` jobs were none of the others: their lease was lost or ran out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub completed: u64,
    pub failed: u64,
    pub released: u64,
    pub dropped: u64,
}

#[derive(Default)]
pub(crate) struct Counters {
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub released: AtomicU64,
    pub dropped: AtomicU64,
}

/// A job in hand and the state of its lease.
pub(crate) struct Active {
    pub job_id: String,
    pub lease_token: String,
    /// The lease length: what the coordinator says it grants (`lease_secs` of the job, then of each
    /// heartbeat response), never more than `--lease-secs`.
    lease_ms: AtomicU64,
    /// When the coordinator will consider the lease expired, as far as we know.
    lease_deadline: Mutex<Instant>,
    next_heartbeat: Mutex<Instant>,
    /// The coordinator told us the lease is gone (409): stop work, report nothing.
    pub lost: AtomicBool,
    /// Granted by a long poll that returned after the drain began: give it back unstarted.
    pub leased_while_draining: bool,
}

impl Active {
    pub fn deadline(&self) -> Instant {
        *self
            .lease_deadline
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn lease_len(&self) -> Duration {
        Duration::from_millis(self.lease_ms.load(Ordering::Relaxed))
    }

    fn set_lease_len(&self, cfg: &Config, lease_secs: Option<f64>) {
        let len = lease_secs
            .map(|s| Duration::from_secs_f64(s.min(cfg.lease_duration.as_secs_f64())))
            .unwrap_or(cfg.lease_duration);
        self.lease_ms
            .store(len.as_millis().max(1) as u64, Ordering::Relaxed);
    }

    /// Heartbeats come six times per lease (every 30 s for protocol v1's 180 s), or more often if
    /// `--heartbeat-secs` says so.
    fn heartbeat_interval(&self, cfg: &Config) -> Duration {
        cfg.heartbeat_interval.min(self.lease_len() / 6)
    }
}

pub(crate) struct Ctx {
    pub cfg: Config,
    pub client: Client,
    pub cache: Arc<BlobCache>,
    pub runners: RunnerSet,
    pub pool: RunnerPool,
    pub shutdown: Arc<Shutdown>,
    pub counters: Counters,
    pub work_dir: PathBuf,
    active: Mutex<HashMap<u64, Arc<Active>>>,
    slot_freed: Condvar,
    stop_heartbeats: AtomicBool,
}

impl Ctx {
    fn active(&self) -> MutexGuard<'_, HashMap<u64, Arc<Active>>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Run a worker until it is drained (or, with `once`, until the queue is empty).
pub fn run(cfg: Config, shutdown: Arc<Shutdown>) -> Result<Stats, String> {
    std::fs::create_dir_all(&cfg.cache_dir)
        .map_err(|e| format!("creating {}: {e}", cfg.cache_dir.display()))?;
    let runners = RunnerSet::new(&cfg.runner, &cfg.runners_dir)
        .map_err(|e| format!("runner {}: {e}", cfg.runner.display()))?;
    let cache = BlobCache::open(&cfg.cache_dir, cfg.cache_cap_bytes)
        .map_err(|e| format!("cache {}: {e}", cfg.cache_dir.display()))?;
    // Scratch space of a previous (crashed) run is garbage: its jobs were re-leased after their leases expired.
    let work_dir = cfg.cache_dir.join("work");
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| format!("creating {}: {e}", work_dir.display()))?;
    if cfg.coordinator.starts_with("http://") {
        crate::log!(
            "warn",
            "coordinator URL is plain http: tokens travel unencrypted",
            coordinator = cfg.coordinator
        );
    }
    crate::log!(
        "info",
        "worker starting",
        name = cfg.name,
        concurrency = cfg.concurrency,
        patch = runners.primary_patch(),
        cache_bytes = cache.total_bytes()
    );

    let ctx = Arc::new(Ctx {
        client: Client::new(&cfg.coordinator, cfg.http_timeout),
        cache,
        runners,
        pool: RunnerPool::new(cfg.concurrency),
        shutdown,
        counters: Counters::default(),
        work_dir,
        active: Mutex::new(HashMap::new()),
        slot_freed: Condvar::new(),
        stop_heartbeats: AtomicBool::new(false),
        cfg,
    });

    let heartbeats = {
        let ctx = ctx.clone();
        std::thread::spawn(move || heartbeat_loop(&ctx))
    };
    let result = match load_token(&ctx.cfg) {
        Some(token) => {
            ctx.client.set_token(&token);
            lease_loop(&ctx, false)
        }
        None => match register(&ctx) {
            Ok(true) => lease_loop(&ctx, true),
            Ok(false) => Ok(()),
            Err(e) => Err(e),
        },
    };
    drain(&ctx);
    ctx.stop_heartbeats.store(true, Ordering::SeqCst);
    let _ = heartbeats.join();
    ctx.pool.shutdown();
    let _ = std::fs::remove_dir_all(&ctx.work_dir);
    let c = &ctx.counters;
    let stats = Stats {
        completed: c.completed.load(Ordering::Relaxed),
        failed: c.failed.load(Ordering::Relaxed),
        released: c.released.load(Ordering::Relaxed),
        dropped: c.dropped.load(Ordering::Relaxed),
    };
    crate::log!(
        "info",
        "worker stopped",
        completed = stats.completed,
        failed = stats.failed,
        released = stats.released,
        dropped = stats.dropped
    );
    result.map(|()| stats)
}

// ---- registration ----

fn load_token(cfg: &Config) -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg.token_file).ok()?).ok()?;
    // A token belongs to one coordinator; never send it to another.
    if v["coordinator"].as_str() != Some(cfg.coordinator.as_str()) {
        crate::log!(
            "warn",
            "token file is for another coordinator, registering again",
            file = cfg.token_file.display().to_string()
        );
        return None;
    }
    v["token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

fn save_token(cfg: &Config, worker_id: &str, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = cfg
        .token_file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = cfg
        .token_file
        .with_extension(format!("{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let _ = std::fs::remove_file(&tmp);
    let mut file = options.open(&tmp)?;
    let body = json!({"coordinator": cfg.coordinator, "worker_id": worker_id, "name": cfg.name, "token": token});
    file.write_all(format!("{body}\n").as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, &cfg.token_file)
}

/// Register and persist the worker token. `Ok(false)`: a drain was requested before we got through.
fn register(ctx: &Ctx) -> Result<bool, String> {
    let cfg = &ctx.cfg;
    let Some(reg) = cfg.registration_token.as_deref() else {
        return Err(format!(
            "no worker token in {} and no registration token given",
            cfg.token_file.display()
        ));
    };
    let mut backoff = Backoff::new(cfg.backoff_min, cfg.backoff_max);
    loop {
        if ctx.shutdown.is_draining() {
            return Ok(false);
        }
        match ctx
            .client
            .register(reg, &cfg.name, &cfg.labels, cfg.concurrency)
        {
            Ok((worker_id, token)) => {
                ctx.client.set_token(&token);
                if let Err(e) = save_token(cfg, &worker_id, &token) {
                    // Still usable for this run; the next start registers again.
                    crate::log!(
                        "error",
                        "cannot persist the worker token",
                        file = cfg.token_file.display().to_string(),
                        error = e.to_string()
                    );
                }
                crate::log!("info", "registered", worker_id = worker_id);
                return Ok(true);
            }
            Err(ApiError::Transient(m)) => {
                let wait = backoff.next_delay();
                crate::log!(
                    "warn",
                    "coordinator unreachable, backing off",
                    call = "register",
                    error = m,
                    retry_ms = wait.as_millis() as u64
                );
                sleep_unless(wait, || ctx.shutdown.is_draining());
            }
            Err(e) => return Err(format!("registration refused: {e}")),
        }
    }
}

// ---- leasing ----

/// `just_registered`: the token in use came from a registration in this run and has not worked yet.
fn lease_loop(ctx: &Arc<Ctx>, mut just_registered: bool) -> Result<(), String> {
    let cfg = &ctx.cfg;
    let mut backoff = Backoff::new(cfg.backoff_min, cfg.backoff_max);
    let mut seq = 0u64;
    while !ctx.shutdown.is_draining() {
        let free = {
            let active = ctx.active();
            if active.len() >= cfg.concurrency {
                drop(
                    ctx.slot_freed
                        .wait_timeout(active, Duration::from_millis(200)),
                );
                continue;
            }
            cfg.concurrency - active.len()
        };
        let asked = Instant::now();
        match ctx.client.lease(free, cfg.poll_wait_ms) {
            Ok(jobs) => {
                backoff.reset();
                just_registered = false;
                if jobs.is_empty() {
                    if cfg.once && ctx.active().is_empty() {
                        break;
                    }
                    // A coordinator that answers an empty poll at once must not be hammered.
                    let floor = Duration::from_millis(cfg.poll_wait_ms.clamp(1, 200));
                    sleep_unless(floor.saturating_sub(asked.elapsed()), || {
                        ctx.shutdown.is_draining()
                    });
                }
                // Decided when the poll returns, not when a job thread starts: a job leased before the
                // drain began is finished, one the poll brought back after it is released.
                let draining = ctx.shutdown.is_draining();
                for raw in jobs {
                    seq += 1;
                    start_job(ctx, raw, seq, asked, draining);
                }
            }
            Err(ApiError::Unauthorized) => {
                // A stored token can be stale (rotated, coordinator reset): register again if we can. A 401
                // on a token registered a moment ago means registering does not help: stop, do not loop.
                if just_registered || cfg.registration_token.is_none() {
                    return Err(
                        "the coordinator does not accept this worker's token (revoked?)".into(),
                    );
                }
                crate::log!("warn", "worker token no longer accepted, registering again");
                if !register(ctx)? {
                    break;
                }
                just_registered = true;
            }
            Err(e) => {
                let wait = backoff.next_delay();
                crate::log!(
                    "warn",
                    "lease failed, backing off",
                    error = e.to_string(),
                    retry_ms = wait.as_millis() as u64
                );
                sleep_unless(wait, || ctx.shutdown.is_draining());
            }
        }
    }
    Ok(())
}

/// Frees the job's slot when the job thread ends, however it ends.
struct Slot {
    ctx: Arc<Ctx>,
    seq: u64,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.ctx.active().remove(&self.seq);
        self.ctx.slot_freed.notify_all();
    }
}

fn start_job(ctx: &Arc<Ctx>, raw: Value, seq: u64, asked: Instant, draining: bool) {
    let Some(lease) = Lease::from_value(&raw) else {
        crate::log!(
            "error",
            "leased job has no job_id / lease_token, ignoring it",
            job = raw
        );
        return;
    };
    // The lease was granted after `asked`, so counting from `asked` never overestimates it.
    let active = Arc::new(Active {
        job_id: lease.job_id.clone(),
        lease_token: lease.lease_token.clone(),
        lease_ms: AtomicU64::new(0),
        lease_deadline: Mutex::new(asked),
        next_heartbeat: Mutex::new(asked),
        lost: AtomicBool::new(false),
        leased_while_draining: draining,
    });
    active.set_lease_len(&ctx.cfg, lease.lease_secs);
    *active
        .lease_deadline
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = asked + active.lease_len();
    *active
        .next_heartbeat
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Instant::now() + active.heartbeat_interval(&ctx.cfg);
    ctx.active().insert(seq, active.clone());
    crate::log!("info", "job leased", job = lease.job_id, kind = lease.kind);
    let slot = Slot {
        ctx: ctx.clone(),
        seq,
    };
    std::thread::spawn(move || {
        crate::job::handle(&slot.ctx, &active, &lease, &raw, seq);
        drop(slot);
    });
}

// ---- heartbeats and the drain file ----

fn heartbeat_loop(ctx: &Ctx) {
    let tick = (ctx.cfg.heartbeat_interval / 4)
        .clamp(Duration::from_millis(5), Duration::from_millis(250));
    while !ctx.stop_heartbeats.load(Ordering::SeqCst) {
        if let Some(file) = ctx
            .cfg
            .drain_file
            .as_ref()
            .filter(|f| !ctx.shutdown.is_draining() && f.exists())
        {
            crate::log!(
                "info",
                "drain file appeared, draining",
                file = file.display().to_string()
            );
            ctx.shutdown.request_drain();
        }
        let now = Instant::now();
        let due: Vec<Arc<Active>> = ctx
            .active()
            .values()
            .filter(|a| {
                !a.lost.load(Ordering::Relaxed)
                    && *a.next_heartbeat.lock().unwrap_or_else(|e| e.into_inner()) <= now
            })
            .cloned()
            .collect();
        for a in due {
            let sent = Instant::now();
            let interval = a.heartbeat_interval(&ctx.cfg);
            let retry = (interval / 6).clamp(Duration::from_millis(5), Duration::from_secs(5));
            let next = match ctx.client.heartbeat(&a.job_id, &a.lease_token) {
                Ok(response) => {
                    if let Some(secs) = crate::protocol::lease_secs(&response) {
                        a.set_lease_len(&ctx.cfg, Some(secs));
                    }
                    *a.lease_deadline.lock().unwrap_or_else(|e| e.into_inner()) =
                        sent + a.lease_len();
                    sent + a.heartbeat_interval(&ctx.cfg)
                }
                Err(ApiError::LeaseLost | ApiError::NotFound) => {
                    crate::log!(
                        "warn",
                        "lease lost, stopping work on the job",
                        job = a.job_id
                    );
                    a.lost.store(true, Ordering::Relaxed);
                    continue;
                }
                Err(e) => {
                    crate::log!(
                        "warn",
                        "heartbeat failed, will retry",
                        job = a.job_id,
                        error = e.to_string()
                    );
                    Instant::now() + retry
                }
            };
            *a.next_heartbeat.lock().unwrap_or_else(|e| e.into_inner()) = next;
        }
        std::thread::sleep(tick);
    }
}

// ---- drain ----

/// Wait for running jobs: first patiently (they finish and report), then, past the drain timeout or on an
/// abort request, only as long as it takes them to be killed and released.
fn drain(ctx: &Ctx) {
    let running = ctx.active().len();
    if running > 0 {
        crate::log!(
            "info",
            "draining: no new leases, waiting for running jobs",
            running = running,
            timeout_s = ctx.cfg.drain_timeout.as_secs()
        );
    }
    let wait_until = |end: Instant, or_abort: bool| loop {
        let active = ctx.active();
        if active.is_empty() {
            return true;
        }
        if Instant::now() >= end || (or_abort && ctx.shutdown.is_aborting()) {
            return false;
        }
        drop(
            ctx.slot_freed
                .wait_timeout(active, Duration::from_millis(50)),
        );
    };
    if wait_until(ctx.shutdown.drain_since() + ctx.cfg.drain_timeout, true) {
        return;
    }
    crate::log!(
        "warn",
        "drain timeout or abort: killing running matches and releasing their jobs",
        running = ctx.active().len()
    );
    ctx.shutdown.request_abort();
    let grace = ctx.cfg.http_timeout + Duration::from_secs(5);
    if !wait_until(Instant::now() + grace, false) {
        crate::log!(
            "error",
            "jobs still running at exit; their leases will expire",
            running = ctx.active().len()
        );
    }
}
