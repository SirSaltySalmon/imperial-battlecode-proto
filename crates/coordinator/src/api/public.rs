//! Read-only endpoints that need no token: ladder, matches, replays, maps, metrics, health. A token, when
//! sent, only adds the caller's own submissions to the match and series views (`league::Viewer`).

use super::worker::blob_response;
use super::{auth_team_or_admin, AppState, Caller};
use crate::blobs::BlobStore;
use crate::db::{self, jobs, league, ratings, scrims};
use crate::error::{ApiError, ApiResult};
use crate::model::CLASSES;
use crate::util::now_ms;
use crate::App;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ibc_replay::{Keep, Replay};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

/// Who is asking: a valid team or admin credential widens the view; none, or one that is not accepted
/// (a stale session on a public page), is the public view. Other failures are errors.
fn viewer(app: &App, headers: &HeaderMap) -> ApiResult<league::Viewer> {
    match auth_team_or_admin(app, headers) {
        Ok(Caller::Admin) => Ok(league::Viewer::Admin),
        Ok(Caller::Team(team)) => Ok(league::Viewer::Team(team.id)),
        Err(e) if e.status == StatusCode::UNAUTHORIZED => Ok(league::Viewer::Public),
        Err(e) => Err(e),
    }
}

/// `[{rank, team_id, team_name, rating|null, tier|null, provisional, games, series, wins, losses, draws}]`,
/// computed from the stored ratings on every request (one row per team with a live submission).
pub async fn ladder(State(app): AppState) -> ApiResult<Json<Value>> {
    let rows = app
        .db
        .tx(|tx| ratings::public_ladder(tx, app.rating.as_ref(), &app.cfg))?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
pub struct MatchesQuery {
    /// Team id or name.
    team: Option<String>,
    series: Option<String>,
    /// `ranked`, `unranked` or `tournament`.
    kind: Option<String>,
    /// Page backwards: the `next` value of the previous page (a match id).
    before: Option<String>,
    limit: Option<i64>,
}

pub async fn matches(
    State(app): AppState,
    Query(q): Query<MatchesQuery>,
) -> ApiResult<Json<Value>> {
    if let Some(kind) = q.kind.as_deref() {
        league::class_of_kind(kind).ok_or_else(|| {
            ApiError::bad_request(format!("kind {kind:?}: use ranked, unranked or tournament"))
        })?;
    }
    let list = app.db.tx(|tx| {
        let team = match &q.team {
            Some(t) => Some(db::find_team(tx, t)?.id),
            None => None,
        };
        let filter = league::MatchFilter {
            team,
            series: q.series.clone(),
            kind: q.kind.clone(),
            before: q.before.clone(),
            limit: q.limit.unwrap_or(50).clamp(1, 500),
        };
        league::list_matches(tx, &filter)
    })?;
    // A full page may have more behind it: `next` is the `before` of the following request.
    let limit = q.limit.unwrap_or(50).clamp(1, 500) as usize;
    let next = if list.len() == limit {
        list.last().map(|m| m["match_id"].clone())
    } else {
        None
    };
    Ok(Json(json!({"matches": list, "next": next})))
}

pub async fn get_match(
    State(app): AppState,
    Path(match_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let viewer = viewer(&app, &headers)?;
    app.db
        .tx(|tx| league::get_match(tx, &match_id, &viewer))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no match {match_id:?}")))
}

#[derive(Deserialize)]
pub struct ReplayQuery {
    /// `public` (default), `team` or `full`.
    view: Option<String>,
}

fn forbidden(message: &str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "forbidden", message)
}

/// Whose private data a view keeps, after checking the caller may see it. `None` means the stored record.
fn view_keep(
    app: &App,
    headers: &HeaderMap,
    view: &str,
    sides: &[String; 2],
) -> ApiResult<Option<Keep>> {
    if view == "public" {
        return Ok(Some(Keep::None));
    }
    match (view, auth_team_or_admin(app, headers)?) {
        ("full", Caller::Admin) => Ok(None),
        ("full", Caller::Team(_)) => Err(forbidden("view=full needs the admin token")),
        (_, Caller::Admin) => Err(forbidden("the admin token reads view=full")),
        (_, Caller::Team(team)) => match (sides[0] == team.id, sides[1] == team.id) {
            (true, true) => Ok(Some(Keep::Both)),
            (true, false) => Ok(Some(Keep::Team(0))),
            (false, true) => Ok(Some(Keep::Team(1))),
            (false, false) => Err(forbidden("your team did not play this match")),
        },
    }
}

/// Redacted views made recently, newest last: a popular match is decoded and re-encoded once, not per request.
/// Keyed by the team names too, so a renamed team is never served under its old name.
type ViewKey = (String, Keep, [String; 2]);
type ViewCache = Mutex<Vec<(ViewKey, Arc<Vec<u8>>)>>;
const VIEW_CACHE_ENTRIES: usize = 32;

fn view_cache() -> &'static ViewCache {
    static CACHE: OnceLock<ViewCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// The stored replay `sha256` with only the private data of `keep`, its sides named after `team_names`
/// (workers record a module hash prefix there, see docs/JOBS.md).
fn redacted(
    store: &dyn BlobStore,
    sha256: &str,
    keep: Keep,
    team_names: [String; 2],
) -> ApiResult<Arc<Vec<u8>>> {
    let key = (sha256.to_string(), keep, team_names);
    let lock = || view_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, bytes)) = lock().iter().find(|(k, _)| *k == key) {
        return Ok(bytes.clone());
    }
    let full = store
        .get(sha256)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("no blob {sha256}")))?;
    // Replays recorded before format 2 cannot be redacted; only the admin's view=full serves them.
    let replay = Replay::decode(&full).map_err(|e| {
        ApiError::new(
            StatusCode::GONE,
            "replay_unredactable",
            format!("this replay cannot be redacted ({e}); only view=full serves it"),
        )
    })?;
    let mut view = replay.view(keep);
    for (bot, name) in view.header.bots.iter_mut().zip(&key.2) {
        bot.name.clone_from(name);
    }
    let bytes = Arc::new(view.encode());
    let mut cache = lock();
    if cache.len() >= VIEW_CACHE_ENTRIES {
        cache.remove(0);
    }
    cache.push((key, bytes.clone()));
    Ok(bytes)
}

