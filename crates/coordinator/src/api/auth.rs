//! Who is calling (admin token, team token, user token or session cookie), CSRF for cookie writes, and the
//! account endpoints: register, login, logout, `me` and device login (docs/api/auth.md).

use super::{parse, teams, AppState};
use crate::accounts::{self, DEVICE_INTERVAL_SECS, DEVICE_TTL_MS, SESSION_COOKIE, SESSION_MS};
use crate::db::users::{self, DevicePoll, UserRow};
use crate::db::{self, TeamRow};
use crate::error::{ApiError, ApiResult};
use crate::util::{new_token, now_ms, sha256_hex};
use crate::App;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// Prefix of user tokens (CLI sign-in); team tokens start with `ibt_`.
pub const USER_TOKEN_PREFIX: &str = "ibu_";
/// Prefix of session tokens: the cookie's value, also accepted as a bearer token.
pub const SESSION_TOKEN_PREFIX: &str = "ibs_";
/// The header a cookie-authenticated write must carry (a cross-site form cannot set it).
pub const CSRF_HEADER: &str = "x-ibc-csrf";

pub(crate) enum Identity {
    Admin,
    Team(TeamRow),
    User(UserRow),
}

/// The session token of the `ibc_session` cookie, if the request has one.
fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(name, value)| *name == SESSION_COOKIE && !value.is_empty())
        .map(|(_, value)| value)
}

fn not_banned(id: Identity) -> ApiResult<Identity> {
    match id {
        Identity::User(u) if u.banned => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "banned",
            "this account is banned",
        )),
        id => Ok(id),
    }
}

/// Resolve the caller. A bearer token wins over the cookie; a bad bearer token is a 401 even with a cookie.
pub(crate) fn identify(app: &App, headers: &HeaderMap) -> ApiResult<Identity> {
    let now = now_ms();
    if headers.contains_key(header::AUTHORIZATION) {
        let token = super::bearer(headers)?;
        if super::is_admin(app, token) {
            return Ok(Identity::Admin);
        }
        let hash = sha256_hex(token.as_bytes());
        let found = app.db.tx(|tx| {
            Ok(if token.starts_with(USER_TOKEN_PREFIX) {
                users::user_by_token(tx, &hash, now)?.map(Identity::User)
            } else if token.starts_with(SESSION_TOKEN_PREFIX) {
                users::user_by_session(tx, &hash, now)?.map(Identity::User)
            } else {
                db::team_by_token(tx, &hash)?.map(Identity::Team)
            })
        })?;
        return not_banned(found.ok_or_else(ApiError::unauthorized)?);
    }
    let token = session_token(headers).ok_or_else(ApiError::unauthorized)?;
    let hash = sha256_hex(token.as_bytes());
    let user = app.db.tx(|tx| users::user_by_session(tx, &hash, now))?;
    not_banned(Identity::User(user.ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "the session expired; sign in again",
        )
    })?))
}

/// A signed-in user (session cookie or `ibu` token); team and admin tokens are refused.
pub(crate) fn auth_user(app: &App, headers: &HeaderMap) -> ApiResult<UserRow> {
    match identify(app, headers)? {
        Identity::User(u) => Ok(u),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "user_required",
            "this endpoint needs a user session or a user token (`ibc login`)",
        )),
    }
}

/// A user acting on a team route acts for their team; an admin user without a team acts as the organisers.
pub(crate) fn user_caller(app: &App, user: UserRow) -> ApiResult<super::Caller> {
    match app.db.tx(|tx| users::team_of_user(tx, &user.id))? {
        Some(team) => Ok(super::Caller::Team(team)),
        None if user.is_admin => Ok(super::Caller::Admin),
        None => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "no_team",
            "you are not in a team; create one or join one with a join code",
        )),
    }
}

/// Middleware: a write authenticated by the session cookie must carry `X-IBC-CSRF: 1`. Bearer requests
/// and requests without the cookie pass.
pub(crate) async fn csrf_guard(req: Request, next: Next) -> Response {
    let headers = req.headers();
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let cookie_auth =
        !headers.contains_key(header::AUTHORIZATION) && session_token(headers).is_some();
    let marked = headers
        .get(CSRF_HEADER)
        .is_some_and(|v| v.as_bytes() == b"1");
    if !safe && cookie_auth && !marked {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "csrf",
            "writes authenticated by the session cookie need the header `X-IBC-CSRF: 1`",
        )
        .into_response();
    }
    next.run(req).await
}

