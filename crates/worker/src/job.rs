//! One leased job, from artifacts to `complete` / `fail` (docs/JOBS.md, "Worker procedure").

use crate::backoff::{sleep_unless, Backoff};
use crate::cache::{CacheError, Pinned};
use crate::client::ApiError;
use crate::protocol::{CompleteBody, Lease, MatchResult, MatchSpec};
use crate::runner::Recv;
use crate::worker::{Active, Ctx};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// A match that ran: everything `complete` needs. `replay_sha256` is always our own hash of `replay`.
struct Finished {
    winner: Value,
    outcome: Value,
    score: Value,
    ticks: Value,
    wall_ms: Value,
    replay_sha256: String,
    replay: Vec<u8>,
    summary: Box<RawValue>,
}

/// Why a job did not finish.
enum Stop {
    Fail {
        error: String,
        retryable: bool,
        /// `Some("timeout")`: the match itself ran past the wall-clock limit.
        code: Option<&'static str>,
    },
    /// Not this worker's job (another patch, an unknown kind) or the worker is shutting down: give it
    /// back without spending one of its attempts.
    Release(String),
    /// Nothing to tell the coordinator: the lease is gone or about to be.
    Dropped(&'static str),
}

fn fail(error: impl Into<String>, retryable: bool) -> Stop {
    Stop::Fail {
        error: error.into(),
        retryable,
        code: None,
    }
}

/// Stop working on a job nobody is waiting for any more.
fn check(ctx: &Ctx, active: &Active) -> Result<(), Stop> {
    if active.lost.load(Ordering::Relaxed) {
        return Err(Stop::Dropped("lease lost"));
    }
    if ctx.shutdown.is_aborting() {
        return Err(Stop::Release("worker shutting down".into()));
    }
    if Instant::now() >= active.deadline() {
        return Err(Stop::Dropped("lease expired"));
    }
    Ok(())
}

pub(crate) fn handle(ctx: &Ctx, active: &Active, lease: &Lease, raw: &Value, seq: u64) {
    let dir = ctx.work_dir.join(format!("job-{seq}"));
    let started = Instant::now();
    match process(ctx, active, lease, raw, &dir, seq) {
        Ok(done) => report_complete(ctx, active, &done, started),
        Err(Stop::Fail {
            error,
            retryable,
            code,
        }) => report_fail(ctx, active, &error, retryable, code),
        Err(Stop::Release(reason)) => report_release(ctx, active, &reason),
        Err(Stop::Dropped(why)) => dropped(ctx, active, why),
    }
    // Per-job scratch space never outlives the job.
    let _ = std::fs::remove_dir_all(&dir);
}

fn process(
    ctx: &Ctx,
    active: &Active,
    lease: &Lease,
    raw: &Value,
    dir: &Path,
    seq: u64,
) -> Result<Finished, Stop> {
    // Granted by a long poll that was still open when the drain began: nothing has started, so give it back
    // now (no attempt spent) rather than start a match the machine may not live to finish.
    if ctx.shutdown.is_draining() {
        return Err(Stop::Release("worker shutting down".into()));
    }
    if lease.kind != "match" {
        return Err(Stop::Release(format!("unknown job kind {:?}", lease.kind)));
    }
    // A match job this worker cannot parse may be a newer dialect: let another worker try.
    let spec: MatchSpec = serde_json::from_value(raw.clone())
        .map_err(|e| fail(format!("malformed match job: {e}"), true))?;
    for sha in [&spec.bot_a.sha256, &spec.bot_b.sha256, &spec.map.sha256] {
        if !crate::is_sha256(sha) {
            return Err(fail(
                format!("malformed match job: {sha:?} is not a sha256"),
                false,
            ));
        }
    }
    // Artifacts get the same wall-clock budget as the match: a job never sits in download forever.
    let until = Instant::now() + ctx.cfg.job_timeout;
    let runner = resolve_runner(ctx, active, &spec, until)?
        .ok_or_else(|| Stop::Release("patch mismatch".into()))?;
    let bot_a = fetch(ctx, active, &spec.bot_a.sha256, until)?;
    let bot_b = fetch(ctx, active, &spec.bot_b.sha256, until)?;
    let map = fetch(ctx, active, &spec.map.sha256, until)?;

    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut job = json!({
        "id": seq,
        "bot_a": path(bot_a.path()), "bot_b": path(bot_b.path()), "map": path(map.path()),
        "seed": spec.seed, "out_dir": path(dir),
        "fuel_mult_a": spec.bot_a.fuel_mult, "fuel_mult_b": spec.bot_b.fuel_mult,
    });
    if let Some(t) = spec.max_ticks {
        job["max_ticks"] = json!(t);
    }
    let event = run(ctx, active, &runner, &job, seq)?;
    collect(dir, &event)
}

/// The runner for the job's patch: the primary one, a side-by-side one, or (opt-in) one fetched as a blob.
fn resolve_runner(
    ctx: &Ctx,
    active: &Active,
    spec: &MatchSpec,
    until: Instant,
) -> Result<Option<PathBuf>, Stop> {
    if let Some(p) = ctx.runners.resolve(&spec.patch) {
        return Ok(Some(p));
    }
    let named = spec
        .runner_blobs
        .as_ref()
        .and_then(|m| m.get(&crate::runner::platform()));
    // A patch that cannot name a runner directory could never be installed: do not download its binary.
    let installable = spec.patch.dir_name().is_some();
    let Some(sha) =
        named.filter(|s| ctx.cfg.allow_runner_fetch && installable && crate::is_sha256(s))
    else {
        return Ok(None);
    };
    let blob = fetch(ctx, active, sha, until)?;
    crate::log!(
        "info",
        "installing runner named by the job",
        patch = spec.patch,
        sha256 = sha
    );
    ctx.runners
        .install(&spec.patch, blob.path())
        .map_err(|e| fail(format!("installing runner: {e}"), true))
}

/// A verified blob from the cache, downloading it if needed. Transient trouble is retried until `until`
/// (a blob already in the cache is always served).
fn fetch(ctx: &Ctx, active: &Active, sha: &str, until: Instant) -> Result<Pinned, Stop> {
    let mut backoff = Backoff::new(ctx.cfg.backoff_min, ctx.cfg.backoff_max);
    let mut mismatches = 0;
    let mut first = true;
    loop {
        check(ctx, active)?;
        if !first && Instant::now() >= until {
            return Err(fail(
                format!("timeout: blob {sha} could not be downloaded in time"),
                true,
            ));
        }
        first = false;
        let wait = match ctx.cache.get(sha, |tmp| {
            ctx.client.get_blob(sha, tmp, ctx.cfg.max_blob_bytes)
        }) {
            Ok(pinned) => return Ok(pinned),
            Err(CacheError::Fetch(ApiError::NotFound)) => {
                return Err(fail(format!("blob {sha} not found"), false))
            }
            Err(CacheError::Fetch(ApiError::Transient(m))) => {
                crate::log!(
                    "warn",
                    "blob download failed, will retry",
                    job = active.job_id,
                    sha256 = sha,
                    error = m
                );
                backoff.next_delay()
            }
            Err(CacheError::Fetch(e)) => return Err(fail(format!("blob {sha}: {e}"), true)),
            Err(CacheError::HashMismatch { actual, .. }) => {
                crate::log!(
                    "warn",
                    "downloaded blob does not match its hash, discarded",
                    job = active.job_id,
                    sha256 = sha,
                    actual = actual
                );
                mismatches += 1;
                if mismatches >= 2 {
                    return Err(fail(
                        format!("blob {sha}: hash mismatch (received {actual})"),
                        true,
                    ));
                }
                Duration::ZERO
            }
            Err(CacheError::Io(e)) => return Err(fail(format!("blob cache: {e}"), true)),
        };
        sleep_unless(wait, || check(ctx, active).is_err());
    }
}

/// Feed one job to a warm runner process and wait for its `finished` event, within the wall-clock limit.
fn run(ctx: &Ctx, active: &Active, runner: &Path, job: &Value, seq: u64) -> Result<Value, Stop> {
    let mut proc_ = ctx
        .pool
        .take(runner)
        .map_err(|e| fail(format!("cannot start runner: {e}"), true))?;
    proc_
        .submit(job)
        .map_err(|e| fail(format!("runner stdin: {e}"), true))?;
    let started = Instant::now();
    let limit = ctx.cfg.job_timeout;
    loop {
        // Returning drops `proc_`, which kills the process: a lost or timed-out job stops burning CPU.
        check(ctx, active)?;
        let left = limit.saturating_sub(started.elapsed());
        if left.is_zero() {
            // Retryable (this machine may just be overloaded), but marked: a match is deterministic, so the
            // coordinator gives up on a job that times out again instead of spending every attempt on it.
            return Err(Stop::Fail {
                error: format!(
                    "timeout: match exceeded {:.1} s of wall clock",
                    limit.as_secs_f64()
                ),
                retryable: true,
                code: Some("timeout"),
            });
        }
        let event = match proc_.recv(left.min(Duration::from_millis(50))) {
            Recv::Timeout => continue,
            Recv::Exited => return Err(fail("runner exited before finishing the job", true)),
            Recv::Event(e) => e,
        };
        if event.get("id").and_then(Value::as_u64) != Some(seq) {
            continue;
        }
        match event.get("event").and_then(Value::as_str) {
            Some("finished") => {
                ctx.pool.put(proc_);
                return Ok(event);
            }
            Some("failed") => {
                ctx.pool.put(proc_);
                let error = event
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("runner reported a failure")
                    .to_string();
                let retryable = runner_error_is_environmental(&error);
                return Err(fail(error, retryable));
            }
            _ => {}
        }
    }
}

/// The inputs were hash-verified and are readable, so a runner `failed` is a bad input (invalid wasm or
/// map: not retryable) unless the message points at the environment (I/O, a panic).
fn runner_error_is_environmental(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    [
        "internal error",
        "panic",
        "reading ",
        "writing ",
        "creating ",
        "no such bot",
        "os error",
        "invalid job",
    ]
    .iter()
    .any(|m| e.contains(m))
}

/// Read the outputs back. The replay hash we report is the one we compute here from the file.
fn collect(dir: &Path, event: &Value) -> Result<Finished, Stop> {
    let io = |what: &str, e: std::io::Error| fail(format!("reading {what}: {e}"), true);
    let replay = std::fs::read(dir.join("replay.ibcr")).map_err(|e| io("replay", e))?;
    let replay_sha256 = crate::sha256_hex(&replay);
    let summary_text =
        std::fs::read_to_string(dir.join("summary.json")).map_err(|e| io("summary", e))?;
    let summary_value: Value = serde_json::from_str(&summary_text)
        .map_err(|e| fail(format!("summary.json: {e}"), true))?;
    for (source, claimed) in [
        ("runner event", &event["replay_sha256"]),
        ("summary.json", &summary_value["replay_sha256"]),
    ] {
        if claimed.as_str() != Some(&replay_sha256) {
            return Err(fail(format!("replay hash mismatch: {source} says {claimed}, the file hashes to {replay_sha256}"), true));
        }
    }
    let summary = RawValue::from_string(summary_text)
        .map_err(|e| fail(format!("summary.json: {e}"), true))?;
    Ok(Finished {
        winner: event["winner"].clone(),
        outcome: event["outcome"].clone(),
        score: event["score"].clone(),
        ticks: event["ticks"].clone(),
        wall_ms: event["wall_ms"].clone(),
        replay_sha256,
        replay,
        summary,
    })
}

enum GiveUp {
    Lost,
    Expired,
    Aborted,
    Api(ApiError),
}

/// Call the coordinator until it answers, for as long as the lease would still be honoured.
fn retry<T>(
    ctx: &Ctx,
    active: &Active,
    what: &str,
    mut call: impl FnMut() -> Result<T, ApiError>,
) -> Result<T, GiveUp> {
    let mut backoff = Backoff::new(ctx.cfg.backoff_min, ctx.cfg.backoff_max);
    loop {
        if active.lost.load(Ordering::Relaxed) {
            return Err(GiveUp::Lost);
        }
        let left = active.deadline().saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(GiveUp::Expired);
        }
        match call() {
            Ok(v) => return Ok(v),
            Err(ApiError::LeaseLost) => {
                active.lost.store(true, Ordering::Relaxed);
                return Err(GiveUp::Lost);
            }
            Err(ApiError::Transient(m)) => {
                crate::log!(
                    "warn",
                    "coordinator call failed, will retry",
                    call = what,
                    job = active.job_id,
                    error = m
                );
                if ctx.shutdown.is_aborting() {
                    return Err(GiveUp::Aborted);
                }
                let stop = || ctx.shutdown.is_aborting() || active.lost.load(Ordering::Relaxed);
                sleep_unless(backoff.next_delay().min(left), stop);
            }
            Err(e) => return Err(GiveUp::Api(e)),
        }
    }
}

