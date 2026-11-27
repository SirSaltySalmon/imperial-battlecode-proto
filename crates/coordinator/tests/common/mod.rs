//! Test harness: the real router over a temporary data directory, driven in-process (no sockets), and a
//! mock worker that speaks protocol v1 but "plays" a job by deriving a fake result from the job's inputs.
#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use ibc_coordinator::config::Config;
use ibc_coordinator::util::{random_hex, sha256_hex};
use ibc_coordinator::App;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

pub const ADMIN: &str = "admin-secret";
pub const REG: &str = "reg-secret";
pub const TRUSTED_REG: &str = "trusted-reg-secret";

pub struct Harness {
    pub app: Arc<App>,
    router: axum::Router,
    dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Harness {
    /// Defaults suited to tests: no spot verification, no retry grace, no aging, no background tasks
    /// (tests call `app.sweep()` themselves).
    pub fn new(tune: impl FnOnce(&mut Config)) -> Harness {
        let mut cfg = Config {
            admin_token: ADMIN.into(),
            registration_token: REG.into(),
            trusted_registration_token: Some(TRUSTED_REG.into()),
            verify_fraction: 0.0,
            retry_grace_ms: 0,
            aging_ms: 0,
            ..Config::default()
        };
        tune(&mut cfg);
        let dir = std::env::temp_dir().join(format!("ibc-coordinator-test-{}", random_hex(8)));
        let app = App::open(cfg, &dir).expect("open data dir");
        Harness {
            router: ibc_coordinator::api::router(app.clone()),
            app,
            dir,
        }
    }

    /// The data directory (`coordinator.sqlite`, `blobs/`).
    pub fn data_dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Close the app and open the same data directory again with a changed configuration.
    pub fn reopen(&mut self, tune: impl FnOnce(&mut Config)) {
        let mut cfg = self.app.cfg.clone();
        tune(&mut cfg);
        self.app = App::open(cfg, &self.dir).expect("reopen data dir");
        self.router = ibc_coordinator::api::router(self.app.clone());
    }

    pub async fn raw(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Vec<u8>,
    ) -> (StatusCode, Vec<u8>) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let res = self
            .router
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = res.status();
        (
            status,
            res.into_body().collect().await.unwrap().to_bytes().to_vec(),
        )
    }

