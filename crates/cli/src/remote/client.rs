//! Blocking HTTP client for the platform API (`/v1`, see the API contract in docs/CLI.md) and the one error
//! type of the remote commands. Every failure carries the exit code it should end the process with.

use serde_json::{json, Value};
use std::fmt;
use std::io::Read;
use std::time::Duration;

/// Exit codes of the remote commands (clap's own usage errors also exit with 2).
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_AUTH: i32 = 3;

/// The longest `Retry-After` that `--wait` sleeps through.
const MAX_WAIT_SECS: u64 = 3600;
/// Replays and maps are read whole; anything bigger than this is not something the platform serves.
const MAX_BODY_BYTES: u64 = 256 << 20;

/// What went wrong, in the shape `--json` prints: `{error, message, status, retry_after}`.
#[derive(Debug)]
pub struct CliError {
    pub exit: i32,
    /// The server's `error` code, or a local one: `usage`, `auth_required`, `network`, `endpoint_missing`,
    /// `bad_response`, `error`.
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
    pub retry_after: Option<u64>,
}

pub type CliResult<T> = Result<T, CliError>;

impl CliError {
    pub fn new(exit: i32, code: &str, message: impl Into<String>) -> CliError {
        CliError {
            exit,
            code: code.to_string(),
            message: message.into(),
            status: None,
            retry_after: None,
        }
    }
    pub fn usage(message: impl Into<String>) -> CliError {
        CliError::new(EXIT_USAGE, "usage", message)
    }
    pub fn auth(message: impl Into<String>) -> CliError {
        CliError::new(EXIT_AUTH, "auth_required", message)
    }
    pub fn other(message: impl Into<String>) -> CliError {
        CliError::new(EXIT_ERROR, "error", message)
    }
    pub fn to_json(&self) -> Value {
        json!({"error": self.code, "message": self.message, "status": self.status, "retry_after": self.retry_after})
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(s) => write!(
                f,
                "{} ({s} {}): {}",
                self.code_label(),
                self.code,
                self.message
            )?,
            None => write!(f, "{}", self.message)?,
        }
        if let Some(secs) = self.retry_after {
            write!(
                f,
                "; retry after {secs} s (or pass --wait to sleep and retry once)"
            )?;
        }
        Ok(())
    }
}

impl CliError {
    fn code_label(&self) -> &'static str {
        match self.status {
            Some(429) => "rate limited",
            Some(401) => "not signed in",
            Some(s) if s >= 500 => "server error",
            _ => "request failed",
        }
    }
}

impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> CliError {
        CliError::other(format!("{e:#}"))
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> CliError {
        CliError::other(e.to_string())
    }
}

/// A request body.
pub enum Body<'a> {
    Empty,
    Json(&'a Value),
    Bytes(&'a [u8], &'a str),
}

pub struct Client {
    agent: ureq::Agent,
    /// `http(s)://host[:port]`, no trailing slash, no `/v1`.
    pub server: String,
    pub token: Option<String>,
    /// Sleep through one 429 and retry (`--wait`).
    pub wait: bool,
}

impl Client {
    pub fn new(server: &str, token: Option<String>, wait: bool) -> Client {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            // Per read/write, not per request: a large replay on a slow link may take a while.
            .timeout_read(Duration::from_secs(60))
            .timeout_write(Duration::from_secs(60))
            .redirects(0) // never replay a bearer token to wherever a redirect points
            .user_agent(concat!("ibc/", env!("CARGO_PKG_VERSION")))
            .build();
        Client {
            agent,
            server: server.to_string(),
            token,
            wait,
        }
    }

    /// The token, or exit 3 with a hint.
    pub fn require_token(&self) -> CliResult<&str> {
        self.token.as_deref().ok_or_else(|| {
            CliError::auth(format!(
                "not signed in to {}: run `ibc login` (or set IBC_TOKEN)",
                self.server
            ))
        })
    }

    pub fn get_json(&self, path: &str) -> CliResult<Value> {
        json_of(self.send("GET", path, Body::Empty)?)
    }

    pub fn post_json(&self, path: &str, body: &Value) -> CliResult<Value> {
        json_of(self.send("POST", path, Body::Json(body))?)
    }

    pub fn get_bytes(&self, path: &str) -> CliResult<Vec<u8>> {
        bytes_of(self.send("GET", path, Body::Empty)?)
    }

