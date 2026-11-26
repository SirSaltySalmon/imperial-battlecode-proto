//! Team and organiser endpoints: teams, submissions, maps, the patch, series, the job queue, workers.

use super::{auth_admin, auth_team_or_admin, parse, AppState, Caller};
use crate::db::{self, jobs, league, NewSubmission, TeamRow};
use crate::error::{ApiError, ApiResult};
use crate::model::{BotRef, Class, JobSpec, MapRef, Patch};
use crate::util::{is_sha256, new_token, now_ms, sha256_hex};
use crate::wasmcheck::Abi;
use crate::{bump, patch, App};
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
            let (live, status) = app.db.tx(|tx| {
                let status = crate::patch::team_status(&app, tx, &team.id, now_ms())?;
                Ok((db::live_submission(tx, &team.id)?, status))
            })?;
            let mut out = json!({"team_id": team.id, "name": team.name, "live_submission": live.map(|s| s.id)});
            if let (Some(out), Some(status)) = (out.as_object_mut(), status.as_object()) {
                out.extend(status.clone());
            }
            Ok(Json(out))
        }
    }
}

// ---- submissions ----

#[derive(Deserialize)]
pub struct SubmissionQuery {
    /// Upload only: one of the server's languages (`patch::resolve_language`).
    language: Option<String>,
    /// Admin only: the team to upload for (id or name).
    team: Option<String>,
    /// Upload only: make the module live if it validates (default true).
    live: Option<bool>,
    /// Refused: the multiplier is the server's (`Config::languages`).
    fuel_mult: Option<String>,
    /// Upload only: a note to remember the version by (`ibc submit --note`), at most `MAX_NOTE_CHARS`.
    note: Option<String>,
}

/// Longest submission note, in characters.
const MAX_NOTE_CHARS: usize = 200;

