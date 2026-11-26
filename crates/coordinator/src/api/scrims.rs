//! `POST /v1/scrims`: an unranked series a team asks for.
//!
//! The requester may field any of its own validated submissions. The opponent is always its current live
//! bot, except when the request names a past series both teams played: then the opponent plays the
//! submission it fielded in that series. The server resolves it and never says which submission it is, so
//! the archive stays private (nobody can list or count another team's versions).

use super::{auth_team_or_admin, parse, AppState, Caller};
use crate::db::league::{self, SeriesSpec, Sides, Viewer};
use crate::db::{self, scrims};
use crate::error::{ApiError, ApiResult};
use crate::util::now_ms;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct ScrimReq {
    /// Team id or name.
    opponent_team_id: String,
    /// Maps to play, each a full match; default 1.
    games: Option<i64>,
    /// Pool maps by name, played in turn (cycling if shorter than `games`). Default: the pool, least-used
    /// maps first.
    maps: Option<Vec<String>>,
    /// Engine slot of the requester: `"a"`, `"b"` or `"alternate"` (default).
    sides: Option<String>,
    /// One of the requester's own validated submissions. Default: its live one.
    my_submission_id: Option<String>,
    /// A series both teams played: the opponent fields the submission it played there.
    against_series_id: Option<String>,
}

fn parse_sides(sides: Option<&str>) -> ApiResult<Sides> {
    match sides.unwrap_or("alternate") {
        "alternate" => Ok(Sides::Alternate),
        "a" => Ok(Sides::A),
        "b" => Ok(Sides::B),
        other => Err(ApiError::bad_request(format!(
            "sides {other:?}: use \"a\", \"b\" or \"alternate\""
        ))),
    }
}

/// Body: `ScrimReq`. Counts `games` against the team's unranked quota (429 `quota_exceeded` with
/// `retry_after` when it does not fit). Returns the series as the requester sees it.
pub async fn create(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let me = match auth_team_or_admin(&app, &headers)? {
        Caller::Team(team) => team,
        Caller::Admin => {
            return Err(ApiError::bad_request(
                "scrims are requested by a team; organisers create series with POST /v1/series",
            ))
        }
    };
    let req: ScrimReq = parse(&body)?;
    let games = req.games.unwrap_or(1);
    if !(1..=app.cfg.max_scrim_games).contains(&games) {
        return Err(ApiError::bad_request(format!(
            "games must be between 1 and {}",
            app.cfg.max_scrim_games
        )));
    }
    let sides = parse_sides(req.sides.as_deref())?;
    if req.maps.as_ref().is_some_and(|m| m.is_empty()) {
        return Err(ApiError::bad_request(
            "maps, when given, names at least one map",
        ));
    }
    let now = now_ms();
    let view = app.db.tx(|tx| {
        let maps = match &req.maps {
            Some(names) => Some(
                names
                    .iter()
                    .map(|name| scrims::pool_map(tx, name))
                    .collect::<ApiResult<Vec<_>>>()?,
            ),
            None => None,
        };
        let opponent = db::find_team(tx, &req.opponent_team_id)?;
        if opponent.id == me.id {
            return Err(ApiError::bad_request("a team cannot play itself"));
        }
        scrims::check_quota(tx, &me.id, app.cfg.unranked_daily_games, games, now)?;
        let sub_a = match &req.my_submission_id {
            Some(id) => scrims::own_submission(tx, &me.id, id)?,
            None => league::require_live(tx, &me)?,
        };
        let sub_b = match &req.against_series_id {
            Some(series) => scrims::past_opponent_submission(tx, series, &me.id, &opponent.id)?,
            None => league::require_live(tx, &opponent)?,
        };
        let spec = SeriesSpec {
            kind: "unranked",
            team_a: &me,
            team_b: &opponent,
            sub_a,
            sub_b,
            games,
            maps,
            sides,
            requested_by: Some(&me.id),
        };
        let id = app.new_series_with(tx, &spec, now)?;
        league::series_view(tx, &id, &Viewer::Team(me.id.clone()))
    })?;
    app.queue_changed.notify_waiters();
    Ok((StatusCode::CREATED, Json(view.unwrap_or(Value::Null))))
}
