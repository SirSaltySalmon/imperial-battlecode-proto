//! The job queue: idempotent enqueue, leases, completion, failure, expiry, spot verification.
//!
//! States: `queued -> leased -> done`, with `leased -> queued` on a retryable failure or an expired lease,
//! and `-> dead` after `max_attempts` leases or one non-retryable failure. `attempts` counts leases.
//! A `verify` job is a duplicate of a `match` job (same payload, key `verify:<key>`) that only workers with
//! the `trusted` label may lease; on the wire both are `"kind": "match"`.

use super::{WorkerRow, TRUSTED_LABEL};
use crate::error::{ApiError, ApiResult};
use crate::model::{Class, JobSpec, CLASSES};
use crate::util::{opt_rfc3339, random_hex, rfc3339};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug)]
pub struct QueueCfg {
    pub lease_ms: i64,
    pub max_attempts: i64,
    pub aging_ms: i64,
    pub retry_grace_ms: i64,
}

struct JobRow {
    id: String,
    kind: String,
    class: i64,
    key: String,
    payload: String,
    state: String,
    attempts: i64,
    lease_token: Option<String>,
    lease_expires_at: Option<i64>,
    worker_id: Option<String>,
    verifies: Option<String>,
    result: Option<String>,
    replay_sha256: Option<String>,
    verified: bool,
}

const JOB_COLS: &str = "id, kind, class, key, payload, state, attempts, lease_token, lease_expires_at, worker_id, verifies, result, replay_sha256, verified";

fn row(r: &rusqlite::Row) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        class: r.get(2)?,
        key: r.get(3)?,
        payload: r.get(4)?,
        state: r.get(5)?,
        attempts: r.get(6)?,
        lease_token: r.get(7)?,
        lease_expires_at: r.get(8)?,
        worker_id: r.get(9)?,
        verifies: r.get(10)?,
        result: r.get(11)?,
        replay_sha256: r.get(12)?,
        verified: r.get(13)?,
    })
}

fn load(tx: &Transaction, id: &str) -> ApiResult<Option<JobRow>> {
    Ok(tx
        .query_row(
            &format!("SELECT {JOB_COLS} FROM jobs WHERE id = ?1"),
            [id],
            row,
        )
        .optional()?)
}

fn load_by_key(tx: &Transaction, key: &str) -> ApiResult<Option<JobRow>> {
    Ok(tx
        .query_row(
            &format!("SELECT {JOB_COLS} FROM jobs WHERE key = ?1"),
            [key],
            row,
        )
        .optional()?)
}

fn parse_json(text: &str) -> ApiResult<Value> {
    serde_json::from_str(text).map_err(ApiError::internal)
}

// ---- enqueue ----

#[derive(Debug)]
pub struct Enqueued {
    pub job_id: String,
    pub state: String,
    /// False if a job with this idempotency key already existed (no work was created).
    pub created: bool,
    /// The stored result if the job is already done: a cache hit.
    pub result: Option<Value>,
}

/// Queue a match, or return the existing job with the same idempotency key. An existing job that is still
/// queued is promoted if the new request has a higher priority class.
pub fn enqueue(tx: &Transaction, class: Class, spec: &JobSpec, now: i64) -> ApiResult<Enqueued> {
    let key = spec.key();
    if let Some(job) = load_by_key(tx, &key)? {
        if (class as i64) < job.class && job.state != "done" {
            tx.execute(
                "UPDATE jobs SET class = ?2 WHERE id = ?1",
                params![job.id, class as i64],
            )?;
        }
        let result = if job.state == "done" {
            job.result.as_deref().map(parse_json).transpose()?
        } else {
            None
        };
        return Ok(Enqueued {
            job_id: job.id,
            state: job.state,
            created: false,
            result,
        });
    }
    let id = format!("j{}", &key[..24]);
    let payload = serde_json::to_string(spec).map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO jobs(id, kind, class, key, payload, state, created_at, queued_at) VALUES (?1, 'match', ?2, ?3, ?4, 'queued', ?5, ?5)",
        params![id, class as i64, key, payload, now],
    )?;
    Ok(Enqueued {
        job_id: id,
        state: "queued".into(),
        created: true,
        result: None,
    })
}

// ---- lease ----

fn wire_job(id: &str, lease_token: &str, lease_expires_at: i64, payload: &str) -> ApiResult<Value> {
    let mut job = parse_json(payload)?;
    let obj = job
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("job payload is not an object"))?;
    obj.insert("job_id".into(), json!(id));
    obj.insert("lease_token".into(), json!(lease_token));
    obj.insert("lease_expires_at".into(), json!(rfc3339(lease_expires_at)));
    obj.insert("kind".into(), json!("match"));
    Ok(job)
}

