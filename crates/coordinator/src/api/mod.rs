//! HTTP surface: routes, authentication, request parsing.
//!
//! Bodies are parsed by hand from the raw bytes so that every malformed request gets the protocol's
//! `400 {"error", "message"}` instead of the framework's default rejections.

mod manage;
mod public;
mod worker;

use crate::db::{self, TeamRow, WorkerRow};
use crate::error::{ApiError, ApiResult};
use crate::util::{now_ms, secret_eq, sha256_hex};
use crate::App;
use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderMap};
use axum::routing::{get, post, put};
use axum::Router;
use serde::de::DeserializeOwned;
use std::sync::Arc;

pub type AppState = axum::extract::State<Arc<App>>;

pub fn router(app: Arc<App>) -> Router {
    let limit = app
        .cfg
        .max_blob_bytes
        .max(app.cfg.max_wasm_bytes)
        .max(app.cfg.max_map_bytes);
    Router::new()
        // worker protocol v1 (docs/JOBS.md)
        .route("/v1/workers/register", post(worker::register))
        .route("/v1/jobs/lease", post(worker::lease))
        .route("/v1/jobs/{job_id}/heartbeat", post(worker::heartbeat))
        .route("/v1/jobs/{job_id}/complete", post(worker::complete))
        .route("/v1/jobs/{job_id}/fail", post(worker::fail))
        .route("/v1/jobs/{job_id}/release", post(worker::release))
        .route(
            "/v1/blobs/{sha256}",
            get(worker::get_blob).put(worker::put_blob),
        )
        // teams
        .route("/v1/team", get(manage::whoami))
        .route(
            "/v1/submissions",
            post(manage::upload_submission).get(manage::list_submissions),
        )
        .route(
            "/v1/submissions/{submission_id}/live",
            post(manage::select_live),
        )
        .route("/v1/series", post(manage::create_series))
        // organisers
        .route(
            "/v1/admin/teams",
            post(manage::create_team).get(manage::list_teams),
        )
        .route("/v1/admin/maps/{name}", put(manage::upload_map))
        .route("/v1/admin/patch", post(manage::set_patch))
        .route("/v1/admin/ranked-batch", post(manage::ranked_batch))
        .route("/v1/admin/jobs", post(manage::enqueue_job))
        .route("/v1/admin/jobs/{job_id}", get(manage::get_job))
        .route("/v1/admin/jobs/{job_id}/requeue", post(manage::requeue_job))
        .route("/v1/admin/dead", get(manage::dead_jobs))
        .route("/v1/admin/workers", get(manage::list_workers))
        .route(
            "/v1/admin/workers/{worker_id}/trust",
            post(manage::trust_worker),
        )
        .route(
            "/v1/admin/workers/{worker_id}/quarantine",
            post(manage::quarantine_worker),
        )
        // public
        .route("/v1/ladder", get(public::ladder))
        .route("/v1/matches", get(public::matches))
        .route("/v1/matches/{match_id}", get(public::get_match))
        .route("/v1/matches/{match_id}/replay", get(public::replay))
        .route("/v1/series/{series_id}", get(public::get_series))
        .route("/v1/maps", get(public::maps))
        .route("/v1/patch", get(public::patch))
        .route("/metrics", get(public::metrics))
        .route("/healthz", get(public::healthz))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(app)
}

/// Parse a JSON request body; any failure is a 400.
pub(crate) fn parse<T: DeserializeOwned>(body: &[u8]) -> ApiResult<T> {
    serde_json::from_slice(body)
        .map_err(|e| ApiError::bad_request(format!("malformed JSON body: {e}")))
}

pub(crate) fn bearer(headers: &HeaderMap) -> ApiResult<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(ApiError::unauthorized)
}

fn is_admin(app: &App, token: &str) -> bool {
    !app.cfg.admin_token.is_empty() && secret_eq(token, &app.cfg.admin_token)
}

pub(crate) fn auth_admin(app: &App, headers: &HeaderMap) -> ApiResult<()> {
    if is_admin(app, bearer(headers)?) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

/// The worker behind the bearer token; also records that it is alive.
pub(crate) fn auth_worker(app: &App, headers: &HeaderMap) -> ApiResult<WorkerRow> {
    let hash = sha256_hex(bearer(headers)?.as_bytes());
    app.db
        .tx(|tx| db::worker_by_token(tx, &hash, now_ms()))?
        .ok_or_else(ApiError::unauthorized)
}

pub(crate) enum Caller {
    Admin,
    Team(TeamRow),
}

/// A team token or the admin token.
pub(crate) fn auth_team_or_admin(app: &App, headers: &HeaderMap) -> ApiResult<Caller> {
    let token = bearer(headers)?;
    if is_admin(app, token) {
        return Ok(Caller::Admin);
    }
    let hash = sha256_hex(token.as_bytes());
    app.db
        .tx(|tx| db::team_by_token(tx, &hash))?
        .map(Caller::Team)
        .ok_or_else(ApiError::unauthorized)
}