/// 429 with `retry_after` in the body and a `Retry-After` header.
pub(crate) fn too_many(code: &str, message: &str, retry_after: i64) -> Response {
    let retry_after = retry_after.max(1);
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, retry_after.to_string())],
        Json(json!({"error": code, "message": message, "retry_after": retry_after})),
    )
        .into_response()
}

fn cookie(app: &App, value: &str, max_age_secs: i64) -> String {
    let secure = if app.cfg.auth.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{SESSION_COOKIE}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_secs}{secure}"
    )
}

/// Answer a sign-in: the session token in the body and in the cookie.
fn signed_in(app: &App, status: StatusCode, user: &UserRow, token: &str) -> Response {
    (
        status,
        [(header::SET_COOKIE, cookie(app, token, SESSION_MS / 1000))],
        Json(json!({"user": user.json(), "session_token": token})),
    )
        .into_response()
}

fn start_session(tx: &rusqlite::Transaction, user_id: &str, now: i64) -> ApiResult<String> {
    let token = new_token("ibs");
    users::create_session(
        tx,
        &sha256_hex(token.as_bytes()),
        user_id,
        now,
        now + SESSION_MS,
    )?;
    Ok(token)
}

/// Password hashes running at once. Each argon2 call holds `argon2_memory_kib` (19 MiB) for its duration, so
/// a flood of sign-ins queues here instead of exhausting memory.
static HASHING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// Run a password hash or check on the blocking pool, at most `HASHING` at a time.
async fn hashing<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    let _permit = HASHING.acquire().await.map_err(ApiError::internal)?;
    tokio::task::spawn_blocking(f)
        .await
        .map_err(ApiError::internal)
}

// ---- register, login, logout, me ----

#[derive(Deserialize)]
struct RegisterReq {
    email: String,
    password: String,
    display_name: String,
}

pub async fn register(State(app): AppState, body: Bytes) -> ApiResult<Response> {
    let req: RegisterReq = parse(&body)?;
    let email = accounts::normalize_email(&req.email)
        .ok_or_else(|| ApiError::bad_request("that is not an email address"))?;
    accounts::check_password(&req.password)?;
    let display_name = accounts::check_display_name(&req.display_name)?;
    let cost = app.cfg.auth.argon2_memory_kib;
    let hash = hashing(move || accounts::hash_password(&req.password, cost)).await??;
    let admin = app.cfg.auth.is_admin_email(&email);
    let now = now_ms();
    let (user, token) = app.db.tx(|tx| {
        let user = users::create_user(tx, &email, &display_name, &hash, admin, now)?;
        Ok((user.clone(), start_session(tx, &user.id, now)?))
    })?;
    Ok(signed_in(&app, StatusCode::CREATED, &user, &token))
}

#[derive(Deserialize)]
struct LoginReq {
    email: String,
    password: String,
}

pub async fn login(State(app): AppState, body: Bytes) -> ApiResult<Response> {
    let req: LoginReq = parse(&body)?;
    let email =
        accounts::normalize_email(&req.email).unwrap_or_else(|| req.email.trim().to_lowercase());
    let cfg = &app.cfg.auth;
    let now = now_ms();
    let since = now - cfg.login_window_ms;
    // The attempt counts as a failure until it succeeds, and is counted in the same transaction as the
    // check, so parallel guesses cannot all pass the check before any of them is recorded.
    let (failures, oldest, found) = app.db.tx(|tx| {
        let (failures, oldest) = users::recent_failures(tx, &email, since)?;
        if failures >= cfg.login_max_failures {
            return Ok((failures, oldest, None));
        }
        users::record_failure(tx, &email, now, since)?;
        Ok((failures, oldest, users::user_for_login(tx, &email)?))
    })?;
    if failures >= cfg.login_max_failures {
        let wait_ms = oldest.unwrap_or(now) + cfg.login_window_ms - now;
        return Ok(too_many(
            "rate_limited",
            "too many failed sign-ins for this email; try again later",
            (wait_ms + 999) / 1000,
        ));
    }
    let (stored, cost) = (
        found.as_ref().map(|(_, h)| h.clone()),
        cfg.argon2_memory_kib,
    );
    let password = req.password;
    let ok = hashing(move || accounts::verify_password(stored.as_deref(), &password, cost)).await?;
    let user = match found {
        // Whatever the password, so that a banned account is not a password oracle.
        Some((user, _)) if user.banned => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "banned",
                "this account is banned",
            ))
        }
        Some((user, _)) if ok => user,
        _ => {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "invalid credentials",
            ))
        }
    };
    let promote = !user.is_admin && cfg.is_admin_email(&user.email);
    let token = app.db.tx(|tx| {
        users::clear_failures(tx, &email)?;
        if promote {
            users::promote_admin(tx, &user.id)?;
        }
        start_session(tx, &user.id, now)
    })?;
    let user = UserRow {
        is_admin: user.is_admin || promote,
        ..user
    };
    Ok(signed_in(&app, StatusCode::OK, &user, &token))
}