/// The `.ibcr` replay of a finished match. Every view has every action and event and re-simulates to the same
/// world; they differ in whose memory writes, logs, per-turn fuel and debug drawings they keep:
/// `view=public` (default, no token) neither team's, `view=team` (team token) the caller's team's,
/// `view=full` (admin token) both (the stored record). Redacted views are made on request and name each side
/// by its team; the stored record keeps the names the runner recorded.
pub async fn replay(
    State(app): AppState,
    Path(match_id): Path<String>,
    Query(q): Query<ReplayQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let view = q.view.as_deref().unwrap_or("public");
    if !matches!(view, "public" | "team" | "full") {
        return Err(ApiError::bad_request(format!(
            "view {view:?}: use public, team or full"
        )));
    }
    let found = app
        .db
        .tx(|tx| league::match_replay(tx, &match_id))?
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "match {match_id:?} has no replay (unknown or not finished)"
            ))
        })?;
    let Some(keep) = view_keep(&app, &headers, view, &found.team_ids)? else {
        return blob_response(app, found.sha256).await;
    };
    let store = app.blobs.clone();
    let bytes = tokio::task::spawn_blocking(move || {
        redacted(store.as_ref(), &found.sha256, keep, found.team_names)
    })
    .await
    .map_err(ApiError::internal)??;
    let cache = if keep == Keep::None {
        "public, max-age=3600"
    } else {
        "private, no-store"
    };
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, cache),
        ],
        bytes.as_ref().clone(),
    )
        .into_response())
}

pub async fn get_series(
    State(app): AppState,
    Path(series_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let viewer = viewer(&app, &headers)?;
    app.db
        .tx(|tx| league::series_view(tx, &series_id, &viewer))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no series {series_id:?}")))
}

/// Maps of a tournament whose maps are still hidden are left out.
pub async fn maps(State(app): AppState) -> ApiResult<Json<Value>> {
    let mut maps = app.db.tx(db::list_maps)?;
    let hidden = app.db.tx(db::tournaments::hidden_maps)?;
    maps.retain(|m| m["name"].as_str().is_none_or(|n| !hidden.contains(n)));
    Ok(Json(json!({"maps": maps})))
}

/// The `.ibcmap` text of a pool map (`ibc maps pull`).
pub async fn map_text(State(app): AppState, Path(name): Path<String>) -> ApiResult<Response> {
    let (_, sha256) = app.db.tx(|tx| scrims::pool_map(tx, &name))?;
    let mut res = blob_response(app, sha256).await?;
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok(res)
}

pub async fn patch(State(app): AppState) -> ApiResult<Json<Value>> {
    Ok(Json(app.db.tx(|tx| {
        crate::patch::public_status(&app, tx, now_ms())
    })?))
}

pub async fn healthz(State(app): AppState) -> ApiResult<&'static str> {
    app.db.tx(db::current_patch)?;
    Ok("ok\n")
}

