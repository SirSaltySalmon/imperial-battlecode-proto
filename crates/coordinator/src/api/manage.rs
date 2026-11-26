//! Team and organiser endpoints: teams, submissions, maps, the patch, series, the job queue, workers.

use super::{auth_admin, auth_team_or_admin, parse, AppState, Caller};
use crate::db::{self, jobs, league, NewSubmission, TeamRow};
use crate::error::{ApiError, ApiResult};
use crate::model::{BotRef, Class, JobSpec, MapRef, Patch};
use crate::util::{is_sha256, new_token, now_ms, sha256_hex};
use crate::wasmcheck::{check_wasm, Limits};
use crate::{bump, App};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

type Created = (StatusCode, Json<Value>);

fn valid_name(name: &str, max: usize) -> bool {
    !name.is_empty()
        && name.len() <= max
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn finalize(app: &App, tx: &rusqlite::Transaction, now: i64) -> ApiResult<usize> {
    league::finalize_ready(tx, app.rating.as_ref(), now, app.cfg.ladder_snapshots_kept)
}

// ---- teams ----

#[derive(Deserialize)]
struct CreateTeamReq {
    name: String,
}

/// The token is returned once; only its hash is stored.
pub async fn create_team(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Created> {
    auth_admin(&app, &headers)?;
    let req: CreateTeamReq = parse(&body)?;
    let (team, token) = app.db.tx(|tx| insert_team(tx, &req.name, now_ms()))?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"team_id": team.id, "name": team.name, "token": token})),
    ))
}

/// Create a team with a fresh `ibt` token; returns the team and the token (only its hash is stored). Used
/// by the organisers' endpoint and by users creating a team (`api::teams`).
pub(crate) fn insert_team(
    tx: &rusqlite::Transaction,
    name: &str,
    now: i64,
) -> ApiResult<(TeamRow, String)> {
    if !valid_name(name, 40) {
        return Err(ApiError::bad_request(
            "team name must be 1..=40 characters of [A-Za-z0-9_.-]",
        ));
    }
    let token = new_token("ibt");
    let team = db::create_team(tx, name, &sha256_hex(token.as_bytes()), now)?;
    Ok((team, token))
}

pub async fn list_teams(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    Ok(Json(json!({"teams": app.db.tx(db::list_teams)?})))
}

pub async fn whoami(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    match auth_team_or_admin(&app, &headers)? {
        Caller::Admin => Ok(Json(json!({"admin": true}))),
        Caller::Team(team) => {
            let live = app.db.tx(|tx| db::live_submission(tx, &team.id))?;
            Ok(Json(
                json!({"team_id": team.id, "name": team.name, "live_submission": live.map(|s| s.id)}),
            ))
        }
    }
}

// ---- submissions ----

#[derive(Deserialize)]
pub struct SubmissionQuery {
    language: Option<String>,
    /// Admin only: the team to upload for (id or name).
    team: Option<String>,
    /// Upload only: make the module live if it validates (default true).
    live: Option<bool>,
}

/// The team a request acts for: the token's team, or `?team=` when the organisers call.
pub(crate) fn acting_team(app: &App, caller: Caller, team: Option<&str>) -> ApiResult<TeamRow> {
    match (caller, team) {
        (Caller::Team(t), _) => Ok(t),
        (Caller::Admin, Some(team)) => app.db.tx(|tx| db::find_team(tx, team)),
        (Caller::Admin, None) => Err(ApiError::bad_request(
            "the admin token needs ?team=<id or name>",
        )),
    }
}