fn dropped(ctx: &Ctx, active: &Active, why: &str) {
    ctx.counters.dropped.fetch_add(1, Ordering::Relaxed);
    crate::log!("warn", "job dropped", job = active.job_id, reason = why);
}

fn give_up(ctx: &Ctx, active: &Active, what: &str, g: GiveUp) {
    match g {
        GiveUp::Lost => dropped(ctx, active, "lease lost"),
        GiveUp::Expired => dropped(
            ctx,
            active,
            "lease expired before the coordinator could be reached",
        ),
        GiveUp::Aborted => dropped(ctx, active, "worker shutting down"),
        GiveUp::Api(e) => {
            crate::log!(
                "error",
                "coordinator refused a call",
                call = what,
                job = active.job_id,
                error = e.to_string()
            );
            dropped(ctx, active, "coordinator refused the call");
        }
    }
}

/// Upload the replay, then `complete`.
fn report_complete(ctx: &Ctx, active: &Active, done: &Finished, started: Instant) {
    match retry(ctx, active, "put replay", || {
        ctx.client.put_blob(&done.replay_sha256, &done.replay)
    }) {
        Ok(()) => {}
        Err(GiveUp::Api(e)) => {
            return report_fail(
                ctx,
                active,
                &format!("replay upload refused: {e}"),
                true,
                None,
            )
        }
        Err(g) => return give_up(ctx, active, "put replay", g),
    }
    let body = CompleteBody {
        lease_token: &active.lease_token,
        result: MatchResult {
            winner: &done.winner,
            outcome: &done.outcome,
            score: &done.score,
            ticks: &done.ticks,
            replay_sha256: &done.replay_sha256,
            summary: &done.summary,
            wall_ms: &done.wall_ms,
        },
    };
    match retry(ctx, active, "complete", || {
        ctx.client.complete(&active.job_id, &body)
    }) {
        Ok(()) => {
            ctx.counters.completed.fetch_add(1, Ordering::Relaxed);
            crate::log!(
                "info",
                "job completed",
                job = active.job_id,
                replay_sha256 = done.replay_sha256,
                ticks = done.ticks,
                total_ms = started.elapsed().as_millis() as u64
            );
        }
        Err(GiveUp::Api(e)) => {
            report_fail(ctx, active, &format!("complete refused: {e}"), true, None)
        }
        Err(g) => give_up(ctx, active, "complete", g),
    }
}