/// The upload's note, trimmed; an empty note is none.
fn submission_note(note: Option<&str>) -> ApiResult<Option<&str>> {
    let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    if note.chars().count() > MAX_NOTE_CHARS || note.chars().any(char::is_control) {
        return Err(ApiError::bad_request(format!(
            "note: at most {MAX_NOTE_CHARS} characters, no control characters"
        )));
    }
    Ok(Some(note))
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
/// `?live=false`. Series that already exist keep the submissions they were created with. The module is
/// checked against the current patch; its fuel multiplier comes from the server's language table.
pub async fn upload_submission(
    State(app): AppState,
    Query(q): Query<SubmissionQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Created> {
    let caller = auth_team_or_admin(&app, &headers)?;
    let by_team = matches!(caller, Caller::Team(_));
    let team = acting_team(&app, caller, q.team.as_deref())?;
    if by_team {
        super::moderation::check_upload(&app, &team)?;
    }
    if q.fuel_mult.is_some() {
        return Err(ApiError::bad_request(
            "the server sets the fuel multiplier from the language; send ?language= only",
        ));
    }
    let note = submission_note(q.note.as_deref())?;
    if let Some(language) = q.language.as_deref() {
        if app.cfg.fuel_multiplier(language).is_none() {
            return Err(patch::unknown_language(&app.cfg, language));
        }
    }
    if body.len() > app.cfg.max_wasm_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            format!(
                "the module is {} bytes; the limit is {} bytes",
                body.len(),
                app.cfg.max_wasm_bytes
            ),
        ));
    }
    let size = body.len();
    let patch::Inspected {
        sha256,
        verdict,
        detected,
        patch_id,
    } = patch::inspect_upload(&app, body).await?;
    // A module that does not validate is stored as rejected; what it says about its language is moot.
    let detected = if verdict.is_ok() {
        detected
    } else {
        Default::default()
    };
    let (language, fuel_mult) =
        patch::resolve_language(&app.cfg, q.language.as_deref(), &detected)?;
    let new = NewSubmission {
        team_id: &team.id,
        sha256: &sha256,
        size,
        language: &language,
        fuel_mult,
        rejected: verdict.as_ref().err().map(String::as_str),
        make_live: q.live.unwrap_or(true),
        note,
    };
    let now = now_ms();
    let (id, live) = app.db.tx(|tx| {
        if by_team {
            // Again under the write lock: parallel uploads all passed the check above.
            db::moderation::check_upload(tx, &team.id, app.cfg.moderation.uploads_per_hour, now)?;
        }
        patch::confirm_same_patch(tx, patch_id)?;
        let (id, change) = db::insert_submission(tx, &new, now)?;
        if let Some(patch_id) = patch_id {
            db::patches::record_check(tx, &id, patch_id, new.rejected, now)?;
        }
        app.after_live_change(tx, &team.id, change, now)?;
        let live = db::live_submission(tx, &team.id)?.is_some_and(|s| s.id == id);
        Ok((id, live))
    })?;
    super::moderation::retain(&app, &team.id);
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
/// created afterwards use it; a replaced submission widens the team's rating uncertainty. One that does not
/// validate on the current patch is refused (409 `invalid_on_patch`).
pub async fn select_live(
    State(app): AppState,
    Path(submission_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let caller = auth_team_or_admin(&app, &headers)?;
    let own_team = match &caller {
        Caller::Team(t) => Some(t.id.as_str()),
        Caller::Admin => None,
    };
    patch::require_valid(&app, &submission_id, own_team).await?;
    let now = now_ms();
    let (team, change) = app.db.tx(|tx| {
        let team = match &caller {
            Caller::Team(t) => t.clone(),
            Caller::Admin => db::submission_team(tx, &submission_id)?,
        };
        patch::confirm_valid(tx, &submission_id, &team.id)?;
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
    let list = app.db.tx(|tx| {
        let mut list = db::list_submissions(tx, &team.id)?;
        patch::annotate_submissions(tx, &team.id, &mut list)?;
        Ok(list)
    })?;
    Ok(Json(json!({"submissions": list})))
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
    /// The imports the patch offers bots (`wasmcheck::Abi`); default: this coordinator build's lists.
    imports: Option<Abi>,
    /// A balance patch: every rating's uncertainty is widened (only when the patch changes).
    #[serde(default)]
    balance: bool,
}

/// Switch the current patch (docs/JOBS.md, "Patch boundary"), then check every live submission on it.
pub async fn set_patch(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let SetPatchReq {
        patch,
        runner_blobs,
        imports,
        balance,
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
    let req = patch::SwitchReq {
        patch: &patch,
        runner_blobs: runner_blobs.as_ref(),
        abi: imports.as_ref(),
        balance,
    };
    let switched = app.db.tx(|tx| patch::switch(&app, tx, &req, now_ms()))?;
    app.queue_changed.notify_waiters();
    let checked = patch::revalidate(&app).await?;
    Ok(Json(json!({
        "patch": patch, "runner_blobs": runner_blobs, "switched": switched.switched,
        "balance": balance && switched.switched, "ratings_widened": switched.widened,
        "replanned_series": switched.replanned, "kept_series": switched.kept,
        "revalidated": checked.checked, "invalid_submissions": checked.failed,
    })))
}

// ---- series ----

#[derive(Deserialize)]
struct SeriesReq {
    team_a: String,
    team_b: String,
    games: Option<i64>,
    /// `unranked` (default) or `tournament`.
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

/// Organisers: a series between the live submissions of two teams, not counted against any quota. Teams
/// ask for unranked series with `POST /v1/scrims`.
pub async fn create_series(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Created> {
    if let Caller::Team(_) = auth_team_or_admin(&app, &headers)? {
        return Err(ApiError::bad_request(
            "teams request unranked series with POST /v1/scrims",
        ));
    }
    let req: SeriesReq = parse(&body)?;
    let games = check_games(&app, req.games.unwrap_or(1))?;
    let kind = match req.kind.as_deref().unwrap_or("unranked") {
        "unranked" => "unranked",
        "tournament" => "tournament",
        other => return Err(ApiError::bad_request(format!(
            "kind {other:?}: use `unranked` or `tournament` (ranked series go through /v1/admin/ranked-batch)"
        ))),
    };
    let now = now_ms();
    let view = app.db.tx(|tx| {
        let (a, b) = (
            db::find_team(tx, &req.team_a)?,
            db::find_team(tx, &req.team_b)?,
        );
        let id = app.new_series(tx, kind, &a, &b, games, None, now)?;
        league::series_view(tx, &id, &league::Viewer::Admin)
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