    /// Send `method /v1/<path>`; a 429 is slept through and retried once when `wait` is set.
    pub fn send(&self, method: &str, path: &str, body: Body) -> CliResult<ureq::Response> {
        match self.send_once(method, path, &body) {
            Err(e) if e.status == Some(429) && self.wait => match e.retry_after {
                Some(secs) if secs <= MAX_WAIT_SECS => {
                    eprintln!(
                        "rate limited ({}): waiting {secs} s, then retrying once",
                        e.message
                    );
                    std::thread::sleep(Duration::from_secs(secs));
                    self.send_once(method, path, &body)
                }
                _ => Err(e),
            },
            other => other,
        }
    }

    fn send_once(&self, method: &str, path: &str, body: &Body) -> CliResult<ureq::Response> {
        let url = format!("{}/v1/{path}", self.server);
        let mut req = self.agent.request(method, &url);
        if let Some(token) = &self.token {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
        let result = match body {
            Body::Empty => req.call(),
            Body::Json(v) => req
                .set("Content-Type", "application/json")
                .send_string(&v.to_string()),
            Body::Bytes(b, ty) => req.set("Content-Type", ty).send_bytes(b),
        };
        match result {
            Ok(resp) if (200..300).contains(&resp.status()) => Ok(resp),
            Ok(resp) => Err(status_error(path, resp.status(), resp)),
            Err(ureq::Error::Status(code, resp)) => Err(status_error(path, code, resp)),
            Err(ureq::Error::Transport(t)) => Err(CliError::new(
                EXIT_ERROR,
                "network",
                format!("cannot reach {}: {t}", self.server),
            )),
        }
    }
}

pub fn bytes_of(resp: ureq::Response) -> CliResult<Vec<u8>> {
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| CliError::new(EXIT_ERROR, "network", format!("reading the response: {e}")))?;
    if buf.len() as u64 > MAX_BODY_BYTES {
        return Err(CliError::new(
            EXIT_ERROR,
            "bad_response",
            format!("the answer is larger than {MAX_BODY_BYTES} bytes"),
        ));
    }
    Ok(buf)
}

pub fn json_of(resp: ureq::Response) -> CliResult<Value> {
    let bytes = bytes_of(resp)?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|e| {
        CliError::new(
            EXIT_ERROR,
            "bad_response",
            format!("the server's answer is not JSON: {e}"),
        )
    })
}

/// `Retry-After` in seconds (the HTTP-date form is not used by the platform).
fn retry_after_header(resp: &ureq::Response) -> Option<u64> {
    resp.header("Retry-After")
        .and_then(|v| v.trim().parse().ok())
}

/// Turn a non-2xx answer into an error: the server's `{error, message, retry_after}` when it sent one.
fn status_error(path: &str, status: u16, resp: ureq::Response) -> CliError {
    let header_wait = retry_after_header(&resp);
    let text = bytes_of(resp)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let body: Option<Value> = serde_json::from_str(&text).ok();
    let field = |k: &str| {
        body.as_ref()
            .and_then(|b| b.get(k))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let (code, message) = match (field("error"), status) {
        (Some(code), _) => (code, field("message").unwrap_or_default()),
        // A 404 without the API's error body is the router saying the endpoint does not exist.
        (None, 404) => (
            "endpoint_missing".to_string(),
            format!(
                "the server has no endpoint /v1/{} (an older coordinator?)",
                path_only(path)
            ),
        ),
        (None, _) => (format!("http_{status}"), truncate(text.trim(), 300)),
    };
    let body_wait = body
        .as_ref()
        .and_then(|b| b.get("retry_after"))
        .and_then(Value::as_u64);
    let exit = if status == 401 { EXIT_AUTH } else { EXIT_ERROR };
    let message = if status == 401 && !message.contains("ibc login") {
        format!("{message} (run `ibc login`, or check IBC_TOKEN)")
    } else {
        message
    };
    CliError {
        exit,
        code,
        message,
        status: Some(status),
        retry_after: header_wait.or(body_wait),
    }
}

fn path_only(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Percent-encode a path segment or query value.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `?k=v&...` from the pairs whose value is present (empty string when none are).
pub fn query(pairs: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", encode(v))))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_skips_missing_values_and_encodes() {
        assert_eq!(query(&[("a", None)]), "");
        assert_eq!(
            query(&[
                ("a", Some("x y&z".into())),
                ("b", None),
                ("c", Some("1".into()))
            ]),
            "?a=x%20y%26z&c=1"
        );
    }

    #[test]
    fn rate_limit_message_names_the_wait() {
        let mut e = CliError::new(EXIT_ERROR, "rate_limited", "too many uploads");
        e.status = Some(429);
        e.retry_after = Some(42);
        let text = e.to_string();
        assert!(
            text.contains("rate limited") && text.contains("42 s") && text.contains("--wait"),
            "{text}"
        );
        assert_eq!(e.to_json()["retry_after"], 42);
    }
}
