//! The moderation terminal's API (`docs/api/moderation.md`): team sanctions, the overview, computed flags,
//! the audit log; and `guard`, the middleware in front of every route that applies the rate limits, refuses
//! writes by suspended teams and records every admin write in the audit log.

use super::{auth_admin, bearer, is_admin, parse, AppState};
use crate::db::{self, moderation as m, users, TeamRow};
use crate::error::{ApiError, ApiResult};
use crate::ratelimit::{Kind, Limiter};
use crate::util::{now_ms, opt_rfc3339, rfc3339, sha256_hex};
use crate::App;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

// ---- sanctions ----

#[derive(Deserialize)]
struct SanctionReq {
    reason: Option<String>,
    /// Suspensions only: `7d`, `12h`, `30m`, `2026-12-01` or `2026-12-01T18:00:00Z` (UTC). Missing: until lifted.
    until: Option<String>,
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// `until` of a suspension as epoch ms: relative (`7d`, `12h`, `30m`) or absolute UTC.
pub(crate) fn parse_until(s: &str, now: i64) -> Result<i64, String> {
    let s = s.trim();
    let bad = || format!("until {s:?}: use 7d, 12h, 30m, 2026-12-01 or 2026-12-01T18:00:00Z");
    if let Some(unit) = s.chars().last().filter(|c| matches!(c, 'd' | 'h' | 'm')) {
        let n: i64 = s[..s.len() - 1].parse().map_err(|_| bad())?;
        let ms = match unit {
            'd' => 86_400_000,
            'h' => 3_600_000,
            _ => 60_000,
        };
        return n
            .checked_mul(ms)
            .filter(|d| *d > 0)
            .and_then(|d| now.checked_add(d))
            .ok_or_else(bad);
    }
    let (date, time) = s.split_once('T').unwrap_or((s, "00:00:00Z"));
    let num = |t: &str| t.parse::<i64>().map_err(|_| bad());
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.strip_suffix('Z').ok_or_else(bad)?.split(':').collect();
    if d.len() != 3 || t.len() != 3 {
        return Err(bad());
    }
    let (y, mo, day) = (num(d[0])?, num(d[1])?, num(d[2])?);
    let (h, mi, sec) = (num(t[0])?, num(t[1])?, num(t[2])?);
    if !(1970..=9999).contains(&y)
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&h)
        || !(0..60).contains(&mi)
        || !(0..60).contains(&sec)
    {
        return Err(bad());
    }
    let ms = ((days_from_civil(y, mo, day) * 24 + h) * 60 + mi) * 60_000 + sec * 1000;
    // Reject dates that do not exist (2026-02-30) by formatting the result back.
    if !rfc3339(ms).starts_with(&format!("{y:04}-{mo:02}-{day:02}")) {
        return Err(bad());
    }
    Ok(ms)
}

fn sanction(app: &App, team: &str, body: &[u8], kind: &str) -> ApiResult<Json<Value>> {
    let req: SanctionReq = parse(body)?;
    let reason = req.reason.as_deref().map(str::trim).unwrap_or_default();
    if reason.is_empty() || reason.len() > 500 {
        return Err(ApiError::bad_request(
            "`reason` is required (1..=500 characters); the team sees it",
        ));
    }
    let now = now_ms();
    let until = match (kind, req.until.as_deref()) {
        (_, None) => None,
        ("banned", Some(_)) => {
            return Err(ApiError::bad_request(
                "a ban lasts until it is lifted; suspend with `until` for a fixed time",
            ))
        }
        (_, Some(u)) => Some(parse_until(u, now).map_err(ApiError::bad_request)?),
    };
    if until.is_some_and(|u| u <= now) {
        return Err(ApiError::bad_request("`until` is in the past"));
    }
    let team = app.db.tx(|tx| {
        let team = db::find_team(tx, team)?;
        m::set_sanction(tx, &team.id, kind, reason, until, now)?;
        Ok(team)
    })?;
    Ok(Json(
        json!({"team_id": team.id, "name": team.name, "kind": kind, "reason": reason,
                   "until": opt_rfc3339(until)}),
    ))
}