/// End the cookie's session, or revoke the `ibu` token the request carries. Always clears the cookie.
pub async fn logout(State(app): AppState, headers: HeaderMap) -> ApiResult<Response> {
    let bearer = super::bearer(&headers).ok();
    app.db.tx(|tx| {
        if let Some(token) = bearer.filter(|t| t.starts_with(USER_TOKEN_PREFIX)) {
            users::delete_user_token(tx, &sha256_hex(token.as_bytes()))?;
        }
        let session = bearer.filter(|t| t.starts_with(SESSION_TOKEN_PREFIX));
        if let Some(token) = session.or_else(|| session_token(&headers)) {
            users::delete_session(tx, &sha256_hex(token.as_bytes()))?;
        }
        Ok(())
    })?;
    Ok((
        [(header::SET_COOKIE, cookie(&app, "", 0))],
        Json(json!({"ok": true})),
    )
        .into_response())
}

pub async fn me(State(app): AppState, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let user = auth_user(&app, &headers)?;
    let team = app.db.tx(|tx| match users::team_of_user(tx, &user.id)? {
        Some(team) => teams::team_view(tx, &team).map(Some),
        None => Ok(None),
    })?;
    Ok(Json(json!({"user": user.json(), "team": team})))
}

// ---- device login (RFC 8628 shape) ----

pub async fn device_start(State(app): AppState) -> ApiResult<Json<Value>> {
    let device_code = new_token("ibd");
    let hash = sha256_hex(device_code.as_bytes());
    let now = now_ms();
    let user_code = app.db.tx(|tx| loop {
        let code = accounts::new_user_code();
        if users::create_device_code(tx, &hash, &code, now, now + DEVICE_TTL_MS)? {
            return Ok(code);
        }
    })?;
    Ok(Json(json!({
        "device_code": device_code,
        "user_code": user_code,
        "verify_url": format!("{}/device?code={user_code}", app.cfg.auth.public_url),
        "interval": DEVICE_INTERVAL_SECS,
        "expires_in": DEVICE_TTL_MS / 1000,
    })))
}

#[derive(Deserialize)]
struct ApproveReq {
    user_code: String,
}

pub async fn device_approve(
    State(app): AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let user = auth_user(&app, &headers)?;
    let req: ApproveReq = parse(&body)?;
    let code = accounts::normalize_user_code(&req.user_code);
    if !app
        .db
        .tx(|tx| users::approve_device_code(tx, &code, &user.id, now_ms()))?
    {
        return Err(ApiError::not_found(
            "no pending sign-in with this code (it may have expired; run `ibc login` again)",
        ));
    }
    Ok(Json(json!({"ok": true, "user_code": code})))
}

#[derive(Deserialize)]
struct TokenReq {
    device_code: String,
}

pub async fn device_token(State(app): AppState, body: Bytes) -> ApiResult<Response> {
    let req: TokenReq = parse(&body)?;
    let hash = sha256_hex(req.device_code.as_bytes());
    let now = now_ms();
    // Polls up to a second early are tolerated, so a client sleeping exactly `interval` is never refused.
    let min_gap = DEVICE_INTERVAL_SECS * 1000 - 1000;
    let issued = app.db.tx(|tx| {
        Ok(match users::poll_device_code(tx, &hash, now, min_gap)? {
            DevicePoll::Approved(user_id) => {
                let token = new_token("ibu");
                let hash = sha256_hex(token.as_bytes());
                users::create_user_token(tx, &hash, &user_id, "device login", now)?;
                Ok((users::user_by_id(tx, &user_id)?, token))
            }
            other => Err(other),
        })
    })?;
    match issued {
        Ok((user, token)) => Ok(Json(json!({"token": token, "user": user.json()})).into_response()),
        Err(DevicePoll::Pending { too_soon: true }) => Ok(too_many(
            "slow_down",
            "polling too fast; wait `interval` seconds between polls",
            DEVICE_INTERVAL_SECS,
        )),
        Err(DevicePoll::Pending { too_soon: false }) => Err(ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "authorization_pending",
            "waiting for the code to be approved in the browser",
        )),
        Err(DevicePoll::Expired) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "expired_token",
            "the device code expired; start again",
        )),
        Err(_) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "unknown or already used device code",
        )),
    }
}