/// Lease up to `max` jobs to `worker`: lowest effective class first, oldest first within it. The effective
/// class of a job is its class minus one for every `aging_ms` it has existed, so nothing starves.
/// Within a class the oldest job is also the most aged, so only the head of each class has to be looked at.
pub fn lease(
    tx: &Transaction,
    worker: &WorkerRow,
    max: usize,
    now: i64,
    cfg: &QueueCfg,
) -> ApiResult<Vec<Value>> {
    if worker.quarantined {
        return Ok(Vec::new());
    }
    let labels = serde_json::to_string(&worker.labels).map_err(ApiError::internal)?;
    let mut out = Vec::new();
    let mut head = tx.prepare_cached(
        "SELECT id, created_at, payload FROM jobs
         WHERE state = 'queued' AND class = ?1
           AND (require_label IS NULL OR require_label IN (SELECT value FROM json_each(?2)))
           AND (avoid_worker IS NULL OR avoid_worker != ?3 OR queued_at <= ?4)
         ORDER BY created_at, rowid LIMIT 1",
    )?;
    while out.len() < max {
        let mut best: Option<(i64, i64, String, String)> = None; // (effective class, created_at, id, payload)
        for class in CLASSES {
            let found = head
                .query_row(
                    params![class as i64, labels, worker.id, now - cfg.retry_grace_ms],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((id, created_at, payload)) = found else {
                continue;
            };
            let promoted = if cfg.aging_ms > 0 {
                (now - created_at).max(0) / cfg.aging_ms
            } else {
                0
            };
            // Promotion stops at class 1: nothing but a tournament job is ever served before a tournament
            // job (docs/JOBS.md), while ranked, unranked and bench cannot starve one another.
            let floor = (class as i64).min(1);
            let effective = (class as i64 - promoted).max(floor);
            // Strict `<`: on a tie the higher class (visited first) wins.
            if best
                .as_ref()
                .is_none_or(|b| (effective, created_at) < (b.0, b.1))
            {
                best = Some((effective, created_at, id, payload));
            }
        }
        let Some((_, _, id, payload)) = best else {
            break;
        };
        let token = random_hex(16);
        let expires = now + cfg.lease_ms;
        tx.execute(
            "UPDATE jobs SET state = 'leased', attempts = attempts + 1, lease_token = ?2, lease_expires_at = ?3, worker_id = ?4
             WHERE id = ?1 AND state = 'queued'",
            params![id, token, expires, worker.id],
        )?;
        out.push(wire_job(&id, &token, expires, &payload)?);
    }
    Ok(out)
}

/// The job, if `token` is its live lease held by `worker`. 404 for an unknown job, 409 otherwise.
fn live_lease(job: &JobRow, token: &str, worker: &WorkerRow, now: i64) -> bool {
    job.state == "leased"
        && job.lease_token.as_deref() == Some(token)
        && job.worker_id.as_deref() == Some(worker.id.as_str())
        && job.lease_expires_at.is_some_and(|t| t > now)
}

pub fn heartbeat(
    tx: &Transaction,
    job_id: &str,
    token: &str,
    worker: &WorkerRow,
    now: i64,
    cfg: &QueueCfg,
) -> ApiResult<i64> {
    let job = load(tx, job_id)?.ok_or_else(|| ApiError::not_found(format!("no job {job_id:?}")))?;
    if !live_lease(&job, token, worker, now) {
        return Err(ApiError::lease_lost());
    }
    let expires = now + cfg.lease_ms;
    tx.execute(
        "UPDATE jobs SET lease_expires_at = ?2 WHERE id = ?1",
        params![job_id, expires],
    )?;
    Ok(expires)
}

// ---- complete ----

#[derive(Debug, Default)]
pub struct Completed {
    /// The job already had this result (repeat call, or the same replay from a stale lease).
    pub noop: bool,
    /// A trusted re-run was queued for this result.
    pub verify_enqueued: bool,
    /// A trusted re-run agreed with an untrusted result.
    pub confirmed: bool,
    /// A trusted re-run disagreed: the untrusted worker was quarantined.
    pub mismatch: bool,
    /// Two trusted results disagree: a determinism bug, nobody is quarantined.
    pub conflict: bool,
    /// Results invalidated and requeued by a quarantine.
    pub requeued: usize,
}

/// Store the result of a leased job. `sample(class)` decides whether an untrusted result is spot-verified.
/// The caller has already checked that `replay_sha256` is well formed and that its blob exists.
pub fn complete(
    tx: &Transaction,
    job_id: &str,
    token: &str,
    result: &Value,
    replay_sha256: &str,
    worker: &WorkerRow,
    now: i64,
    sample: &dyn Fn(Class) -> bool,
) -> ApiResult<Completed> {
    let job = load(tx, job_id)?.ok_or_else(|| ApiError::not_found(format!("no job {job_id:?}")))?;
    let mut out = Completed::default();
    if job.state == "done" {
        // Idempotent for the same lease token; a different lease that reproduces the stored replay changes
        // nothing either. Anything else is a stale lease.
        if job.lease_token.as_deref() == Some(token)
            || job.replay_sha256.as_deref() == Some(replay_sha256)
        {
            out.noop = true;
            return Ok(out);
        }
        return Err(ApiError::lease_lost());
    }
    if !live_lease(&job, token, worker, now) {
        return Err(ApiError::lease_lost());
    }
    let result_text = serde_json::to_string(result).map_err(ApiError::internal)?;
    tx.execute(
        "UPDATE jobs SET state = 'done', result = ?2, replay_sha256 = ?3, verified = ?4, finished_at = ?5,
                lease_expires_at = NULL, error = NULL, avoid_worker = NULL WHERE id = ?1",
        params![job.id, result_text, replay_sha256, worker.trusted, now],
    )?;

    if job.kind == "verify" {
        if let Some(original) = job.verifies.as_deref() {
            apply_verification(
                tx,
                original,
                &result_text,
                replay_sha256,
                &worker.id,
                now,
                &mut out,
            )?;
        }
    } else if !worker.trusted {
        match load_by_key(tx, &format!("verify:{}", job.key))? {
            // A trusted result for this match already exists (this job was invalidated and re-run).
            Some(v) if v.state == "done" => {
                let (v_result, v_sha) = (
                    v.result.unwrap_or_default(),
                    v.replay_sha256.unwrap_or_default(),
                );
                let v_worker = v.worker_id.unwrap_or_default();
                apply_verification(tx, &job.id, &v_result, &v_sha, &v_worker, now, &mut out)?;
            }
            Some(_) => {}
            None if sample(Class::from_i64(job.class)) => {
                tx.execute(
                    "INSERT INTO jobs(id, kind, class, key, payload, state, require_label, verifies, created_at, queued_at)
                     VALUES (?1, 'verify', ?2, ?3, ?4, 'queued', ?5, ?6, ?7, ?7)",
                    params![format!("v{}", &job.key[..24]), job.class, format!("verify:{}", job.key), job.payload, TRUSTED_LABEL, job.id, now],
                )?;
                out.verify_enqueued = true;
            }
            None => {}
        }
    }
    Ok(out)
}

/// Compare the trusted result (`v_*`) with the job it verifies.
fn apply_verification(
    tx: &Transaction,
    original_id: &str,
    v_result: &str,
    v_sha: &str,
    v_worker: &str,
    now: i64,
    out: &mut Completed,
) -> ApiResult<()> {
    let Some(original) = load(tx, original_id)? else {
        return Ok(());
    };
    let adopt = |tx: &Transaction| {
        tx.execute(
            "UPDATE jobs SET state = 'done', result = ?2, replay_sha256 = ?3, verified = 1, worker_id = ?4,
                    finished_at = COALESCE(finished_at, ?5), lease_token = NULL, lease_expires_at = NULL, error = NULL
             WHERE id = ?1",
            params![original_id, v_result, v_sha, v_worker, now],
        )
    };
    if original.state != "done" {
        // The original was invalidated (or died) and is waiting for a re-run: the trusted result settles it.
        adopt(tx)?;
        return Ok(());
    }
    if original.replay_sha256.as_deref() == Some(v_sha) {
        tx.execute("UPDATE jobs SET verified = 1 WHERE id = ?1", [original_id])?;
        out.confirmed = true;
    } else if original.verified {
        eprintln!(
            "DETERMINISM CONFLICT on job {original_id}: trusted workers {} and {v_worker} produced different replays",
            original.worker_id.as_deref().unwrap_or("?")
        );
        out.conflict = true;
    } else {
        // The trusted result wins; the original worker is flagged and its unverified results are re-queued.
        adopt(tx)?;
        out.mismatch = true;
        if let Some(bad) = original.worker_id.as_deref() {
            let reason = format!("spot verification mismatch on job {original_id}");
            out.requeued = quarantine(tx, bad, &reason, now)?;
        }
    }
    Ok(())
}

const ON_CURRENT_PATCH: &str = "EXISTS (SELECT 1 FROM patches p WHERE p.current = 1
                       AND p.engine = json_extract(jobs.payload, '$.patch.engine')
                       AND p.ruleset_hash = json_extract(jobs.payload, '$.patch.ruleset_hash')
                       AND p.wasmtime = json_extract(jobs.payload, '$.patch.wasmtime'))";