/// Prometheus text format. `ibc_queue_depth` and `ibc_queue_oldest_age_seconds` against
/// `ibc_workers_cores` are what an autoscaler needs.
pub async fn metrics(State(app): AppState) -> ApiResult<Response> {
    let s = app
        .db
        .tx(|tx| jobs::stats(tx, now_ms(), app.cfg.worker_live_ms))?;
    let mut out = String::new();
    let mut metric = |name: &str, kind: &str, help: &str, samples: &[(String, f64)]| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (labels, value) in samples {
            let _ = writeln!(out, "{name}{labels} {value}");
        }
    };
    let per_class = |f: &dyn Fn(usize) -> f64| -> Vec<(String, f64)> {
        CLASSES
            .iter()
            .map(|c| (format!("{{class=\"{}\"}}", c.name()), f(*c as usize)))
            .collect()
    };
    let one = |v: f64| vec![(String::new(), v)];
    let counter = |c: &std::sync::atomic::AtomicU64| one(c.load(Ordering::Relaxed) as f64);
    let c = &app.counters;
    metric(
        "ibc_queue_depth",
        "gauge",
        "Queued jobs per priority class.",
        &per_class(&|i| s.queued[i].0 as f64),
    );
    metric(
        "ibc_queue_oldest_age_seconds",
        "gauge",
        "Age of the oldest queued job per class.",
        &per_class(&|i| s.queued[i].1 as f64 / 1000.0),
    );
    metric(
        "ibc_jobs_leased",
        "gauge",
        "Jobs currently leased to workers.",
        &one(s.leased as f64),
    );
    metric(
        "ibc_jobs_dead",
        "gauge",
        "Jobs on the dead-letter list.",
        &one(s.dead as f64),
    );
    metric(
        "ibc_verify_pending",
        "gauge",
        "Spot verifications waiting for or running on a trusted worker.",
        &one(s.verify_pending as f64),
    );
    metric(
        "ibc_jobs_per_minute",
        "gauge",
        "Jobs completed in the last 60 seconds.",
        &one(s.done_last_minute as f64),
    );
    metric(
        "ibc_workers_live",
        "gauge",
        "Workers heard from recently, not quarantined.",
        &one(s.live_workers as f64),
    );
    metric(
        "ibc_workers_cores",
        "gauge",
        "Cores of the live workers.",
        &one(s.live_cores as f64),
    );
    metric(
        "ibc_workers_quarantined",
        "gauge",
        "Quarantined workers.",
        &one(s.quarantined_workers as f64),
    );
    metric(
        "ibc_cache_hits_total",
        "counter",
        "Enqueues answered from an existing result.",
        &counter(&c.cache_hits),
    );
    metric(
        "ibc_jobs_completed_total",
        "counter",
        "Results accepted.",
        &counter(&c.completed),
    );
    metric(
        "ibc_jobs_failed_total",
        "counter",
        "Failures reported by workers.",
        &counter(&c.failed),
    );
    metric(
        "ibc_jobs_dead_total",
        "counter",
        "Jobs moved to the dead-letter list.",
        &counter(&c.dead),
    );
    metric(
        "ibc_leases_expired_total",
        "counter",
        "Leases that ran out without a result.",
        &counter(&c.leases_expired),
    );
    metric(
        "ibc_verify_confirmed_total",
        "counter",
        "Spot verifications that agreed.",
        &counter(&c.verify_confirmed),
    );
    metric(
        "ibc_verify_mismatch_total",
        "counter",
        "Spot verifications that disagreed (worker quarantined).",
        &counter(&c.verify_mismatch),
    );
    metric(
        "ibc_verify_conflict_total",
        "counter",
        "Two trusted workers disagreed (determinism bug).",
        &counter(&c.verify_conflict),
    );
    Ok(([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out).into_response())
}
