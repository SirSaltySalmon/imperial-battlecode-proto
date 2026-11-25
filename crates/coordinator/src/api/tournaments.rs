//! Tournaments (docs/api/tournaments.md): organisers create, lock, start and reveal; anyone reads.

use super::{auth_admin, bearer, is_admin, parse, AppState};
use crate::db::tournaments::{self, Field, NewTournament};
use crate::error::{ApiError, ApiResult};
use crate::util::now_ms;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateReq {
    name: String,
    format: String,
    field: Option<Field>,
    maps: Vec<String>,
}

/// `POST /v1/admin/tournaments`: a draft. The field defaults to every team.
pub async fn create(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    auth_admin(&app, &headers)?;
    let req: CreateReq = parse(&body)?;
    let format = tournaments::parse_format(&req.format)
        .ok_or_else(|| ApiError::bad_request("format must be \"single\" or \"double\""))?;
    let new = NewTournament {
        name: req.name,
        format,
        field: req.field.unwrap_or(Field::All),
        maps: req.maps,
    };
    let now = now_ms();
    let view = app.db.tx(|tx| {
        let id = tournaments::create(tx, new, now)?;
        tournaments::view(tx, &id, true, now)
    })?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// `POST /v1/admin/tournaments/{id}/{lock|start|reveal|resume-ranked}`. Returns the admin view.
pub async fn action(
    State(app): AppState,
    Path((id, action)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let now = now_ms();
    let view = app.db.tx(|tx| {
        match action.as_str() {
            "lock" => tournaments::lock(
                tx,
                app.rating.as_ref(),
                &id,
                app.cfg.tournament_pause_ms,
                now,
            )?,
            "start" => tournaments::start(tx, &id, now)?,
            "reveal" => tournaments::reveal(tx, &id, now)?,
            // The pause is global; the id only says which tournament the organiser is looking at.
            "resume-ranked" => tournaments::resume_ranked(tx)?,
            other => {
                return Err(ApiError::not_found(format!(
                    "no tournament action {other:?} (lock, start, reveal, resume-ranked)"
                )))
            }
        }
        tournaments::view(tx, &id, true, now)
    })?;
    if action == "start" {
        app.queue_changed.notify_waiters();
    }
    Ok(Json(view))
}

/// `GET /v1/tournaments`.
pub async fn list(State(app): AppState) -> ApiResult<Json<Value>> {
    let now = now_ms();
    let (list, paused) = app
        .db
        .tx(|tx| Ok((tournaments::list(tx)?, tournaments::ranked_paused(tx, now)?)))?;
    Ok(Json(json!({
        "tournaments": list,
        "ranked_paused_until": paused.map(crate::util::rfc3339),
    })))
}

/// `GET /v1/tournaments/{id}`: maps are `null` until revealed, except for the admin token.
pub async fn get(
    State(app): AppState,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let admin = bearer(&headers).is_ok_and(|t| is_admin(&app, t));
    let now = now_ms();
    Ok(Json(
        app.db.tx(|tx| tournaments::view(tx, &id, admin, now))?,
    ))
}