    /// JSON in (or no body for `Value::Null`), JSON out.
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let bytes = if body.is_null() {
            Vec::new()
        } else {
            body.to_string().into_bytes()
        };
        let (status, out) = self.raw(method, path, token, bytes).await;
        (status, serde_json::from_slice(&out).unwrap_or(Value::Null))
    }

    pub async fn ok(&self, method: &str, path: &str, token: Option<&str>, body: Value) -> Value {
        let (status, out) = self.call(method, path, token, body).await;
        assert!(status.is_success(), "{method} {path} -> {status}: {out}");
        out
    }

    pub async fn admin(&self, method: &str, path: &str, body: Value) -> Value {
        self.ok(method, path, Some(ADMIN), body).await
    }

    /// Store a blob as the organisers; returns its hash.
    pub async fn blob(&self, bytes: &[u8]) -> String {
        let sha = sha256_hex(bytes);
        let (status, _) = self
            .raw(
                "PUT",
                &format!("/v1/blobs/{sha}"),
                Some(ADMIN),
                bytes.to_vec(),
            )
            .await;
        assert!(status.is_success());
        sha
    }

    pub async fn set_patch(&self) {
        self.admin(
            "POST",
            "/v1/admin/patch",
            json!({"engine": "0.1.0", "ruleset_hash": "00112233aabbccdd", "wasmtime": "51.0.0"}),
        )
        .await;
    }

    /// Queue a stand-alone match between two fresh fake blobs; returns the enqueue response.
    pub async fn enqueue(&self, class: &str, seed: u64) -> Value {
        let (a, b, map) = (
            self.blob(b"bot a").await,
            self.blob(b"bot b").await,
            self.blob(b"a map").await,
        );
        self.admin(
            "POST",
            "/v1/admin/jobs",
            json!({"class": class, "bot_a": {"sha256": a, "fuel_mult": 1.0}, "bot_b": {"sha256": b}, "map": {"sha256": map}, "seed": seed}),
        )
        .await
    }

    /// Create a team with a valid live submission; returns `(team_id, team_token)`.
    pub async fn team(&self, name: &str) -> (String, String) {
        let team = self
            .admin("POST", "/v1/admin/teams", json!({"name": name}))
            .await;
        let token = team["token"].as_str().unwrap().to_string();
        let (status, out) = self
            .raw("POST", "/v1/submissions", Some(&token), bot_wasm(name))
            .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&out)
        );
        (team["team_id"].as_str().unwrap().to_string(), token)
    }

    /// Patch, one pool map and `names.len()` teams with live submissions.
    pub async fn league(&self, names: &[&str]) -> Vec<(String, String)> {
        self.set_patch().await;
        let (status, _) = self
            .raw(
                "PUT",
                "/v1/admin/maps/arena",
                Some(ADMIN),
                b"name: arena\n---\n###\n".to_vec(),
            )
            .await;
        assert!(status.is_success());
        let mut teams = Vec::new();
        for name in names {
            teams.push(self.team(name).await);
        }
        teams
    }

    /// The ladder's row for a team, by name.
    pub async fn standing(&self, name: &str) -> Value {
        let ladder = self.ok("GET", "/v1/ladder", None, Value::Null).await;
        ladder
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["team_name"] == name)
            .unwrap_or_else(|| panic!("{name} is not on the ladder: {ladder}"))
            .clone()
    }

    /// The stored rating of a team, by name (the ladder shows only the displayed number).
    pub fn rating(&self, name: &str) -> ibc_coordinator::db::ratings::Standing {
        self.app
            .db
            .tx(|tx| {
                let team = ibc_coordinator::db::find_team(tx, name)?;
                ibc_coordinator::db::ratings::load(tx, self.app.rating.as_ref(), &team.id)
            })
            .unwrap()
    }

    pub async fn metric(&self, name: &str) -> f64 {
        let (_, body) = self.raw("GET", "/metrics", None, Vec::new()).await;
        let text = String::from_utf8(body).unwrap();
        text.lines()
            .find(|l| l.starts_with(name) && l[name.len()..].starts_with(' '))
            .and_then(|l| l.rsplit(' ').next()?.parse().ok())
            .unwrap_or_else(|| panic!("no metric {name} in:\n{text}"))
    }
}

/// A minimal valid bot; `tag` makes the module (and so its hash) unique.
pub fn bot_wasm(tag: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
             (import "ibc" "end_turn" (func $end_turn))
             (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
             (memory (export "memory") 1)
             (func (export "_start") (call $end_turn))
             (data (i32.const 0) "{tag}"))"#
    ))
    .unwrap()
}

/// The fake match: the replay is a pure function of the job's inputs, and the bot whose module hash is
/// lexicographically smaller wins 7-3. A cheating worker returns a different replay and the other winner.
pub fn fake_result(job: &Value, cheat: bool) -> (Vec<u8>, Value) {
    let (a, b) = (
        job["bot_a"]["sha256"].as_str().unwrap(),
        job["bot_b"]["sha256"].as_str().unwrap(),
    );
    let mut replay = format!(
        "replay|{a}|{b}|{}|{}",
        job["map"]["sha256"].as_str().unwrap(),
        job["seed"]
    )
    .into_bytes();
    let mut winner = if a < b { 0 } else { 1 };
    if cheat {
        replay.extend_from_slice(b"|tampered");
        winner = 1 - winner;
    }
    let sha = sha256_hex(&replay);
    let score = if winner == 0 { [7, 3] } else { [3, 7] };
    let outcome = if winner == 0 { "a" } else { "b" };
    let result = json!({"winner": winner, "outcome": outcome, "score": score, "ticks": 100, "replay_sha256": sha,
                        "wall_ms": 40, "summary": {"outcome": outcome, "replay_sha256": sha}});
    (replay, result)
}

