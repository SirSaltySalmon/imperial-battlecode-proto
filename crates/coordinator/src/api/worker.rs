//! Coordinator <-> worker lease protocol v1, exactly as in docs/JOBS.md part B.

use super::{auth_admin, auth_worker, parse, AppState};
use crate::db::{self, jobs};
use crate::error::{ApiError, ApiResult};
use crate::model::Class;
use crate::util::{is_sha256, new_token, now_ms, rfc3339, secret_eq, sha256_hex};
use crate::{bump, App};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Deserialize)]
struct RegisterReq {
    registration_token: String,
    name: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    cores: i64,
}

pub async fn register(State(app): AppState, body: Bytes) -> ApiResult<Json<Value>> {
    let req: RegisterReq = parse(&body)?;
    let matches = |secret: &str| !secret.is_empty() && secret_eq(&req.registration_token, secret);
    let trusted = app
        .cfg
        .trusted_registration_token
        .as_deref()
        .is_some_and(matches);
    if !trusted && !matches(&app.cfg.registration_token) {
        return Err(ApiError::unauthorized());
    }
    if req.name.is_empty()
        || req.name.len() > 100
        || req.labels.len() > 32
        || req.labels.iter().any(|l| l.len() > 100)
    {
        return Err(ApiError::bad_request(
            "name must be 1..=100 bytes; at most 32 labels of at most 100 bytes",
        ));
    }
    if !(0..=65_536).contains(&req.cores) {
        return Err(ApiError::bad_request("cores must be between 0 and 65536"));
    }
    let token = new_token("ibw");
    let hash = sha256_hex(token.as_bytes());
    let worker_id = app.db.tx(|tx| {
        db::register_worker(
            tx,
            &req.name,
            &req.labels,
            req.cores,
            &hash,
            trusted,
            now_ms(),
        )
    })?;
    Ok(Json(json!({"worker_id": worker_id, "token": token})))
}

#[derive(Deserialize)]
struct LeaseReq {
    #[serde(default)]
    max: i64,
    #[serde(default)]
    wait_ms: i64,
}

/// Long poll: answers as soon as there is a job for this worker, or with `{"jobs": []}` after
/// `min(wait_ms, 25000)` ms.
pub async fn lease(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let token_hash = sha256_hex(super::bearer(&headers)?.as_bytes());
    auth_worker(&app, &headers)?;
    let req: LeaseReq = parse(&body)?;
    let max = req.max.clamp(1, 256) as usize;
    let wait = Duration::from_millis((req.wait_ms.max(0) as u64).min(app.cfg.max_wait_ms));
    let deadline = Instant::now() + wait;
    let cfg = app.queue_cfg();
    loop {
        // Register for wake-ups before looking at the queue, so a job queued in between is not missed.
        let changed = app.queue_changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let jobs = app.db.tx(|tx| {
            // Re-read the worker each time: it may have been quarantined or trusted while it waited.
            let now = now_ms();
            let worker =
                db::worker_by_token(tx, &token_hash, now)?.ok_or_else(ApiError::unauthorized)?;
            jobs::lease(tx, &worker, max, now, &cfg)
        })?;
        let now = Instant::now();
        if !jobs.is_empty() || now >= deadline {
            return Ok(Json(json!({"jobs": jobs})));
        }
        // Also re-check every second: retry grace periods end without any event.
        let until = deadline.min(now + Duration::from_secs(1));
        tokio::select! {
            _ = &mut changed => {}
            _ = tokio::time::sleep_until(until) => {}
        }
    }
}

#[derive(Deserialize)]
struct HeartbeatReq {
    lease_token: String,
}

