//! Teams of users: create, join by code, leave, rotate the join code, regenerate the team token
//! (docs/api/auth.md). A user is in at most one team; a team has at most `IBC_TEAM_MAX` members.

use super::auth::auth_user;
use super::manage::{acting_team, insert_team};
use super::{auth_team_or_admin, parse, AppState};
use crate::accounts::MAX_TEAMS_CREATED;
use crate::db::users;
use crate::db::TeamRow;
use crate::error::{ApiError, ApiResult};
use crate::util::{new_token, now_ms, sha256_hex};
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// What a member sees of their team.
pub(crate) fn team_view(tx: &rusqlite::Transaction, team: &TeamRow) -> ApiResult<Value> {
    Ok(json!({
        "id": team.id,
        "name": team.name,
        "join_code": users::join_code(tx, &team.id)?,
        "members": users::members(tx, &team.id)?,
    }))
}

fn already_in_team(tx: &rusqlite::Transaction, user_id: &str) -> ApiResult<()> {
    match users::team_of_user(tx, user_id)? {
        Some(team) => Err(ApiError::conflict(
            "already_in_team",
            format!("you are already in team {:?}; leave it first", team.name),
        )),
        None => Ok(()),
    }
}

#[derive(Deserialize)]
struct CreateReq {
    name: String,
}

/// Create a team with the caller as its first member. The team token is not shown (`POST /v1/team/token`).
pub async fn create(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let user = auth_user(&app, &headers)?;
    let req: CreateReq = parse(&body)?;
    let now = now_ms();
    let view = app.db.tx(|tx| {
        already_in_team(tx, &user.id)?;
        // Every team has its own unranked quota, so a user cannot mint teams at will.
        if users::note_team_created(tx, &user.id)? >= MAX_TEAMS_CREATED {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "team_limit",
                format!(
                    "an account can create at most {MAX_TEAMS_CREATED} teams; join one instead"
                ),
            ));
        }
        let (team, _token) = insert_team(tx, &req.name, now)?;
        users::add_member(tx, &team.id, &user.id, "owner", now)?;
        users::rotate_join_code(tx, &team.id)?;
        team_view(tx, &team)
    })?;
    let join_code = view["join_code"].clone();
    Ok((
        StatusCode::CREATED,
        Json(json!({"team": view, "join_code": join_code})),
    ))
}

#[derive(Deserialize)]
struct JoinReq {
    join_code: String,
}

pub async fn join(State(app): AppState, headers: HeaderMap, body: Bytes) -> ApiResult<Json<Value>> {
    let user = auth_user(&app, &headers)?;
    let req: JoinReq = parse(&body)?;
    let code = req.join_code.trim().to_ascii_uppercase();
    let max = app.cfg.auth.team_max;
    let now = now_ms();
    let view = app.db.tx(|tx| {
        already_in_team(tx, &user.id)?;
        let team = users::team_by_join_code(tx, &code)?
            .ok_or_else(|| ApiError::not_found("no team has this join code"))?;
        if users::member_count(tx, &team.id)? >= max {
            return Err(ApiError::conflict(
                "team_full",
                format!("team {:?} already has {max} members", team.name),
            ));
        }
        users::add_member(tx, &team.id, &user.id, "member", now)?;
        team_view(tx, &team)
    })?;
    Ok(Json(json!({"team": view})))
}

/// Leave the team. The team itself, its submissions and its rating stay, even with no members left. The
/// team token is replaced, so a copy the leaver took no longer works (members regenerate it).
pub async fn leave(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let user = auth_user(&app, &headers)?;
    app.db.tx(|tx| {
        let team = users::team_of_user(tx, &user.id)?
            .ok_or_else(|| ApiError::conflict("no_team", "you are not in a team"))?;
        users::remove_member(tx, &user.id)?;
        let unused = new_token("ibt");
        users::set_team_token(tx, &team.id, &sha256_hex(unused.as_bytes()))
    })?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct TeamQuery {
    /// Admin only: the team to act on (id or name).
    team: Option<String>,
}

/// New join code; the old one stops working. Any member, the team token, or an admin with `?team=`.
pub async fn rotate_join_code(
    State(app): AppState,
    Query(q): Query<TeamQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let team = acting_team(&app, auth_team_or_admin(&app, &headers)?, q.team.as_deref())?;
    let code = app.db.tx(|tx| users::rotate_join_code(tx, &team.id))?;
    Ok(Json(json!({"team_id": team.id, "join_code": code})))
}

/// New `ibt` team token for automation, shown once; the old one stops working.
pub async fn regenerate_token(
    State(app): AppState,
    Query(q): Query<TeamQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let team = acting_team(&app, auth_team_or_admin(&app, &headers)?, q.team.as_deref())?;
    let token = new_token("ibt");
    app.db
        .tx(|tx| users::set_team_token(tx, &team.id, &sha256_hex(token.as_bytes())))?;
    Ok(Json(
        json!({"team_id": team.id, "name": team.name, "token": token}),
    ))
}
