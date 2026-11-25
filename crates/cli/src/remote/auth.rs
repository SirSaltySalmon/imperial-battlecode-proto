//! `ibc login`, `ibc logout`, `ibc whoami`.
//!
//! Login is the OAuth device flow (RFC 8628) as the platform exposes it: `POST auth/device` hands out a
//! device code and a short user code, the user approves the code in the browser, and `POST auth/device/token`
//! answers 428 `authorization_pending` until then and `{token}` after.

use super::client::{CliError, CliResult, Client, EXIT_AUTH};
use super::config::{self, Credentials, TokenKind};
use super::{missing, s, Opts};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// RFC 8628's default when the server names no interval.
const DEFAULT_INTERVAL_SECS: u64 = 5;

pub fn login(opts: &Opts, token: Option<String>) -> CliResult<()> {
    let server = config::resolve_env(opts.server.as_deref())?.server;
    let token = match token {
        Some(t) => {
            let t = t.trim().to_string();
            if TokenKind::of(&t) == TokenKind::Unknown {
                return Err(CliError::usage(
                    "a token starts with ibu (user) or ibt (team)",
                ));
            }
            t
        }
        None => device_flow(&Client::new(&server, None, opts.wait))?,
    };
    // Only keep a token the server accepts.
    let me = whoami_value(&Client::new(&server, Some(token.clone()), opts.wait))?;
    let path = config::credentials_path()?;
    config::save(
        &path,
        &Credentials {
            server: server.clone(),
            token,
        },
    )?;
    let mut out = me;
    out["saved_to"] = json!(path.display().to_string());
    opts.emit(&out, |v| {
        println!("signed in to {server} as {}", describe(v));
        println!("credentials saved to {}", path.display());
    });
    Ok(())
}

fn device_flow(client: &Client) -> CliResult<String> {
    let start = client.post_json("auth/device", &json!({}))?;
    let device_code = start
        .get("device_code")
        .and_then(Value::as_str)
        .ok_or_else(|| missing("device_code"))?;
    let mut interval = start
        .get("interval")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_INTERVAL_SECS)
        .max(1);
    let expires_in = start
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(600);
    // The prompt goes to stderr so `--json` keeps stdout for the result.
    eprintln!(
        "To sign in, open {} and enter the code {}",
        s(&start, "verify_url"),
        s(&start, "user_code")
    );
    eprintln!(
        "Waiting for approval (the code expires in {} min)...",
        expires_in.div_ceil(60)
    );
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    loop {
        std::thread::sleep(Duration::from_secs(interval));
        match client.post_json("auth/device/token", &json!({"device_code": device_code})) {
            Ok(v) => {
                return v
                    .get("token")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| missing("token"))
            }
            Err(e) if e.code == "authorization_pending" => {}
            Err(e) if e.code == "slow_down" => interval += 5,
            Err(e) if e.status == Some(429) => interval = interval.max(e.retry_after.unwrap_or(5)),
            Err(e) if matches!(e.code.as_str(), "expired_token" | "access_denied") => {
                return Err(CliError::new(
                    EXIT_AUTH,
                    &e.code,
                    format!("sign-in failed: {}", e.message),
                ))
            }
            Err(e) => return Err(e),
        }
        if Instant::now() >= deadline {
            return Err(CliError::new(
                EXIT_AUTH,
                "expired_token",
                "the code expired before it was approved; run `ibc login` again",
            ));
        }
    }
}

pub fn logout(opts: &Opts) -> CliResult<()> {
    let path = config::credentials_path()?;
    // A damaged file is removed all the same.
    let stored = config::load(&path).ok().flatten();
    let existed = path.exists();
    // End a user session on the server too; a team token is the organisers' to revoke.
    if let Some(c) = stored
        .as_ref()
        .filter(|c| TokenKind::of(&c.token) == TokenKind::User)
    {
        if let Ok(server) = config::normalize_server(&c.server) {
            let _ = Client::new(&server, Some(c.token.clone()), false)
                .post_json("auth/logout", &json!({}));
        }
    }
    if existed {
        std::fs::remove_file(&path)
            .map_err(|e| CliError::other(format!("{}: {e}", path.display())))?;
    }
    let out = json!({"logged_out": existed, "path": path.display().to_string()});
    opts.emit(&out, |_| match (existed, stored) {
        (true, Some(c)) => println!("signed out of {} (removed {})", c.server, path.display()),
        (true, None) => println!("removed {}", path.display()),
        (false, _) => println!("not signed in"),
    });
    Ok(())
}

pub fn whoami(opts: &Opts) -> CliResult<()> {
    let client = opts.client()?;
    client.require_token()?;
    let out = whoami_value(&client)?;
    opts.emit(&out, |v| println!("{} on {}", describe(v), client.server));
    Ok(())
}

/// `{server, token_kind, user, team}`, from `GET me` (user tokens) or `GET team` (team tokens, or a server
/// without `me`). `team` always has `id` and `name`.
pub fn whoami_value(client: &Client) -> CliResult<Value> {
    let token = client.require_token()?;
    let kind = TokenKind::of(token);
    let (user, team) = match kind {
        TokenKind::Team => (Value::Null, client.get_json("team")?),
        _ => match client.get_json("me") {
            Ok(me) => (
                me.get("user").cloned().unwrap_or(Value::Null),
                me.get("team").cloned().unwrap_or(Value::Null),
            ),
            Err(e) if e.code == "endpoint_missing" => (Value::Null, client.get_json("team")?),
            Err(e) => return Err(e),
        },
    };
    Ok(
        json!({"server": client.server, "token_kind": kind.name(), "user": user, "team": normalize_team(team)}),
    )
}

/// `GET team` says `team_id`, `GET me` says `id`: give both forms an `id`.
fn normalize_team(mut team: Value) -> Value {
    if let Some(obj) = team.as_object_mut() {
        if !obj.contains_key("id") {
            if let Some(id) = obj.get("team_id").cloned() {
                obj.insert("id".to_string(), id);
            }
        }
    }
    team
}

/// The caller's team id (for `--mine`).
pub fn my_team_id(client: &Client) -> CliResult<String> {
    let me = whoami_value(client)?;
    me["team"]["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::other("you are not in a team yet: create or join one on the platform first")
        })
}

fn describe(v: &Value) -> String {
    let team = match v["team"]["name"].as_str() {
        Some(name) => format!("team {name} ({})", s(&v["team"], "id")),
        None => "no team".to_string(),
    };
    match v["user"].as_object() {
        Some(_) => format!(
            "{} <{}>, {team}",
            s(&v["user"], "display_name"),
            s(&v["user"], "email")
        ),
        None => format!("{team} (team token)"),
    }
}