/// Quarantine a worker: it gets no more jobs, its running leases are revoked and every result of its that
/// no trusted worker has confirmed is thrown away and queued again. Returns the number of such results.
pub fn quarantine(tx: &Transaction, worker_id: &str, reason: &str, now: i64) -> ApiResult<usize> {
    let known = tx.execute(
        "UPDATE workers SET quarantined = 1, quarantine_reason = ?2 WHERE id = ?1",
        params![worker_id, reason],
    )?;
    if known == 0 {
        return Err(ApiError::not_found(format!("no worker {worker_id:?}")));
    }
    // A revoked lease is not the job's fault: it does not count as an attempt.
    tx.execute(
        "UPDATE jobs SET state = 'queued', lease_token = NULL, lease_expires_at = NULL, queued_at = ?2,
                attempts = MAX(attempts - 1, 0) WHERE worker_id = ?1 AND state = 'leased'",
        params![worker_id, now],
    )?;
    // Only results on the current patch can be re-run; older ones stay as they are (unverified).
    let suspect = format!("worker_id = ?1 AND state = 'done' AND verified = 0 AND kind = 'match' AND {ON_CURRENT_PATCH}");
    // Series that were already closed keep their score and rating; say so on the series.
    tx.execute(
        &format!(
            "UPDATE series SET note = 'a result was invalidated after this series closed; ratings were not rolled back'
             WHERE state = 'done' AND id IN (SELECT g.series_id FROM games g JOIN jobs ON jobs.id = g.job_id WHERE {suspect})"
        ),
        [worker_id],
    )?;
    let invalidated = tx.execute(
        &format!(
            "UPDATE jobs SET state = 'queued', result = NULL, replay_sha256 = NULL, finished_at = NULL, lease_token = NULL,
                    worker_id = NULL, attempts = 0, queued_at = ?2 WHERE {suspect}"
        ),
        params![worker_id, now],
    )?;
    Ok(invalidated)
}