pub async fn heartbeat(
    State(app): AppState,
    Path(job_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let worker = auth_worker(&app, &headers)?;
    let req: HeartbeatReq = parse(&body)?;
    let cfg = app.queue_cfg();
    let expires = app
        .db
        .tx(|tx| jobs::heartbeat(tx, &job_id, &req.lease_token, &worker, now_ms(), &cfg))?;
    Ok(Json(
        json!({"lease_expires_at": rfc3339(expires), "lease_secs": jobs::lease_secs(&cfg)}),
    ))
}

/// Largest accepted `complete` body.
const MAX_RESULT_BYTES: usize = 256 << 10;

#[derive(Deserialize)]
struct CompleteReq {
    lease_token: String,
    result: Value,
}

/// `Result` of docs/JOBS.md: `winner` 0 | 1 | null, `outcome` consistent with it, `score` two integers,
/// `ticks`, `replay_sha256`, `summary`, optional `wall_ms`. Returns the replay hash.
fn check_result(result: &Value) -> ApiResult<&str> {
    let bad = |what: &str| ApiError::bad_request(format!("result.{what}"));
    if !result.is_object() {
        return Err(ApiError::bad_request("result must be an object"));
    }
    let winner = match result.get("winner") {
        None | Some(Value::Null) => None,
        Some(w) if matches!(w.as_i64(), Some(0 | 1)) => w.as_i64(),
        Some(_) => return Err(bad("winner must be 0, 1 or null")),
    };
    // `outcome` is optional for workers older than its introduction (the summary has it too), but when it is
    // there it must agree with `winner`: ratings are taken from it.
    // A worker that sends no `outcome` must have it in the summary: a result without either would be taken
    // for an unfinished game and silently left out of rating.
    let outcome = result
        .get("outcome")
        .or_else(|| result.get("summary")?.get("outcome"))
        .map(Value::as_str);
    match (outcome, winner) {
        (Some(Some("a")), Some(0)) | (Some(Some("b")), Some(1)) => {}
        (Some(Some("draw" | "unfinished")), None) => {}
        _ => {
            return Err(bad(
                "outcome must be \"a\" | \"b\" | \"draw\" | \"unfinished\" and agree with winner",
            ))
        }
    }
    if result.get("wall_ms").is_some_and(|v| !v.is_u64()) {
        return Err(bad("wall_ms must be a non-negative integer"));
    }
    let score_ok = result
        .get("score")
        .and_then(Value::as_array)
        .is_some_and(|s| s.len() == 2 && s.iter().all(Value::is_i64));
    if !score_ok {
        return Err(bad("score must be two integers"));
    }
    if !result.get("ticks").is_some_and(Value::is_u64) {
        return Err(bad("ticks must be a non-negative integer"));
    }
    result
        .get("replay_sha256")
        .and_then(Value::as_str)
        .filter(|s| is_sha256(s))
        .ok_or_else(|| bad("replay_sha256 must be 64 lower-case hex digits"))
}

/// Whether a result from an untrusted worker is re-run by a trusted one.
fn sample_for_verification(app: &App, class: Class) -> bool {
    let fraction = app.cfg.verify_fraction;
    fraction > 0.0 && (class == Class::Tournament || rand::random::<f64>() < fraction)
}

pub async fn complete(
    State(app): AppState,
    Path(job_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let worker = auth_worker(&app, &headers)?;
    // The result (with the runner's summary.json) is stored in the job row: keep it small.
    if body.len() > MAX_RESULT_BYTES {
        return Err(ApiError::bad_request(format!(
            "the request is larger than {MAX_RESULT_BYTES} bytes"
        )));
    }
    let req: CompleteReq = parse(&body)?;
    let replay = check_result(&req.result)?;
    if !app.blobs.exists(replay) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "replay_missing",
            format!("replay blob {replay} has not been uploaded"),
        ));
    }
    let now = now_ms();
    let done = app.db.tx(|tx| {
        let sample = |class: Class| sample_for_verification(&app, class);
        let done = jobs::complete(
            tx,
            &job_id,
            &req.lease_token,
            &req.result,
            replay,
            &worker,
            now,
            &sample,
        )?;
        if !done.noop {
            app.after_quarantine(tx, &done.quarantined, now)?;
            db::league::finalize_ready(
                tx,
                app.rating.as_ref(),
                now,
                app.cfg.ladder_snapshots_kept,
            )?;
        }
        Ok(done)
    })?;
    let c = &app.counters;
    bump(&c.completed, usize::from(!done.noop));
    bump(&c.verify_confirmed, usize::from(done.confirmed));
    bump(&c.verify_mismatch, usize::from(done.mismatch));
    bump(&c.verify_conflict, usize::from(done.conflict));
    if done.mismatch {
        let q = &done.quarantined;
        eprintln!(
            "spot verification mismatch on job {job_id}: a worker was quarantined, {} results requeued, {} closed series reopened",
            q.requeued, q.reopened
        );
    }
    if done.verify_enqueued || done.mismatch {
        app.queue_changed.notify_waiters();
    }
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct FailReq {
    lease_token: String,
    #[serde(default)]
    error: String,
    retryable: bool,
    /// `"timeout"`: the match ran past the worker's wall-clock limit. Other values are ignored.
    #[serde(default)]
    code: Option<String>,
}