/// Body: the raw wasm module. A module that validates becomes the team's live submission at once, unless
/// `?live=false`. Series that already exist keep the submissions they were created with.
pub async fn upload_submission(
    State(app): AppState,
    Query(q): Query<SubmissionQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Created> {
    let team = acting_team(&app, auth_team_or_admin(&app, &headers)?, q.team.as_deref())?;
    let language = q.language.unwrap_or_else(|| "rust".to_string());
    let fuel_mult = app
        .cfg
        .fuel_multiplier(&language)
        .ok_or_else(|| ApiError::bad_request(format!("unknown language {language:?}")))?;
    if body.len() > app.cfg.max_wasm_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            format!("the limit is {} bytes", app.cfg.max_wasm_bytes),
        ));
    }
    let limits = Limits {
        max_bytes: app.cfg.max_wasm_bytes,
        max_memory_bytes: app.cfg.max_memory_bytes,
    };
    let store = app.blobs.clone();
    let size = body.len();
    // Validation and the disk write are CPU / IO bound: keep them off the async threads.
    let (sha256, verdict) = tokio::task::spawn_blocking(move || {
        let sha256 = sha256_hex(&body);
        let verdict = check_wasm(&body, &limits).and_then(|()| {
            store
                .put(&sha256, &body)
                .map(|_| ())
                .map_err(|e| format!("storage: {e}"))
        });
        (sha256, verdict)
    })
    .await
    .map_err(ApiError::internal)?;
    if let Err(reason) = &verdict {
        if reason.starts_with("storage: ") {
            return Err(ApiError::internal(reason));
        }
    }
    let new = NewSubmission {
        team_id: &team.id,
        sha256: &sha256,
        size,
        language: &language,
        fuel_mult,
        rejected: verdict.as_ref().err().map(String::as_str),
        make_live: q.live.unwrap_or(true),
    };
    let now = now_ms();
    let (id, live) = app.db.tx(|tx| {
        let (id, change) = db::insert_submission(tx, &new, now)?;
        app.after_live_change(tx, &team.id, change, now)?;
        let live = db::live_submission(tx, &team.id)?.is_some_and(|s| s.id == id);
        Ok((id, live))
    })?;
    match verdict {
        Ok(()) => Ok((
            StatusCode::CREATED,
            Json(
                json!({"submission_id": id, "team_id": team.id, "sha256": sha256, "status": "validated", "live": live,
                        "language": language, "fuel_mult": fuel_mult}),
            ),
        )),
        Err(reason) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_wasm",
            format!("submission {id} rejected: {reason}"),
        )),
    }
}

