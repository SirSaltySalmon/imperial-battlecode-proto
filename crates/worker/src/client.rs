//! Blocking HTTPS client for the lease protocol. Every failure is classified once, here, into what the
//! caller should do about it.

use crate::protocol::CompleteBody;
use serde_json::{json, Value};
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::RwLock;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Network trouble, 408, 429 or 5xx: retry with backoff.
    Transient(String),
    /// 409: the lease is gone, drop the job.
    LeaseLost,
    /// 404: unknown job or blob.
    NotFound,
    /// 401/403: the worker token (or registration token) is not accepted.
    Unauthorized,
    /// Any other 4xx, or a response we cannot use: retrying the same request will not help.
    Rejected(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Transient(m) => write!(f, "transient: {m}"),
            ApiError::LeaseLost => write!(f, "lease lost (409)"),
            ApiError::NotFound => write!(f, "not found (404)"),
            ApiError::Unauthorized => write!(f, "not authorised (401 or 403)"),
            ApiError::Rejected(m) => write!(f, "rejected: {m}"),
        }
    }
}

pub struct Client {
    agent: ureq::Agent,
    base: String,
    token: RwLock<String>,
    timeout: Duration,
}

/// Percent-encode one path segment (job ids are opaque strings chosen by the coordinator).
fn segment(s: &str) -> String {
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

/// With redirects off, ureq hands a 3xx back as a response. Only 2xx is an answer from the coordinator.
fn sent(result: Result<ureq::Response, ureq::Error>) -> Result<ureq::Response, ApiError> {
    let resp = result.map_err(classify)?;
    match resp.status() {
        200..=299 => Ok(resp),
        code => Err(ApiError::Rejected(format!(
            "unexpected HTTP {code} (redirects are not followed)"
        ))),
    }
}

fn classify(e: ureq::Error) -> ApiError {
    match e {
        ureq::Error::Status(code, resp) => {
            let body: String = resp
                .into_string()
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect();
            match code {
                409 => ApiError::LeaseLost,
                404 => ApiError::NotFound,
                401 | 403 => ApiError::Unauthorized,
                408 | 429 | 500..=599 => ApiError::Transient(format!("HTTP {code} {body}")),
                _ => ApiError::Rejected(format!("HTTP {code} {body}")),
            }
        }
        ureq::Error::Transport(t) => ApiError::Transient(t.to_string()),
    }
}

impl Client {
    pub fn new(base: &str, timeout: Duration) -> Client {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(timeout.min(Duration::from_secs(10)))
            .redirects(0) // never replay a bearer token to wherever a redirect points
            .user_agent(concat!("ibc-worker/", env!("CARGO_PKG_VERSION")))
            .build();
        Client {
            agent,
            base: base.trim_end_matches('/').to_string(),
            token: RwLock::new(String::new()),
            timeout,
        }
    }

    pub fn set_token(&self, token: &str) {
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = token.to_string();
    }

    fn request(&self, method: &str, path: &str, timeout: Duration) -> ureq::Request {
        let token = self.token.read().unwrap_or_else(|e| e.into_inner());
        let mut req = self
            .agent
            .request(method, &format!("{}{path}", self.base))
            .timeout(timeout);
        if !token.is_empty() {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
        req
    }

    fn post(&self, path: &str, body: &str, timeout: Duration) -> Result<Value, ApiError> {
        let resp = sent(
            self.request("POST", path, timeout)
                .set("Content-Type", "application/json")
                .send_string(body),
        )?;
        let text = resp
            .into_string()
            .map_err(|e| ApiError::Transient(format!("reading response: {e}")))?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| ApiError::Rejected(format!("response is not JSON: {e}")))
    }

    /// Returns `(worker_id, token)`. Sent without a bearer token.
    pub fn register(
        &self,
        registration_token: &str,
        name: &str,
        labels: &[String],
        cores: usize,
    ) -> Result<(String, String), ApiError> {
        let body = json!({"registration_token": registration_token, "name": name, "labels": labels, "cores": cores});
        let request = self
            .agent
            .post(&format!("{}/v1/workers/register", self.base))
            .timeout(self.timeout);
        let resp = sent(
            request
                .set("Content-Type", "application/json")
                .send_string(&body.to_string()),
        )?;
        let v: Value = serde_json::from_str(
            &resp
                .into_string()
                .map_err(|e| ApiError::Transient(e.to_string()))?,
        )
        .map_err(|e| ApiError::Rejected(format!("register response is not JSON: {e}")))?;
        let field = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        match (field("worker_id"), field("token")) {
            (Some(id), Some(token)) => Ok((id, token)),
            _ => Err(ApiError::Rejected(
                "register response lacks worker_id/token".into(),
            )),
        }
    }

    /// Long poll for up to `max` jobs. Jobs stay raw JSON so one malformed job cannot hide the others.
    pub fn lease(&self, max: usize, wait_ms: u64) -> Result<Vec<Value>, ApiError> {
        let timeout = Duration::from_millis(wait_ms.min(25_000)) + self.timeout;
        let v = self.post(
            "/v1/jobs/lease",
            &json!({"max": max, "wait_ms": wait_ms}).to_string(),
            timeout,
        )?;
        match v.get("jobs").and_then(Value::as_array) {
            Some(jobs) => Ok(jobs.clone()),
            None => Err(ApiError::Rejected("lease response lacks `jobs`".into())),
        }
    }

    /// Returns the response (`lease_expires_at`, and `lease_secs` from a coordinator that sends it).
    pub fn heartbeat(&self, job_id: &str, lease_token: &str) -> Result<Value, ApiError> {
        let path = format!("/v1/jobs/{}/heartbeat", segment(job_id));
        self.post(
            &path,
            &json!({"lease_token": lease_token}).to_string(),
            self.timeout.min(Duration::from_secs(10)),
        )
    }

    pub fn complete(&self, job_id: &str, body: &CompleteBody<'_>) -> Result<(), ApiError> {
        let text = serde_json::to_string(body)
            .map_err(|e| ApiError::Rejected(format!("encoding result: {e}")))?;
        self.post(
            &format!("/v1/jobs/{}/complete", segment(job_id)),
            &text,
            self.timeout,
        )
        .map(drop)
    }

    /// `code`: `Some("timeout")` when the match ran past the wall-clock limit (docs/JOBS.md).
    pub fn fail(
        &self,
        job_id: &str,
        lease_token: &str,
        error: &str,
        retryable: bool,
        code: Option<&str>,
    ) -> Result<(), ApiError> {
        let mut body = json!({"lease_token": lease_token, "error": error, "retryable": retryable});
        if let Some(code) = code {
            body["code"] = json!(code);
        }
        self.post(
            &format!("/v1/jobs/{}/fail", segment(job_id)),
            &body.to_string(),
            self.timeout,
        )
        .map(drop)
    }

    /// Give a job back without spending one of its attempts ("not for me").
    pub fn release(&self, job_id: &str, lease_token: &str, reason: &str) -> Result<(), ApiError> {
        let body = json!({"lease_token": lease_token, "reason": reason});
        self.post(
            &format!("/v1/jobs/{}/release", segment(job_id)),
            &body.to_string(),
            self.timeout,
        )
        .map(drop)
    }

    /// Stream a blob into `dest` (the caller verifies its hash). More than `max_bytes` is refused.
    pub fn get_blob(&self, sha256: &str, dest: &Path, max_bytes: u64) -> Result<(), ApiError> {
        let resp = sent(
            self.request("GET", &format!("/v1/blobs/{sha256}"), self.timeout)
                .call(),
        )?;
        // A local disk problem is not something to retry against the coordinator.
        let local =
            |e: std::io::Error| ApiError::Rejected(format!("writing {}: {e}", dest.display()));
        let mut file = std::fs::File::create(dest).map_err(local)?;
        let mut body = resp.into_reader().take(max_bytes + 1);
        let mut buf = [0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = body
                .read(&mut buf)
                .map_err(|e| ApiError::Transient(format!("downloading blob: {e}")))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > max_bytes {
                return Err(ApiError::Rejected(format!(
                    "blob is larger than {max_bytes} bytes"
                )));
            }
            file.write_all(&buf[..n]).map_err(local)?;
        }
        file.flush().map_err(local)
    }

    pub fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), ApiError> {
        sent(
            self.request("PUT", &format!("/v1/blobs/{sha256}"), self.timeout)
                .set("Content-Type", "application/octet-stream")
                .send_bytes(bytes),
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn job_ids_are_escaped_in_paths() {
        assert_eq!(super::segment("a/b c?x#"), "a%2Fb%20c%3Fx%23");
        assert_eq!(super::segment("job-1_2.3~"), "job-1_2.3~");
    }
}
