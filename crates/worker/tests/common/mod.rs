//! A mock coordinator: an in-process HTTP server that implements the endpoints of docs/JOBS.md part B over
//! a scripted job queue, with switches for the failures a worker must survive. Shared by the integration
//! tests and by `examples/mock-coordinator.rs`.
#![allow(dead_code)]

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct State {
    pub registration_token: String,
    pub tokens: HashSet<String>,
    pub registrations: u32,
    /// Jobs waiting for a lease: the `Job` object of the protocol without its lease fields.
    pub queue: VecDeque<Value>,
    /// job_id -> (lease token, expiry)
    pub leased: HashMap<String, (String, Instant)>,
    /// Lease calls received (a long poll counts when it starts).
    pub polls: u32,
    pub lease_secs: u64,
    pub blobs: HashMap<String, Vec<u8>>,
    /// Blobs served with wrong content.
    pub corrupt: HashSet<String>,
    /// Every blob download takes this long.
    pub blob_delay: Duration,
    pub blob_gets: u32,
    /// The SHA-256 asked for by every blob download, in order.
    pub blob_log: Vec<String>,
    pub heartbeats: HashMap<String, u32>,
    /// Answer every heartbeat with 409, as if the job had been reassigned.
    pub heartbeat_conflict: bool,
    /// Answer everything with 503.
    pub unavailable: bool,
    /// Answer the next N `complete` calls with 503.
    pub fail_completes: u32,
    pub completed: Vec<(String, Value)>,
    /// (job_id, error, retryable)
    pub failed: Vec<(String, String, bool)>,
    /// job_id -> `code` of its `fail`, when one was sent.
    pub fail_codes: HashMap<String, String>,
    /// (job_id, reason) of `release` calls. A released job is not queued again (the scripts here do not
    /// need it; a real coordinator re-queues it and keeps it from this worker for a while).
    pub released: Vec<(String, String)>,
    /// Answer `release` with 404, as a coordinator that predates it does.
    pub no_release: bool,
    pub first_lease: Option<Instant>,
    pub last_report: Option<Instant>,
    seq: u64,
}

pub struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub struct Mock {
    pub url: String,
    shared: Arc<Shared>,
    server: Arc<tiny_http::Server>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    ibc_worker::sha256_hex(bytes)
}

type Reply = (u16, Vec<u8>);

fn ok(v: Value) -> Reply {
    (200, v.to_string().into_bytes())
}

fn err(status: u16, code: &str) -> Reply {
    (
        status,
        json!({"error": code, "message": code})
            .to_string()
            .into_bytes(),
    )
}