// ---- fail, expiry ----

#[derive(Debug, PartialEq, Eq)]
pub enum Failed {
    Requeued,
    Dead,
    /// Repeat of a `fail` that was already applied.
    Noop,
}

pub fn fail(
    tx: &Transaction,
    job_id: &str,
    token: &str,
    error: &str,
    retryable: bool,
    worker: &WorkerRow,
    now: i64,
    cfg: &QueueCfg,
) -> ApiResult<Failed> {
    let job = load(tx, job_id)?.ok_or_else(|| ApiError::not_found(format!("no job {job_id:?}")))?;
    // After a `fail` the row keeps its lease token until the next lease, so a repeat is recognised.
    if matches!(job.state.as_str(), "queued" | "dead") && job.lease_token.as_deref() == Some(token)
    {
        return Ok(Failed::Noop);
    }
    if !live_lease(&job, token, worker, now) {
        return Err(ApiError::lease_lost());
    }
    let error: String = error.chars().take(2000).collect();
    if retryable && job.attempts < cfg.max_attempts {
        tx.execute(
            "UPDATE jobs SET state = 'queued', lease_expires_at = NULL, queued_at = ?2, error = ?3, avoid_worker = ?4 WHERE id = ?1",
            params![job.id, now, error, worker.id],
        )?;
        Ok(Failed::Requeued)
    } else {
        tx.execute(
            "UPDATE jobs SET state = 'dead', lease_expires_at = NULL, finished_at = ?2, error = ?3 WHERE id = ?1",
            params![job.id, now, error],
        )?;
        Ok(Failed::Dead)
    }
}

/// Return jobs whose lease ran out to the queue, or to the dead-letter list once their attempts are used up.
/// Returns `(requeued, dead)`.
pub fn sweep_expired(tx: &Transaction, now: i64, cfg: &QueueCfg) -> ApiResult<(usize, usize)> {
    let dead = tx.execute(
        "UPDATE jobs SET state = 'dead', error = 'lease expired; attempts exhausted', finished_at = ?1,
                lease_token = NULL, lease_expires_at = NULL
         WHERE state = 'leased' AND lease_expires_at <= ?1 AND attempts >= ?2",
        params![now, cfg.max_attempts],
    )?;
    let requeued = tx.execute(
        "UPDATE jobs SET state = 'queued', lease_token = NULL, lease_expires_at = NULL, queued_at = ?1
         WHERE state = 'leased' AND lease_expires_at <= ?1",
        [now],
    )?;
    Ok((requeued, dead))
}