/// Make one of the team's validated submissions the live one (team token, or the admin token). Only series
/// created afterwards use it; a replaced submission widens the team's rating uncertainty.
pub async fn select_live(
    State(app): AppState,
    Path(submission_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let caller = auth_team_or_admin(&app, &headers)?;
    let now = now_ms();
    let (team, change) = app.db.tx(|tx| {
        let team = match &caller {
            Caller::Team(t) => t.clone(),
            Caller::Admin => db::submission_team(tx, &submission_id)?,
        };
        let change = db::set_live(tx, &team.id, &submission_id)?;
        app.after_live_change(tx, &team.id, change, now)?;
        Ok((team, change))
    })?;
    Ok(Json(
        json!({"team_id": team.id, "live_submission": submission_id, "changed": change != db::LiveChange::Unchanged}),
    ))
}

pub async fn list_submissions(
    State(app): AppState,
    Query(q): Query<SubmissionQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let team = acting_team(&app, auth_team_or_admin(&app, &headers)?, q.team.as_deref())?;
    Ok(Json(
        json!({"submissions": app.db.tx(|tx| db::list_submissions(tx, &team.id))?}),
    ))
}

// ---- maps, patch ----

#[derive(Deserialize)]
pub struct MapQuery {
    in_pool: Option<bool>,
}

/// Body: the `.ibcmap` text. The coordinator is patch-agnostic and does not parse maps; a map the engine
/// rejects shows up as dead-lettered jobs, so check it with `ibc` before uploading.
pub async fn upload_map(
    State(app): AppState,
    Path(name): Path<String>,
    Query(q): Query<MapQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    if !valid_name(&name, 64) {
        return Err(ApiError::bad_request(
            "map name must be 1..=64 characters of [A-Za-z0-9_.-]",
        ));
    }
    if body.is_empty() || body.len() > app.cfg.max_map_bytes || std::str::from_utf8(&body).is_err()
    {
        return Err(ApiError::bad_request(format!(
            "a map is non-empty UTF-8 text of at most {} bytes",
            app.cfg.max_map_bytes
        )));
    }
    let sha256 = sha256_hex(&body);
    app.blobs.put(&sha256, &body)?;
    let in_pool = q.in_pool.unwrap_or(true);
    app.db
        .tx(|tx| db::upsert_map(tx, &name, &sha256, in_pool, now_ms()))?;
    Ok(Json(
        json!({"name": name, "sha256": sha256, "in_pool": in_pool}),
    ))
}

#[derive(Deserialize)]
struct SetPatchReq {
    #[serde(flatten)]
    patch: Patch,
    /// `"<os>-<arch>"` (e.g. `linux-x86_64`) -> SHA-256 of an uploaded `ibc` binary of this patch, sent with
    /// its jobs as `runner_blobs` (workers started with `--allow-runner-fetch` install it).
    runner_blobs: Option<BTreeMap<String, String>>,
}

pub async fn set_patch(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let SetPatchReq {
        patch,
        runner_blobs,
    } = parse(&body)?;
    let one_line = |s: &str| !s.is_empty() && s.len() <= 100 && !s.contains('\n');
    if !(one_line(&patch.engine) && one_line(&patch.ruleset_hash) && one_line(&patch.wasmtime)) {
        return Err(ApiError::bad_request(
            "engine, ruleset_hash and wasmtime must be non-empty single-line strings",
        ));
    }
    for (platform, sha) in runner_blobs.iter().flatten() {
        if !valid_name(platform, 64) || !is_sha256(sha) || !app.blobs.exists(sha) {
            return Err(ApiError::bad_request(format!(
                "runner_blobs.{platform}: expected the sha256 of an uploaded blob"
            )));
        }
    }
    let runner_blobs = runner_blobs.map(|m| json!(m));
    let now = now_ms();
    let superseded = app.db.tx(|tx| {
        let superseded = db::set_current_patch(tx, &patch, runner_blobs.as_ref(), now)?;
        finalize(&app, tx, now)?;
        Ok(superseded)
    })?;
    bump(&app.counters.dead, superseded);
    Ok(Json(
        json!({"patch": patch, "runner_blobs": runner_blobs, "superseded_jobs": superseded}),
    ))
}

// ---- series ----

#[derive(Deserialize)]
struct SeriesReq {
    /// Team token: the team to play against (id or name).
    opponent: Option<String>,
    /// Admin token: both teams.
    team_a: Option<String>,
    team_b: Option<String>,
    games: Option<i64>,
    /// Admin token only: `unranked` (default) or `tournament`.
    kind: Option<String>,
}

fn check_games(app: &App, games: i64) -> ApiResult<i64> {
    if (1..=app.cfg.max_series_games).contains(&games) {
        Ok(games)
    } else {
        Err(ApiError::bad_request(format!(
            "games must be between 1 and {}",
            app.cfg.max_series_games
        )))
    }
}

/// An unranked series between the live submissions of two teams. A team may request
/// `unranked_daily_games` games per rolling 24 hours; the organisers are not limited.
pub async fn create_series(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Created> {
    let caller = auth_team_or_admin(&app, &headers)?;
    let req: SeriesReq = parse(&body)?;
    let games = check_games(&app, req.games.unwrap_or(1))?;
    let now = now_ms();
    let view = app.db.tx(|tx| {
        let (kind, a, b, requested_by) = match &caller {
            Caller::Team(me) => {
                if req.kind.as_deref().is_some_and(|k| k != "unranked") {
                    return Err(ApiError::bad_request("teams can only request unranked series"));
                }
                let opponent = req.opponent.as_deref().ok_or_else(|| ApiError::bad_request("missing `opponent`"))?;
                let used = league::games_requested_since(tx, &me.id, now - 86_400_000)?;
                if used + games > app.cfg.unranked_daily_games {
                    return Err(ApiError::new(
                        StatusCode::TOO_MANY_REQUESTS,
                        "quota_exceeded",
                        format!("{used} of {} unranked games used in the last 24 h; {games} more do not fit", app.cfg.unranked_daily_games),
                    ));
                }
                ("unranked", me.clone(), db::find_team(tx, opponent)?, Some(me.id.clone()))
            }
            Caller::Admin => {
                let kind = match req.kind.as_deref().unwrap_or("unranked") {
                    "unranked" => "unranked",
                    "tournament" => "tournament",
                    other => return Err(ApiError::bad_request(format!("kind {other:?}: use `unranked` or `tournament` (ranked series go through /v1/admin/ranked-batch)"))),
                };
                let team = |t: &Option<String>, what: &str| -> ApiResult<TeamRow> {
                    db::find_team(tx, t.as_deref().ok_or_else(|| ApiError::bad_request(format!("missing `{what}`")))?)
                };
                (kind, team(&req.team_a, "team_a")?, team(&req.team_b, "team_b")?, None)
            }
        };
        let id = app.new_series(tx, kind, &a, &b, games, requested_by.as_deref(), now)?;
        league::series_view(tx, &id)
    })?;
    app.queue_changed.notify_waiters();
    Ok((StatusCode::CREATED, Json(view.unwrap_or(Value::Null))))
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Pairing {
    Pair([String; 2]),
    Named {
        a: String,
        b: String,
        games: Option<i64>,
    },
}

#[derive(Deserialize)]
struct RankedBatchReq {
    /// `[["a", "b"], {"a": .., "b": .., "games": n}, ...]` (team ids or names). Missing: let the
    /// matchmaker pair the teams, as the periodic scheduler does.
    pairings: Option<Vec<Pairing>>,
    /// Games per series when a pairing does not say; default `ranked_games`.
    games: Option<i64>,
    /// With automatic pairing: most series to create; default `ranked_series_budget`.
    budget: Option<u32>,
}

pub async fn ranked_batch(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let req: RankedBatchReq = parse(&body)?;
    let default_games = check_games(&app, req.games.unwrap_or(app.cfg.ranked_games))?;
    let now = now_ms();
    let series = match req.pairings {
        None => app.ranked_round(
            Some(req.budget.unwrap_or_else(|| {
                u32::try_from(app.cfg.ranked_series_budget).unwrap_or(u32::MAX)
            })),
            default_games,
        )?,
        Some(pairings) => {
            let created = app.db.tx(|tx| {
                let mut created = Vec::new();
                for pairing in &pairings {
                    let (a, b, games) = match pairing {
                        Pairing::Pair([a, b]) => (a, b, default_games),
                        Pairing::Named { a, b, games } => {
                            (a, b, check_games(&app, games.unwrap_or(default_games))?)
                        }
                    };
                    let (a, b) = (db::find_team(tx, a)?, db::find_team(tx, b)?);
                    created.push(app.new_series(tx, "ranked", &a, &b, games, None, now)?);
                }
                Ok(created)
            })?;
            app.queue_changed.notify_waiters();
            created
        }
    };
    Ok(Json(json!({"series": series})))
}

// ---- jobs ----

#[derive(Deserialize)]
struct EnqueueReq {
    /// `tournament`, `ranked`, `unranked` or `bench` (default).
    class: Option<String>,
    /// Default: the current patch.
    patch: Option<Patch>,
    bot_a: BotRef,
    bot_b: BotRef,
    map: MapRef,
    seed: u64,
    max_ticks: Option<u32>,
}

/// Queue one match outside any series (benchmarks, re-runs). If a job with the same idempotency key exists
/// it is returned instead, with its result if it is done (`"cached": true`): no work is created.
pub async fn enqueue_job(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let req: EnqueueReq = parse(&body)?;
    let class = match req.class.as_deref() {
        None => Class::Bench,
        Some(name) => Class::parse(name)
            .ok_or_else(|| ApiError::bad_request(format!("unknown class {name:?}")))?,
    };
    for (what, sha) in [
        ("bot_a", &req.bot_a.sha256),
        ("bot_b", &req.bot_b.sha256),
        ("map", &req.map.sha256),
    ] {
        if !app.blobs.exists(sha) {
            return Err(ApiError::bad_request(format!(
                "{what}: no blob {sha} (upload it first)"
            )));
        }
    }
    let now = now_ms();
    let queued = app.db.tx(|tx| {
        let patch = match &req.patch {
            Some(p) => p.clone(),
            None => db::current_patch(tx)?
                .ok_or_else(|| ApiError::conflict("no_patch", "no current patch is set"))?,
        };
        let spec = JobSpec {
            patch,
            bot_a: req.bot_a.clone(),
            bot_b: req.bot_b.clone(),
            map: req.map.clone(),
            seed: req.seed,
            max_ticks: req.max_ticks,
        }
        .normalize()
        .map_err(ApiError::bad_request)?;
        jobs::enqueue(tx, class, &spec, now)
    })?;
    let cached = queued.result.is_some();
    bump(&app.counters.cache_hits, usize::from(cached));
    if queued.created {
        app.queue_changed.notify_waiters();
    }
    Ok(Json(
        json!({"job_id": queued.job_id, "state": queued.state, "created": queued.created, "cached": cached, "result": queued.result}),
    ))
}

pub async fn get_job(
    State(app): AppState,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    app.db
        .tx(|tx| jobs::job_view(tx, &job_id))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no job {job_id:?}")))
}

/// The dead-letter list.
pub async fn dead_jobs(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    Ok(Json(
        json!({"jobs": app.db.tx(|tx| jobs::dead_jobs(tx, 500))?}),
    ))
}

pub async fn requeue_job(
    State(app): AppState,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    app.db.tx(|tx| jobs::requeue_dead(tx, &job_id, now_ms()))?;
    app.queue_changed.notify_waiters();
    Ok(Json(json!({})))
}

// ---- workers ----

pub async fn list_workers(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    Ok(Json(
        json!({"workers": app.db.tx(|tx| db::list_workers(tx, now_ms(), app.cfg.worker_live_ms))?}),
    ))
}

#[derive(Deserialize)]
struct TrustReq {
    trusted: bool,
}

pub async fn trust_worker(
    State(app): AppState,
    Path(worker_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let req: TrustReq = parse(&body)?;
    app.db
        .tx(|tx| db::set_worker_trusted(tx, &worker_id, req.trusted))?;
    app.queue_changed.notify_waiters();
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct QuarantineReq {
    quarantined: bool,
    reason: Option<String>,
}

/// Quarantining by hand does what a verification mismatch does: no more jobs, leases revoked, unverified
/// results requeued, closed series that counted them reopened and the ratings recomputed without them.
/// Lifting it only lets the worker lease again.
pub async fn quarantine_worker(
    State(app): AppState,
    Path(worker_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let req: QuarantineReq = parse(&body)?;
    let now = now_ms();
    let q = app.db.tx(|tx| {
        if req.quarantined {
            let reason = req
                .reason
                .as_deref()
                .unwrap_or("quarantined by an organiser");
            let q = jobs::quarantine(tx, &worker_id, reason, now)?;
            app.after_quarantine(tx, &q, now)?;
            Ok(q)
        } else {
            db::release_worker(tx, &worker_id).map(|()| jobs::Quarantined::default())
        }
    })?;
    app.queue_changed.notify_waiters();
    Ok(Json(
        json!({"requeued_results": q.requeued, "reopened_series": q.reopened}),
    ))
}