pub struct MockWorker {
    pub id: String,
    pub token: String,
    pub cheat: bool,
}

impl MockWorker {
    pub async fn register(h: &Harness, name: &str, trusted: bool) -> MockWorker {
        let reg = if trusted { TRUSTED_REG } else { REG };
        let out = h.ok("POST", "/v1/workers/register", None, json!({"registration_token": reg, "name": name, "labels": ["x86_64-linux"], "cores": 4})).await;
        MockWorker {
            id: out["worker_id"].as_str().unwrap().into(),
            token: out["token"].as_str().unwrap().into(),
            cheat: false,
        }
    }

    pub async fn lease(&self, h: &Harness, max: u32, wait_ms: u32) -> Vec<Value> {
        let out = h
            .ok(
                "POST",
                "/v1/jobs/lease",
                Some(&self.token),
                json!({"max": max, "wait_ms": wait_ms}),
            )
            .await;
        out["jobs"].as_array().unwrap().clone()
    }

    pub async fn lease_one(&self, h: &Harness) -> Value {
        let mut jobs = self.lease(h, 1, 0).await;
        assert_eq!(jobs.len(), 1, "expected exactly one job");
        jobs.remove(0)
    }

    async fn job_call(
        &self,
        h: &Harness,
        job: &Value,
        action: &str,
        mut body: Value,
    ) -> (StatusCode, Value) {
        body["lease_token"] = job["lease_token"].clone();
        h.call(
            "POST",
            &format!("/v1/jobs/{}/{action}", job["job_id"].as_str().unwrap()),
            Some(&self.token),
            body,
        )
        .await
    }

    pub async fn heartbeat(&self, h: &Harness, job: &Value) -> (StatusCode, Value) {
        self.job_call(h, job, "heartbeat", json!({})).await
    }

    /// Upload the fake replay and complete the job. A refused upload (a worker that holds no lease) is
    /// returned as it came, and `complete` is not called.
    pub async fn complete(&self, h: &Harness, job: &Value) -> (StatusCode, Value) {
        let (replay, result) = fake_result(job, self.cheat);
        let sha = result["replay_sha256"].as_str().unwrap();
        let (status, body) = h
            .raw(
                "PUT",
                &format!("/v1/blobs/{sha}"),
                Some(&self.token),
                replay,
            )
            .await;
        if !status.is_success() {
            return (status, serde_json::from_slice(&body).unwrap_or(Value::Null));
        }
        self.job_call(h, job, "complete", json!({"result": result}))
            .await
    }

    pub async fn fail(&self, h: &Harness, job: &Value, retryable: bool) -> (StatusCode, Value) {
        self.job_call(
            h,
            job,
            "fail",
            json!({"error": "mock failure", "retryable": retryable}),
        )
        .await
    }

    /// `fail` as a worker whose match ran past its wall-clock limit.
    pub async fn time_out(&self, h: &Harness, job: &Value) -> (StatusCode, Value) {
        self.job_call(
            h,
            job,
            "fail",
            json!({"error": "timeout: match exceeded 300.0 s of wall clock", "retryable": true, "code": "timeout"}),
        )
        .await
    }

    pub async fn release(&self, h: &Harness, job: &Value, reason: &str) -> (StatusCode, Value) {
        self.job_call(h, job, "release", json!({"reason": reason}))
            .await
    }

    /// Lease and complete until the queue has nothing for this worker; returns how many jobs it played.
    pub async fn drain(&self, h: &Harness) -> usize {
        let mut played = 0;
        loop {
            let jobs = self.lease(h, 8, 0).await;
            if jobs.is_empty() {
                return played;
            }
            for job in &jobs {
                let (status, out) = self.complete(h, job).await;
                assert_eq!(status, StatusCode::OK, "{out}");
                played += 1;
            }
        }
    }
}