impl Mock {
    /// Listen on 127.0.0.1 (`port` 0: any free port).
    pub fn start(port: u16, registration_token: &str) -> Mock {
        let server =
            Arc::new(tiny_http::Server::http(("127.0.0.1", port)).expect("mock coordinator: bind"));
        let port = server.server_addr().to_ip().expect("ip listener").port();
        let state = State {
            registration_token: registration_token.to_string(),
            lease_secs: 180,
            ..State::default()
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            changed: Condvar::new(),
        });
        let (srv, sh) = (server.clone(), shared.clone());
        std::thread::spawn(move || {
            for request in srv.incoming_requests() {
                let sh = sh.clone();
                std::thread::spawn(move || serve(&sh, request));
            }
        });
        Mock {
            url: format!("http://127.0.0.1:{port}"),
            shared,
            server,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn add_blob(&self, bytes: &[u8]) -> String {
        let sha = sha256_hex(bytes);
        self.state().blobs.insert(sha.clone(), bytes.to_vec());
        sha
    }

    /// Queue a job; `job` carries everything but the lease fields.
    pub fn push_job(&self, job: Value) {
        self.state().queue.push_back(job);
        self.shared.changed.notify_all();
    }

    /// Poll until `pred` holds; panics after `timeout`.
    pub fn wait_for(&self, what: &str, timeout: Duration, pred: impl Fn(&State) -> bool) {
        let end = Instant::now() + timeout;
        while !pred(&self.state()) {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

/// A match job as the coordinator would describe it.
pub fn match_job(
    job_id: &str,
    patch: &Value,
    bot_a: &str,
    bot_b: &str,
    map: &str,
    seed: u64,
) -> Value {
    json!({
        "job_id": job_id, "kind": "match", "patch": patch,
        "bot_a": {"sha256": bot_a, "fuel_mult": 1.0}, "bot_b": {"sha256": bot_b, "fuel_mult": 1.0},
        "map": {"sha256": map}, "seed": seed,
    })
}

fn serve(shared: &Shared, mut request: tiny_http::Request) {
    let mut body = Vec::new();
    let _ = request.as_reader().read_to_end(&mut body);
    let method = request.method().as_str().to_string();
    let path = request.url().split('?').next().unwrap_or("").to_string();
    let bearer = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_string));
    let (status, bytes) = route(shared, &method, &path, bearer.as_deref(), &body);
    let kind = if method == "GET" && status == 200 {
        "application/octet-stream"
    } else {
        "application/json"
    };
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes())
        .expect("static header");
    let _ = request.respond(
        tiny_http::Response::from_data(bytes)
            .with_status_code(status)
            .with_header(header),
    );
}

fn route(shared: &Shared, method: &str, path: &str, bearer: Option<&str>, body: &[u8]) -> Reply {
    let lock = || shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if lock().unavailable {
        return err(503, "unavailable");
    }
    let json_body = || serde_json::from_slice::<Value>(body).ok();
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if (method, parts.as_slice()) == ("POST", &["v1", "workers", "register"][..]) {
        let Some(b) = json_body() else {
            return err(400, "malformed");
        };
        let mut st = lock();
        if b["registration_token"].as_str() != Some(st.registration_token.as_str()) {
            return err(401, "bad_registration_token");
        }
        if !(b["name"].is_string() && b["labels"].is_array() && b["cores"].is_u64()) {
            return err(400, "malformed");
        }
        st.registrations += 1;
        let (id, token) = (
            format!("w{}", st.registrations),
            format!("worker-token-{}", st.registrations),
        );
        st.tokens.insert(token.clone());
        return ok(json!({"worker_id": id, "token": token}));
    }
    if !bearer.is_some_and(|t| lock().tokens.contains(t)) {
        return err(401, "bad_token");
    }
    match (method, parts.as_slice()) {
        ("POST", ["v1", "jobs", "lease"]) => match json_body() {
            Some(b) => lease(
                shared,
                b["max"].as_u64().unwrap_or(1) as usize,
                b["wait_ms"].as_u64().unwrap_or(0),
            ),
            None => err(400, "malformed"),
        },
        ("POST", ["v1", "jobs", job_id, action]) => match json_body() {
            Some(b) => job_action(&mut lock(), job_id, action, &b),
            None => err(400, "malformed"),
        },
        ("GET", ["v1", "blobs", sha]) => {
            let delay = lock().blob_delay;
            std::thread::sleep(delay);
            let mut st = lock();
            st.blob_gets += 1;
            st.blob_log.push(sha.to_string());
            match st.blobs.get(*sha) {
                Some(_) if st.corrupt.contains(*sha) => {
                    (200, b"these are not the bytes you asked for".to_vec())
                }
                Some(bytes) => (200, bytes.clone()),
                None => err(404, "unknown_blob"),
            }
        }
        ("PUT", ["v1", "blobs", sha]) => {
            if sha256_hex(body) != *sha {
                return err(400, "hash_mismatch");
            }
            match lock().blobs.insert(sha.to_string(), body.to_vec()) {
                Some(_) => ok(json!({})),
                None => (201, b"{}".to_vec()),
            }
        }
        _ => err(404, "no_such_endpoint"),
    }
}

/// Long poll: answer as soon as a job is queued, or with no jobs after `min(wait_ms, 25000)`.
fn lease(shared: &Shared, max: usize, wait_ms: u64) -> Reply {
    let end = Instant::now() + Duration::from_millis(wait_ms.min(25_000));
    let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    st.polls += 1;
    shared.changed.notify_all();
    loop {
        // Expired leases are forgotten (a real coordinator re-queues the job; the scripts here do not need it).
        let now = Instant::now();
        let expired: Vec<String> = st
            .leased
            .iter()
            .filter(|(_, (_, exp))| *exp <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            st.leased.remove(&id);
        }
        if !st.queue.is_empty() || now >= end {
            break;
        }
        st = shared
            .changed
            .wait_timeout(st, (end - now).min(Duration::from_millis(50)))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    let mut jobs = Vec::new();
    while jobs.len() < max.max(1) {
        let Some(mut job) = st.queue.pop_front() else {
            break;
        };
        st.seq += 1;
        let lease_token = format!("lease-{}", st.seq);
        let lease_secs = st.lease_secs;
        job["lease_token"] = json!(lease_token);
        job["lease_expires_at"] = json!(ibc_worker::log::rfc3339(
            ibc_worker::log::unix_now() + lease_secs
        ));
        job["lease_secs"] = json!(lease_secs);
        let id = job["job_id"]
            .as_str()
            .expect("scripted job has a job_id")
            .to_string();
        st.leased.insert(
            id,
            (
                lease_token,
                Instant::now() + Duration::from_secs(lease_secs),
            ),
        );
        st.first_lease.get_or_insert_with(Instant::now);
        jobs.push(job);
    }
    ok(json!({"jobs": jobs}))
}

fn job_action(st: &mut State, job_id: &str, action: &str, body: &Value) -> Reply {
    let Some(token) = body["lease_token"].as_str() else {
        return err(400, "malformed");
    };
    let done = st.completed.iter().any(|(id, _)| id == job_id)
        || st.failed.iter().any(|(id, _, _)| id == job_id)
        || st.released.iter().any(|(id, _)| id == job_id);
    let holds = st
        .leased
        .get(job_id)
        .is_some_and(|(t, exp)| t == token && *exp > Instant::now());
    match action {
        "heartbeat" => {
            *st.heartbeats.entry(job_id.to_string()).or_default() += 1;
            if st.heartbeat_conflict || !holds {
                return err(409, "lease_lost");
            }
            let lease_secs = st.lease_secs;
            st.leased.get_mut(job_id).expect("held").1 =
                Instant::now() + Duration::from_secs(lease_secs);
            ok(
                json!({"lease_expires_at": ibc_worker::log::rfc3339(ibc_worker::log::unix_now() + lease_secs),
                       "lease_secs": lease_secs}),
            )
        }
        "release" if st.no_release => err(404, "no_such_endpoint"),
        // Idempotent for the same lease token.
        "complete" | "fail" | "release"
            if done && st.leased.get(job_id).is_some_and(|(t, _)| t == token) =>
        {
            ok(json!({}))
        }
        "complete" | "fail" | "release" if !holds => err(409, "lease_lost"),
        "release" => {
            let reason = body["reason"].as_str().unwrap_or_default().to_string();
            st.released.push((job_id.to_string(), reason));
            st.last_report = Some(Instant::now());
            ok(json!({}))
        }
        "complete" => {
            if st.fail_completes > 0 {
                st.fail_completes -= 1;
                return err(503, "try_again");
            }
            let result = &body["result"];
            let Some(sha) = result["replay_sha256"].as_str() else {
                return err(400, "malformed");
            };
            if !st.blobs.contains_key(sha) {
                return err(400, "replay_blob_absent");
            }
            if !(result["summary"].is_object()
                && result["score"].is_array()
                && result["ticks"].is_u64()
                && result["outcome"].is_string())
            {
                return err(400, "malformed");
            }
            st.completed.push((job_id.to_string(), result.clone()));
            st.last_report = Some(Instant::now());
            ok(json!({}))
        }
        "fail" => {
            let (Some(error), Some(retryable)) =
                (body["error"].as_str(), body["retryable"].as_bool())
            else {
                return err(400, "malformed");
            };
            st.failed
                .push((job_id.to_string(), error.to_string(), retryable));
            if let Some(code) = body["code"].as_str() {
                st.fail_codes.insert(job_id.to_string(), code.to_string());
            }
            st.last_report = Some(Instant::now());
            ok(json!({}))
        }
        _ => err(404, "no_such_endpoint"),
    }
}