/// Suspend a team (until `until`, or until lifted): no ranked pairing, scrims or tournament series, and its
/// uploads and other writes answer 403 `team_suspended`.
pub async fn suspend(
    State(app): AppState,
    Path(team_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    sanction(&app, &team_id, &body, "suspended")
}

/// Ban a team: a suspension without an end, until `unban`.
pub async fn ban(
    State(app): AppState,
    Path(team_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    sanction(&app, &team_id, &body, "banned")
}

/// Lift a suspension or ban. The body (`{reason}`) is optional and only goes into the audit log.
pub async fn unban(
    State(app): AppState,
    Path(team_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let (team, lifted) = app.db.tx(|tx| {
        let team = db::find_team(tx, &team_id)?;
        let lifted = m::lift_sanction(tx, &team.id)?;
        Ok((team, lifted))
    })?;
    Ok(Json(
        json!({"team_id": team.id, "name": team.name, "lifted": lifted}),
    ))
}

/// Ban a user account (by id or email): sign-in, sessions and tokens stop working. The team stays.
pub async fn ban_user(
    State(app): AppState,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let user = app
        .db
        .tx(|tx| users::set_banned(tx, &user, Some(now_ms())))?;
    Ok(Json(json!({"user": user.json(), "banned": true})))
}

/// Lift a user ban. The body (`{reason}`) is optional and only goes into the audit log.
pub async fn unban_user(
    State(app): AppState,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let user = app.db.tx(|tx| users::set_banned(tx, &user, None))?;
    Ok(Json(json!({"user": user.json(), "banned": false})))
}

pub async fn sanctions(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    Ok(Json(
        json!({"sanctions": app.db.tx(|tx| m::list_sanctions(tx, now_ms()))?}),
    ))
}

// ---- overview, flags, audit ----

pub async fn overview(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let mut out = app
        .db
        .tx(|tx| m::overview(tx, now_ms(), app.cfg.worker_live_ms))?;
    let c = &app.cfg.moderation;
    out["pause"]["ranked_scheduler"] = json!(if app.cfg.ranked_interval_ms == 0 {
        "off".to_string()
    } else {
        format!("every {} s", app.cfg.ranked_interval_ms / 1000)
    });
    out["limits"] = json!({"uploads_per_hour": c.uploads_per_hour, "api_per_minute": c.api_per_minute,
                           "public_per_minute": c.public_per_minute, "submissions_kept": c.submissions_kept,
                           "max_wasm_bytes": app.cfg.max_wasm_bytes});
    Ok(Json(out))
}

pub async fn flags(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let over = app.cfg.moderation.burst_flag_uploads;
    Ok(Json(
        json!({"flags": app.db.tx(|tx| m::flags(tx, over, now_ms()))?}),
    ))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    before: Option<i64>,
    limit: Option<i64>,
}

pub async fn audit(
    State(app): AppState,
    Query(q): Query<AuditQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth_admin(&app, &headers)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    Ok(Json(
        json!({"entries": app.db.tx(|tx| m::audit_log(tx, q.before, limit))?}),
    ))
}

// ---- hooks of the submission upload ----

/// Before an upload by a team (not the organisers): 403 if the team is suspended, 429 past the upload limit.
pub(crate) fn check_upload(app: &App, team: &TeamRow) -> ApiResult<()> {
    let now = now_ms();
    app.db.tx(|tx| {
        m::ensure_active(tx, team, now)?;
        m::check_upload(tx, &team.id, app.cfg.moderation.uploads_per_hour, now)
    })
}

/// After an upload: apply submission retention to the team and delete blobs nothing refers to any more.
/// Failures are logged; the upload itself already succeeded.
pub(crate) fn retain(app: &App, team_id: &str) {
    let kept = app.cfg.moderation.submissions_kept;
    match app
        .db
        .tx(|tx| m::prune_submissions(tx, team_id, kept, now_ms()))
    {
        // Known gap: another team uploading the identical module between its blob write and its row insert
        // could lose the blob here (a window of milliseconds); re-uploading the module restores it.
        Ok(orphans) => {
            for sha in orphans {
                if let Err(e) = app.blobs.delete(&sha) {
                    eprintln!("retention: deleting blob {sha}: {e}");
                }
            }
        }
        Err(e) => eprintln!("retention: {}", e.message),
    }
}

// ---- the guard middleware ----

/// State of `guard`: the app and the in-memory rate limiter.
#[derive(Clone)]
pub struct Guard {
    app: Arc<App>,
    limiter: Arc<Limiter>,
}

impl Guard {
    pub fn new(app: Arc<App>) -> Guard {
        Guard {
            app,
            limiter: Arc::new(Limiter::default()),
        }
    }
}

/// Authenticated worker protocol routes are exempt from rate limits (leasing is paced by long polls).
/// `POST /v1/workers/register` is not: its secret travels in the body, so it is limited per IP.
fn is_worker_route(path: &str) -> bool {
    path == "/healthz"
        || (path.starts_with("/v1/workers/") && path != "/v1/workers/register")
        || path.starts_with("/v1/jobs/")
        || path.starts_with("/v1/blobs/")
}

/// The team behind a credential: a team token's team, or the team of the user behind a user token
/// (`ibu_`) or a session (`ibs_`, bearer or cookie). A user without a team, or an unknown credential,
/// counts against the client IP.
fn credential_team(app: &App, credential: &str, hash: &str) -> ApiResult<Option<TeamRow>> {
    let now = now_ms();
    app.db.tx(|tx| {
        let user = if credential.starts_with(super::auth::USER_TOKEN_PREFIX) {
            users::user_by_token(tx, hash, now)?
        } else if credential.starts_with(super::auth::SESSION_TOKEN_PREFIX) {
            users::user_by_session(tx, hash, now)?
        } else {
            return db::team_by_token(tx, hash);
        };
        match user {
            Some(user) => users::team_of_user(tx, &user.id),
            None => Ok(None),
        }
    })
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix("ibc_session="))
        .filter(|v| !v.is_empty())
}

fn client_ip(req: &Request, trust_forwarded_for: bool) -> Option<String> {
    if trust_forwarded_for {
        let forwarded = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty());
        if let Some(ip) = forwarded {
            return Some(ip.to_string());
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
}

fn limited(what: &str, per_minute: u32, retry_after: u64) -> Response {
    ApiError::rate_limited(
        "rate_limited",
        format!(
            "rate limit: {per_minute} requests per minute per {what}; retry after {retry_after} s"
        ),
        retry_after,
    )
    .into_response()
}

fn is_write(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// A clear 413 for a bot module over the size limit, before its body is read.
fn too_large_upload(app: &App, req: &Request) -> Option<Response> {
    if req.method() != Method::POST || req.uri().path() != "/v1/submissions" {
        return None;
    }
    let len: usize = req
        .headers()
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    let max = app.cfg.max_wasm_bytes;
    (len > max).then(|| {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            format!("the module is {len} bytes; the limit is {max} bytes ({:.1} MiB). Build with --release", max as f64 / 1_048_576.0),
        )
        .into_response()
    })
}

/// Rate limits, the suspension check on team writes, and the audit log of admin writes, in front of every
/// route. Worker routes and admin requests are never rate limited.
pub async fn guard(State(g): State<Guard>, req: Request, next: Next) -> Response {
    let credential = bearer(req.headers())
        .ok()
        .or_else(|| session_cookie(req.headers()))
        .map(str::to_string);
    if credential.as_deref().is_some_and(|c| is_admin(&g.app, c)) {
        return audited(&g.app, req, next).await;
    }
    if is_worker_route(req.uri().path()) {
        return next.run(req).await;
    }
    if let Some(res) = too_large_upload(&g.app, &req) {
        return res;
    }
    let cfg = &g.app.cfg.moderation;
    let now = now_ms();
    let team = match &credential {
        Some(c) => {
            let hash = sha256_hex(c.as_bytes());
            match credential_team(&g.app, c, &hash) {
                Ok(team) => team.map(|t| (hash, t)),
                Err(e) => return e.into_response(),
            }
        }
        None => None,
    };
    let Some((hash, team)) = team else {
        // Anonymous, or a credential that is not a known team token: limited per client IP.
        if let Some(ip) = client_ip(&req, cfg.trust_forwarded_for) {
            if let Err(secs) = g
                .limiter
                .take(Kind::Public, &ip, cfg.public_per_minute, now)
            {
                return limited("client IP", cfg.public_per_minute, secs);
            }
        }
        return next.run(req).await;
    };
    if let Err(secs) = g
        .limiter
        .take(Kind::Credential, &hash, cfg.api_per_minute, now)
    {
        return limited("token", cfg.api_per_minute, secs);
    }
    if is_write(req.method()) {
        if let Err(e) = g.app.db.tx(|tx| m::ensure_active(tx, &team, now)) {
            return e.into_response();
        }
    }
    next.run(req).await
}

/// Run an admin request; a write is recorded in the audit log with its body and the response status.
async fn audited(app: &Arc<App>, req: Request, next: Next) -> Response {
    if !is_write(req.method()) {
        return next.run(req).await;
    }
    let action = format!(
        "{} {}",
        req.method(),
        req.uri().path_and_query().map_or("", |p| p.as_str())
    );
    let (parts, body) = req.into_parts();
    let limit = app
        .cfg
        .max_blob_bytes
        .max(app.cfg.max_wasm_bytes)
        .max(app.cfg.max_map_bytes);
    let bytes = match axum::body::to_bytes(body, limit).await {
        Ok(b) => b,
        Err(_) => {
            return ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                format!("the body limit is {limit} bytes"),
            )
            .into_response()
        }
    };
    let payload = match serde_json::from_slice::<Value>(&bytes) {
        _ if bytes.is_empty() => Value::Null,
        Ok(v) if bytes.len() <= 16_384 => v,
        _ => json!({"bytes": bytes.len()}),
    };
    let res = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    let status = res.status().as_u16();
    let logged = app
        .db
        .tx(|tx| m::audit(tx, "admin", &action, status, &payload, now_ms()));
    if let Err(e) = logged {
        eprintln!("audit log: {}", e.message);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_forms() {
        let now = 1_767_225_600_000; // 2026-01-01T00:00:00Z
        assert_eq!(parse_until("7d", now), Ok(now + 7 * 86_400_000));
        assert_eq!(parse_until("12h", now), Ok(now + 12 * 3_600_000));
        assert_eq!(parse_until("30m", now), Ok(now + 30 * 60_000));
        assert_eq!(parse_until("2026-01-02", now), Ok(now + 86_400_000));
        assert_eq!(
            parse_until("2026-03-01T18:30:05Z", now).map(rfc3339),
            Ok("2026-03-01T18:30:05Z".to_string())
        );
        assert_eq!(
            parse_until("2000-02-29", now).map(rfc3339),
            Ok("2000-02-29T00:00:00Z".to_string())
        );
        for bad in [
            "",
            "0d",
            "-1d",
            "xd",
            "2026-02-30",
            "2026-13-01",
            "2026-01-01T25:00:00Z",
            "tomorrow",
            "106751990000d",
            "99999999999-01-01",
        ] {
            assert!(parse_until(bad, now).is_err(), "{bad}");
        }
    }
}