fn report_fail(
    ctx: &Ctx,
    active: &Active,
    error: &str,
    retryable: bool,
    code: Option<&'static str>,
) {
    crate::log!(
        "warn",
        "job failed",
        job = active.job_id,
        error = error,
        retryable = retryable,
        code = code
    );
    match retry(ctx, active, "fail", || {
        ctx.client
            .fail(&active.job_id, &active.lease_token, error, retryable, code)
    }) {
        Ok(()) => {
            ctx.counters.failed.fetch_add(1, Ordering::Relaxed);
        }
        Err(g) => give_up(ctx, active, "fail", g),
    }
}

/// `release` the job. A coordinator that predates `release` answers 404: fall back to a retryable `fail`.
fn report_release(ctx: &Ctx, active: &Active, reason: &str) {
    crate::log!("info", "job released", job = active.job_id, reason = reason);
    match retry(ctx, active, "release", || {
        ctx.client
            .release(&active.job_id, &active.lease_token, reason)
    }) {
        Ok(()) => {
            ctx.counters.released.fetch_add(1, Ordering::Relaxed);
        }
        Err(GiveUp::Api(ApiError::NotFound)) => report_fail(ctx, active, reason, true, None),
        Err(g) => give_up(ctx, active, "release", g),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn classifies_runner_errors() {
        use super::runner_error_is_environmental as env;
        assert!(env("internal error (panic): index out of bounds"));
        assert!(env(
            "reading /cache/blobs/ab: Access is denied. (os error 5)"
        ));
        assert!(!env("bot `x`: unsupported imports: env.foo"));
        assert!(!env("map: line 3: unknown tile"));
    }
}
