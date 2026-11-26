//! HTTP surface: routes, authentication, request parsing.
//!
//! Bodies are parsed by hand from the raw bytes so that every malformed request gets the protocol's
//! `400 {"error", "message"}` instead of the framework's default rejections.

mod auth;
mod manage;
mod public;
mod teams;
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
        // accounts and teams of users (docs/api/auth.md)
        .route("/v1/auth/register", post(auth::register))
        .route("/v1/auth/login", post(auth::login))
        .route("/v1/auth/logout", post(auth::logout))
        .route("/v1/auth/device", post(auth::device_start))
        .route("/v1/auth/device/approve", post(auth::device_approve))
        .route("/v1/auth/device/token", post(auth::device_token))
        .route("/v1/me", get(auth::me))
        .route("/v1/teams", post(teams::create))
        .route("/v1/teams/join", post(teams::join))
        .route("/v1/teams/leave", post(teams::leave))
        .route("/v1/teams/join-code/rotate", post(teams::rotate_join_code))
        .route("/v1/team/token", post(teams::regenerate_token))
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
        .layer(axum::middleware::from_fn(auth::csrf_guard))
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

/// The admin token, or a signed-in user with `is_admin`.
pub(crate) fn auth_admin(app: &App, headers: &HeaderMap) -> ApiResult<()> {
    match auth::identify(app, headers)? {
        auth::Identity::Admin => Ok(()),
        auth::Identity::User(u) if u.is_admin => Ok(()),
        _ => Err(ApiError::unauthorized()),
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

/// A team token, the admin token, or a user (`ibu` token or session cookie) acting for their team.
pub(crate) fn auth_team_or_admin(app: &App, headers: &HeaderMap) -> ApiResult<Caller> {
    match auth::identify(app, headers)? {
        auth::Identity::Admin => Ok(Caller::Admin),
        auth::Identity::Team(team) => Ok(Caller::Team(team)),
        auth::Identity::User(user) => auth::user_caller(app, user),
    }
}