pub async fn fail(
    State(app): AppState,
    Path(job_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let worker = auth_worker(&app, &headers)?;
    let req: FailReq = parse(&body)?;
    let now = now_ms();
    let report = jobs::FailReport {
        error: &req.error,
        retryable: req.retryable,
        code: req.code.as_deref(),
    };
    let outcome = app.db.tx(|tx| {
        let outcome = jobs::fail(
            tx,
            &job_id,
            &req.lease_token,
            &report,
            &worker,
            now,
            &app.queue_cfg(),
        )?;
        if outcome == jobs::Failed::Dead {
            db::league::finalize_ready(
                tx,
                app.rating.as_ref(),
                now,
                app.cfg.ladder_snapshots_kept,
            )?;
        }
        Ok(outcome)
    })?;
    match outcome {
        jobs::Failed::Requeued => {
            bump(&app.counters.failed, 1);
            app.queue_changed.notify_waiters();
        }
        jobs::Failed::Dead => {
            bump(&app.counters.failed, 1);
            bump(&app.counters.dead, 1);
            eprintln!("job {job_id} dead-lettered: {}", req.error);
        }
        jobs::Failed::Noop => {}
    }
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct ReleaseReq {
    lease_token: String,
    #[serde(default)]
    reason: String,
}

/// "Not for me": the worker gives the job back without spending an attempt (wrong patch, unknown kind,
/// shutting down). The job is not offered to this worker again for the retry grace period.
pub async fn release(
    State(app): AppState,
    Path(job_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let worker = auth_worker(&app, &headers)?;
    let req: ReleaseReq = parse(&body)?;
    let released = app.db.tx(|tx| {
        jobs::release(
            tx,
            &job_id,
            &req.lease_token,
            &req.reason,
            &worker,
            now_ms(),
        )
    })?;
    if released {
        app.queue_changed.notify_waiters();
    }
    Ok(Json(json!({})))
}

/// Blobs are readable and writable by workers and by the organisers.
fn auth_blob(app: &App, headers: &HeaderMap) -> ApiResult<()> {
    auth_admin(app, headers).or_else(|_| auth_worker(app, headers).map(|_| ()))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> ApiResult<std::io::Result<T>> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(ApiError::internal)
}

/// Send a blob as `application/octet-stream`, or 404.
pub(crate) async fn blob_response(app: Arc<App>, sha256: String) -> ApiResult<Response> {
    let store = app.blobs.clone();
    let key = sha256.clone();
    match blocking(move || store.get(&key)).await?? {
        Some(bytes) => {
            Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
        }
        None => Err(ApiError::not_found(format!("no blob {sha256}"))),
    }
}

pub async fn get_blob(
    State(app): AppState,
    Path(sha256): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    auth_blob(&app, &headers)?;
    blob_response(app, sha256).await
}

pub async fn put_blob(
    State(app): AppState,
    Path(sha256): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    auth_blob(&app, &headers)?;
    if !is_sha256(&sha256) {
        return Err(ApiError::bad_request(
            "the blob name must be 64 lower-case hex digits",
        ));
    }
    let store = app.blobs.clone();
    match blocking(move || store.put(&sha256, &body)).await? {
        Ok(true) => Ok((StatusCode::CREATED, Json(json!({}))).into_response()),
        Ok(false) => Ok((StatusCode::OK, Json(json!({}))).into_response()),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "hash_mismatch",
            "the content does not hash to the sha256 in the path",
        )),
        Err(e) => Err(e.into()),
    }
}
