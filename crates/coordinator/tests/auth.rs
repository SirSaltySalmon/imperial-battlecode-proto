//! Accounts, sessions, CSRF, teams of users, device login, sign-in throttling.

mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use common::{bot_wasm, Harness, ADMIN};
use http_body_util::BodyExt;
use ibc_coordinator::config::Config;
use serde_json::{json, Value};
use tower::ServiceExt;
use Auth::{Anon, Bearer, Cookie, CookieNoCsrf};

const OK: StatusCode = StatusCode::OK;
const CREATED: StatusCode = StatusCode::CREATED;
const BAD: StatusCode = StatusCode::BAD_REQUEST;
const UNAUTH: StatusCode = StatusCode::UNAUTHORIZED;
const FORBIDDEN: StatusCode = StatusCode::FORBIDDEN;
const NOT_FOUND: StatusCode = StatusCode::NOT_FOUND;
const CONFLICT: StatusCode = StatusCode::CONFLICT;
const PENDING: StatusCode = StatusCode::PRECONDITION_REQUIRED;
const SLOW: StatusCode = StatusCode::TOO_MANY_REQUESTS;
const PW: &str = "hunter2hunter2";

/// How a request authenticates.
#[derive(Clone, Copy)]
enum Auth<'a> {
    Anon,
    /// Session cookie, with the CSRF header.
    Cookie(&'a str),
    /// Session cookie without the CSRF header.
    CookieNoCsrf(&'a str),
    Bearer(&'a str),
}

struct Res {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Res {
    /// Assert the status (and the error code, if given); returns the response for further checks.
    #[track_caller]
    fn is(self, status: StatusCode, error: Option<&str>) -> Res {
        assert_eq!(self.status, status, "{}", self.body);
        if let Some(error) = error {
            assert_eq!(self.body["error"], error, "{}", self.body);
        }
        self
    }

    fn str(&self, key: &str) -> String {
        self.body[key].as_str().unwrap().to_string()
    }

    fn header(&self, name: &str) -> &str {
        self.headers[name].to_str().unwrap()
    }
}

struct T {
    h: Harness,
}

impl T {
    fn new(tune: impl FnOnce(&mut Config)) -> T {
        let h = Harness::new(|c| {
            c.auth.argon2_memory_kib = 64;
            tune(c)
        });
        T { h }
    }

    async fn send(&self, method: &str, path: &str, auth: Auth<'_>, body: Vec<u8>) -> Res {
        let mut req = Request::builder().method(method).uri(path);
        match auth {
            Anon => {}
            Cookie(s) => {
                req = req
                    .header("cookie", format!("theme=dark; ibc_session={s}"))
                    .header("x-ibc-csrf", "1")
            }
            CookieNoCsrf(s) => req = req.header("cookie", format!("ibc_session={s}")),
            Bearer(t) => req = req.header("authorization", format!("Bearer {t}")),
        }
        let res = ibc_coordinator::api::router(self.h.app.clone())
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Res {
            status,
            headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn post(&self, path: &str, auth: Auth<'_>, body: Value) -> Res {
        let bytes = if body.is_null() {
            Vec::new()
        } else {
            body.to_string().into_bytes()
        };
        self.send("POST", path, auth, bytes).await
    }

    async fn get(&self, path: &str, auth: Auth<'_>) -> Res {
        self.send("GET", path, auth, Vec::new()).await
    }

    /// Register a user; returns their session token.
    async fn register(&self, email: &str) -> String {
        let name = email.split('@').next().unwrap();
        let body = json!({"email": email, "password": PW, "display_name": name});
        self.post("/v1/auth/register", Anon, body)
            .await
            .is(CREATED, None)
            .str("session_token")
    }

    async fn login(&self, email: &str, password: &str) -> Res {
        let body = json!({"email": email, "password": password});
        self.post("/v1/auth/login", Anon, body).await
    }

    /// Create a team as this user; returns its join code.
    async fn create_team(&self, session: &str, name: &str) -> String {
        self.post("/v1/teams", Cookie(session), json!({"name": name}))
            .await
            .is(CREATED, None)
            .str("join_code")
    }

    async fn join(&self, session: &str, code: &str) -> Res {
        let body = json!({"join_code": code});
        self.post("/v1/teams/join", Cookie(session), body).await
    }

    async fn poll(&self, device_code: &str) -> Res {
        let body = json!({"device_code": device_code});
        self.post("/v1/auth/device/token", Anon, body).await
    }

    async fn approve(&self, auth: Auth<'_>, user_code: &str) -> Res {
        let body = json!({"user_code": user_code});
        self.post("/v1/auth/device/approve", auth, body).await
    }

    /// Run the device flow for the user behind `session`; returns the `ibu` token.
    async fn device_login(&self, session: &str) -> String {
        let start = self.post("/v1/auth/device", Anon, Value::Null).await;
        self.approve(Cookie(session), &start.str("user_code"))
            .await
            .is(OK, None);
        self.poll(&start.str("device_code"))
            .await
            .is(OK, None)
            .str("token")
    }

    fn sql(&self, sql: &str) {
        self.h.app.db.tx(|tx| Ok(tx.execute(sql, [])?)).unwrap();
    }
}

#[tokio::test]
async fn register_login_logout_me() {
    let t = T::new(|_| {});
    let body = json!({"email": " Ada@Example.org", "password": PW, "display_name": " Ada "});
    let res = t
        .post("/v1/auth/register", Anon, body)
        .await
        .is(CREATED, None);
    let user = &res.body["user"];
    assert_eq!(
        (&user["email"], &user["display_name"], &user["is_admin"]),
        (&json!("ada@example.org"), &json!("Ada"), &json!(false))
    );
    let session = res.str("session_token");
    let cookie = res.header("set-cookie");
    assert!(cookie.starts_with(&format!("ibc_session={session};")));
    for attr in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=604800"] {
        assert!(cookie.contains(attr), "{cookie}");
    }
    assert!(!cookie.contains("Secure"));

    let me = t.get("/v1/me", Cookie(&session)).await.is(OK, None);
    assert_eq!(me.body["user"]["email"], "ada@example.org");
    assert!(me.body["team"].is_null());
    // The session token also works as a bearer token; no session at all is a 401.
    t.get("/v1/me", Bearer(&session)).await.is(OK, None);
    t.get("/v1/me", Anon).await.is(UNAUTH, None);
    t.get("/v1/me", Cookie("ibs_nope")).await.is(UNAUTH, None);

    // Duplicate email (any case), bad inputs.
    let dup = json!({"email": "ADA@example.org", "password": "another one", "display_name": "A"});
    let dup = t.post("/v1/auth/register", Anon, dup).await;
    dup.is(CONFLICT, Some("email_taken"));
    for bad in [
        json!({"email": "ada", "password": PW, "display_name": "x"}),
        json!({"email": "b@example.org", "password": "short", "display_name": "x"}),
        json!({"email": "b@example.org", "password": PW, "display_name": "  "}),
    ] {
        t.post("/v1/auth/register", Anon, bad).await.is(BAD, None);
    }

    // Wrong password and unknown email look the same.
    let wrong = t.login("ada@example.org", "wrong password").await;
    let unknown = t.login("zed@example.org", "wrong password").await;
    let wrong = wrong.is(UNAUTH, Some("invalid_credentials"));
    assert_eq!(wrong.body, unknown.body);
    let second = t.login("ADA@example.org ", PW).await.is(OK, None);
    let second = second.str("session_token");
    assert_ne!(second, session);

    // Logout ends that session only, and clears the cookie.
    let out = t
        .post("/v1/auth/logout", Cookie(&session), Value::Null)
        .await;
    assert!(out.is(OK, None).header("set-cookie").contains("Max-Age=0"));
    t.get("/v1/me", Cookie(&session)).await.is(UNAUTH, None);
    t.get("/v1/me", Cookie(&second)).await.is(OK, None);
}

#[tokio::test]
async fn cookie_writes_need_the_csrf_header() {
    let t = T::new(|c| c.auth.cookie_secure = true);
    let body = json!({"email": "a@example.org", "password": PW, "display_name": "A"});
    let res = t.post("/v1/auth/register", Anon, body).await;
    assert!(res.header("set-cookie").ends_with("; Secure"));
    let session = res.str("session_token");

    let team = json!({"name": "alpha"});
    let blocked = t
        .post("/v1/teams", CookieNoCsrf(&session), team.clone())
        .await;
    blocked.is(FORBIDDEN, Some("csrf"));
    // Reads do not need it, and a bearer token skips it.
    t.get("/v1/me", CookieNoCsrf(&session)).await.is(OK, None);
    t.post("/v1/teams", Bearer(&session), team)
        .await
        .is(CREATED, None);
    // Requests without the cookie are untouched.
    t.login("a@example.org", PW).await.is(OK, None);
}

#[tokio::test]
async fn teams_create_join_leave_and_cap() {
    let t = T::new(|c| c.auth.team_max = 2);
    let ada = t.register("ada@x.org").await;
    let bob = t.register("bob@x.org").await;
    let cy = t.register("cy@x.org").await;
    let code = t.create_team(&ada, "alpha").await;
    assert_eq!(code.len(), 10);

    let me = t.get("/v1/me", Cookie(&ada)).await;
    let team = &me.body["team"];
    assert_eq!(
        (&team["name"], &team["join_code"]),
        (&json!("alpha"), &json!(code))
    );
    assert_eq!(team["members"][0]["role"], "owner");
    // One team per user; team names stay unique.
    let beta = t
        .post("/v1/teams", Cookie(&ada), json!({"name": "beta"}))
        .await;
    beta.is(CONFLICT, Some("already_in_team"));
    let taken = t
        .post("/v1/teams", Cookie(&cy), json!({"name": "alpha"}))
        .await;
    taken.is(CONFLICT, Some("team_exists"));

    t.join(&bob, "NOPENOPENO").await.is(NOT_FOUND, None);
    let joined = t.join(&bob, &code.to_lowercase()).await.is(OK, None);
    assert_eq!(joined.body["team"]["members"].as_array().unwrap().len(), 2);
    t.join(&cy, &code).await.is(CONFLICT, Some("team_full"));

    // Leaving frees a seat; leaving twice is a conflict.
    let leave = || t.post("/v1/teams/leave", Cookie(&bob), Value::Null);
    leave().await.is(OK, None);
    leave().await.is(CONFLICT, Some("no_team"));
    assert!(t.get("/v1/me", Cookie(&bob)).await.body["team"].is_null());

    // A rotated code replaces the old one.
    let rotate = t
        .post("/v1/teams/join-code/rotate", Cookie(&ada), Value::Null)
        .await;
    let new_code = rotate.is(OK, None).str("join_code");
    assert_ne!(new_code, code);
    t.join(&cy, &code).await.is(NOT_FOUND, None);
    t.join(&cy, &new_code).await.is(OK, None);
}

#[tokio::test]
async fn an_account_creates_a_bounded_number_of_teams_and_leaving_rotates_the_token() {
    let t = T::new(|_| {});
    let ada = t.register("ada@x.org").await;
    let bob = t.register("bob@x.org").await;
    let code = t.create_team(&ada, "t0").await;
    t.join(&bob, &code).await.is(OK, None);
    let regen = t.post("/v1/team/token", Cookie(&bob), Value::Null).await;
    let ibt = regen.is(OK, None).str("token");
    t.get("/v1/team", Bearer(&ibt)).await.is(OK, None);
    // The leaver's copy of the team token stops working.
    let leave = t.post("/v1/teams/leave", Cookie(&bob), Value::Null).await;
    leave.is(OK, None);
    t.get("/v1/team", Bearer(&ibt)).await.is(UNAUTH, None);

    for i in 1..3 {
        t.post("/v1/teams/leave", Cookie(&ada), Value::Null)
            .await
            .is(OK, None);
        t.create_team(&ada, &format!("t{i}")).await;
    }
    t.post("/v1/teams/leave", Cookie(&ada), Value::Null)
        .await
        .is(OK, None);
    let fourth = t
        .post("/v1/teams", Cookie(&ada), json!({"name": "t3"}))
        .await;
    fourth.is(FORBIDDEN, Some("team_limit"));
}

#[tokio::test]
async fn team_routes_accept_sessions_and_user_tokens() {
    let t = T::new(|_| {});
    let ada = t.register("ada@x.org").await;
    let loner = t.register("loner@x.org").await;
    t.get("/v1/team", Cookie(&loner))
        .await
        .is(FORBIDDEN, Some("no_team"));
    // Team creation does not reveal the team token.
    let created = t
        .post("/v1/teams", Cookie(&ada), json!({"name": "alpha"}))
        .await;
    assert!(
        !created.body.to_string().contains("ibt_"),
        "{}",
        created.body
    );

    let up = t
        .send("POST", "/v1/submissions", Cookie(&ada), bot_wasm("a"))
        .await;
    let up = up.is(CREATED, None);
    let no_csrf = t
        .send("POST", "/v1/submissions", CookieNoCsrf(&ada), bot_wasm("b"))
        .await;
    no_csrf.is(FORBIDDEN, Some("csrf"));
    let team = t.get("/v1/team", Cookie(&ada)).await;
    assert_eq!(team.body["name"], "alpha");
    assert_eq!(team.body["live_submission"], up.body["submission_id"]);

    // A device-login token acts for the team too, with no CSRF header.
    let token = t.device_login(&ada).await;
    assert!(token.starts_with("ibu_"));
    let path = "/v1/submissions?live=false";
    t.send("POST", path, Bearer(&token), bot_wasm("c"))
        .await
        .is(CREATED, None);
    let subs = t.get("/v1/submissions", Bearer(&token)).await;
    assert_eq!(subs.body["submissions"].as_array().unwrap().len(), 2);

    // The team token, regenerated for automation, works; the user token is revoked by logout.
    let regen = t.post("/v1/team/token", Bearer(&token), Value::Null).await;
    let ibt = regen.is(OK, None).str("token");
    assert!(ibt.starts_with("ibt_"));
    assert_eq!(t.get("/v1/team", Bearer(&ibt)).await.body["name"], "alpha");
    t.post("/v1/auth/logout", Bearer(&token), Value::Null)
        .await
        .is(OK, None);
    t.get("/v1/team", Bearer(&token)).await.is(UNAUTH, None);
    // User-only routes refuse team tokens.
    t.get("/v1/me", Bearer(&ibt))
        .await
        .is(FORBIDDEN, Some("user_required"));
}

#[tokio::test]
async fn device_flow_pending_approve_token() {
    let t = T::new(|c| c.auth.public_url = "https://ibc.example".into());
    let ada = t.register("ada@x.org").await;
    let start = t
        .post("/v1/auth/device", Anon, Value::Null)
        .await
        .is(OK, None);
    let (user_code, device_code) = (start.str("user_code"), start.str("device_code"));
    assert_eq!((user_code.len(), &user_code[4..5]), (9, "-"));
    let url = format!("https://ibc.example/device?code={user_code}");
    assert_eq!(start.str("verify_url"), url);
    assert_eq!(start.body["interval"], 5);
    assert_eq!(start.body["expires_in"], 600);

    t.poll(&device_code)
        .await
        .is(PENDING, Some("authorization_pending"));
    let fast = t.poll(&device_code).await.is(SLOW, Some("slow_down"));
    assert_eq!(fast.header("retry-after"), "5");

    // Approval needs a user; the code is accepted in any case and without the dash.
    let typed = user_code.to_lowercase().replace('-', "");
    t.approve(Anon, &typed).await.is(UNAUTH, None);
    t.approve(Cookie(&ada), "BBBB-BBBB")
        .await
        .is(NOT_FOUND, None);
    t.approve(Cookie(&ada), &typed).await.is(OK, None);
    t.approve(Cookie(&ada), &typed).await.is(NOT_FOUND, None);

    let done = t.poll(&device_code).await.is(OK, None);
    assert_eq!(done.body["user"]["email"], "ada@x.org");
    let me = t.get("/v1/me", Bearer(&done.str("token"))).await;
    assert_eq!(me.body["user"]["email"], "ada@x.org");
    // The code is spent.
    t.poll(&device_code).await.is(BAD, Some("invalid_grant"));
}

#[tokio::test]
async fn expired_device_codes() {
    let t = T::new(|_| {});
    let ada = t.register("ada@x.org").await;
    let start = t.post("/v1/auth/device", Anon, Value::Null).await;
    t.sql("UPDATE device_codes SET expires_at = 0");
    t.approve(Cookie(&ada), &start.str("user_code"))
        .await
        .is(NOT_FOUND, None);
    t.poll(&start.str("device_code"))
        .await
        .is(BAD, Some("expired_token"));
}

#[tokio::test]
async fn login_throttling_per_email() {
    let t = T::new(|c| c.auth.login_max_failures = 3);
    t.register("ada@x.org").await;
    for _ in 0..3 {
        t.login("ada@x.org", "wrong password")
            .await
            .is(UNAUTH, None);
    }
    // Throttled, even with the right password.
    for pw in ["wrong password", PW] {
        let res = t
            .login("ada@x.org", pw)
            .await
            .is(SLOW, Some("rate_limited"));
        let retry = res.body["retry_after"].as_i64().unwrap();
        assert!((1..=900).contains(&retry), "{retry}");
        assert_eq!(res.header("retry-after"), retry.to_string());
    }
    // Other emails are not affected.
    t.register("bob@x.org").await;
    t.login("bob@x.org", PW).await.is(OK, None);
}

#[tokio::test]
async fn admin_users_and_banned_users() {
    let t = T::new(|c| c.auth.admin_emails = vec!["boss@x.org".into()]);
    let boss = t.register("Boss@x.org").await;
    let me = t.get("/v1/me", Cookie(&boss)).await;
    assert_eq!(me.body["user"]["is_admin"], true);
    t.get("/v1/admin/teams", Cookie(&boss)).await.is(OK, None);
    // An admin user without a team acts as the organisers on team routes.
    let omega = json!({"name": "omega"});
    t.post("/v1/admin/teams", Cookie(&boss), omega)
        .await
        .is(CREATED, None);
    let path = "/v1/teams/join-code/rotate?team=omega";
    let code = t.post(path, Cookie(&boss), Value::Null).await;
    let code = code.is(OK, None).str("join_code");
    let ada = t.register("ada@x.org").await;
    t.get("/v1/admin/teams", Cookie(&ada))
        .await
        .is(UNAUTH, None);
    // ...and a user can join a team the organisers created, by its code.
    let joined = t.join(&ada, &code).await.is(OK, None);
    assert_eq!(joined.body["team"]["name"], "omega");
    t.get("/v1/admin/teams", Bearer(ADMIN)).await.is(OK, None);

    t.sql("UPDATE users SET banned_at = 1 WHERE email = 'ada@x.org'");
    t.get("/v1/me", Cookie(&ada))
        .await
        .is(FORBIDDEN, Some("banned"));
    t.login("ada@x.org", PW).await.is(FORBIDDEN, Some("banned"));
}