// ---- admin views ----

const VIEW_COLS: &str = "id, kind, class, key, state, attempts, worker_id, lease_expires_at, verified, error, result, payload, created_at, finished_at, verifies";

fn view(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(json!({
        "job_id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?,
        "class": Class::from_i64(r.get(2)?).name(), "key": r.get::<_, String>(3)?,
        "state": r.get::<_, String>(4)?, "attempts": r.get::<_, i64>(5)?,
        "worker_id": r.get::<_, Option<String>>(6)?, "lease_expires_at": opt_rfc3339(r.get(7)?),
        "verified": r.get::<_, bool>(8)?, "error": r.get::<_, Option<String>>(9)?,
        "result": parse(r.get(10)?), "spec": parse(r.get(11)?),
        "created_at": rfc3339(r.get(12)?), "finished_at": opt_rfc3339(r.get(13)?),
        "verifies": r.get::<_, Option<String>>(14)?,
    }))
}

pub fn job_view(tx: &Transaction, id: &str) -> ApiResult<Option<Value>> {
    Ok(tx
        .query_row(
            &format!("SELECT {VIEW_COLS} FROM jobs WHERE id = ?1"),
            [id],
            view,
        )
        .optional()?)
}

/// The dead-letter list, most recent first.
pub fn dead_jobs(tx: &Transaction, limit: i64) -> ApiResult<Vec<Value>> {
    let mut st = tx.prepare(&format!(
        "SELECT {VIEW_COLS} FROM jobs WHERE state = 'dead' ORDER BY finished_at DESC LIMIT ?1"
    ))?;
    let rows = st.query_map([limit], view)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A human decided a dead job deserves another three attempts.
pub fn requeue_dead(tx: &Transaction, id: &str, now: i64) -> ApiResult<()> {
    let n = tx.execute(
        &format!(
        "UPDATE jobs SET state = 'queued', attempts = 0, error = NULL, finished_at = NULL, lease_token = NULL,
                avoid_worker = NULL, queued_at = ?2 WHERE id = ?1 AND state = 'dead' AND {ON_CURRENT_PATCH}"
        ),
        params![id, now],
    )?;
    if n == 0 {
        return Err(ApiError::conflict(
            "not_dead",
            format!("job {id:?} is not on the dead-letter list, or belongs to a superseded patch"),
        ));
    }
    Ok(())
}

// ---- metrics ----

#[derive(Debug, Default)]
pub struct QueueStats {
    /// Per class (index = class number): queued jobs and the age in ms of the oldest one.
    pub queued: [(i64, i64); 4],
    pub leased: i64,
    pub dead: i64,
    pub verify_pending: i64,
    pub done_last_minute: i64,
    pub live_workers: i64,
    pub live_cores: i64,
    pub quarantined_workers: i64,
}

pub fn stats(tx: &Transaction, now: i64, worker_live_ms: i64) -> ApiResult<QueueStats> {
    let mut s = QueueStats::default();
    {
        let mut st = tx.prepare("SELECT class, COUNT(*), MIN(created_at) FROM jobs WHERE state = 'queued' GROUP BY class")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for r in rows {
            let (class, count, oldest) = r?;
            if let Some(slot) = s.queued.get_mut(class as usize) {
                *slot = (count, (now - oldest).max(0));
            }
        }
    }
    let count = |sql: &str| -> ApiResult<i64> { Ok(tx.query_row(sql, [], |r| r.get(0))?) };
    s.leased = count("SELECT COUNT(*) FROM jobs WHERE state = 'leased'")?;
    s.dead = count("SELECT COUNT(*) FROM jobs WHERE state = 'dead'")?;
    s.verify_pending =
        count("SELECT COUNT(*) FROM jobs WHERE state IN ('queued', 'leased') AND kind = 'verify'")?;
    s.quarantined_workers = count("SELECT COUNT(*) FROM workers WHERE quarantined = 1")?;
    s.done_last_minute = tx.query_row(
        "SELECT COUNT(*) FROM jobs WHERE finished_at > ?1 AND state = 'done'",
        [now - 60_000],
        |r| r.get(0),
    )?;
    (s.live_workers, s.live_cores) = tx.query_row(
        "SELECT COUNT(*), COALESCE(SUM(cores), 0) FROM workers WHERE last_seen >= ?1 AND quarantined = 0",
        [now - worker_live_ms],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(s)
}
