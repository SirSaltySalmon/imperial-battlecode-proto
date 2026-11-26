//! End to end: the real coordinator (in-process, real HTTP) and real `ibc-worker` processes running real
//! matches with the real runner. One worker is killed mid-run; its leases expire and the other finishes.
//!
//! Needs `IBC_RUNNER=/path/to/ibc` (a built `ibc` binary), like `tests/worker.rs`; skipped without it.

use ibc_coordinator::config::Config;
use ibc_coordinator::App;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ADMIN: &str = "e2e-admin";
const REG: &str = "e2e-reg";
const TRUSTED_REG: &str = "e2e-trusted";

struct Coordinator {
    url: String,
    dir: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Coordinator {
    fn start(tag: &str) -> Coordinator {
        let dir = std::env::temp_dir().join(format!("ibc-e2e-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = Config {
            admin_token: ADMIN.into(),
            registration_token: REG.into(),
            trusted_registration_token: Some(TRUSTED_REG.into()),
            // Short leases, so a killed worker's jobs come back within the test.
            lease_ms: 3_000,
            sweep_ms: 200,
            retry_grace_ms: 1_000,
            verify_fraction: 0.25,
            // The test queues its own ranked series.
            ranked_interval_ms: 0,
            ..Config::default()
        };
        let app = App::open(cfg, &dir.join("data")).expect("coordinator data dir");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            runtime
                .block_on(ibc_coordinator::serve(app, listener, async {
                    let _ = stopped.await;
                }))
                .unwrap();
            runtime.shutdown_timeout(Duration::from_secs(1));
        });
        Coordinator {
            url,
            dir,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&[u8]>,
    ) -> (u16, Vec<u8>) {
        let mut req = ureq::request(method, &format!("{}{path}", self.url));
        if let Some(t) = token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        let res = match body {
            Some(b) => req.send_bytes(b),
            None => req.call(),
        };
        let res = match res {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(e) => panic!("{method} {path}: {e}"),
        };
        let status = res.status();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut res.into_reader(), &mut bytes).unwrap();
        (status, bytes)
    }

    fn json(&self, method: &str, path: &str, token: Option<&str>, body: Value) -> Value {
        let body = (!body.is_null()).then(|| body.to_string().into_bytes());
        let (status, out) = self.call(method, path, token, body.as_deref());
        assert!(
            (200..300).contains(&status),
            "{method} {path} -> {status}: {}",
            String::from_utf8_lossy(&out)
        );
        serde_json::from_slice(&out).unwrap_or(Value::Null)
    }

    fn admin(&self, method: &str, path: &str, body: Value) -> Value {
        self.json(method, path, Some(ADMIN), body)
    }

    fn metric(&self, name: &str) -> f64 {
        let (_, text) = self.call("GET", "/metrics", None, None);
        String::from_utf8(text)
            .unwrap()
            .lines()
            .find(|l| l.starts_with(name) && l[name.len()..].starts_with(' '))
            .and_then(|l| l.rsplit(' ').next()?.parse().ok())
            .unwrap_or_else(|| panic!("no metric {name}"))
    }

    fn wait(&self, what: &str, timeout: Duration, mut done: impl FnMut(&Coordinator) -> bool) {
        let end = Instant::now() + timeout;
        while !done(self) {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A worker process; killed when dropped.
struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn worker(c: &Coordinator, runner: &Path, name: &str, registration: &str) -> Worker {
    let cache = c.dir.join(name);
    let child = Command::new(env!("CARGO_BIN_EXE_ibc-worker"))
        .args([
            "--coordinator",
            &c.url,
            "--registration-token",
            registration,
            "--name",
            name,
        ])
        .args(["--concurrency", "2", "--poll-ms", "500"])
        .arg("--runner")
        .arg(runner)
        .arg("--cache")
        .arg(&cache)
        .arg("--drain-file")
        .arg(cache.with_extension("drain"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(cache.with_extension("log")).unwrap())
        .spawn()
        .expect("starting ibc-worker");
    Worker(child)
}

fn build_bot(runner: &Path, root: &Path, bot: &str) -> Vec<u8> {
    let dir = root.join("bots").join(bot);
    let out = Command::new(runner)
        .arg("build")
        .arg(&dir)
        .output()
        .expect("ibc build");
    assert!(
        out.status.success(),
        "ibc build {bot}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read(dir.join(".ibc/bot.wasm")).expect("built bot.wasm")
}

#[test]
fn real_coordinator_and_workers_play_a_ranked_league_and_survive_a_killed_worker() {
    let Some(runner) = std::env::var_os("IBC_RUNNER")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    else {
        eprintln!("SKIPPED: set IBC_RUNNER to a built `ibc` binary to run the end-to-end test");
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let c = Coordinator::start("league");

    // The league: the runner's patch, three maps, four teams (two of them with the same bot).
    let version: Value = serde_json::from_slice(
        &Command::new(&runner)
            .args(["version", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let patch = json!({"engine": version["engine"], "ruleset_hash": version["ruleset_hash"], "wasmtime": version["wasmtime"]});
    c.admin("POST", "/v1/admin/patch", patch.clone());
    for map in ["crossfire", "tunnels", "agora"] {
        let bytes = std::fs::read(root.join("maps").join(format!("{map}.ibcmap"))).unwrap();
        let (status, _) = c.call(
            "PUT",
            &format!("/v1/admin/maps/{map}"),
            Some(ADMIN),
            Some(&bytes),
        );
        assert_eq!(status, 200);
    }
    let teams = [
        ("alpha", "rusher"),
        ("beta", "holder"),
        ("gamma", "idle"),
        ("delta", "rusher"),
    ];
    for (team, bot) in teams {
        let t = c.admin("POST", "/v1/admin/teams", json!({"name": team}));
        let wasm = build_bot(&runner, &root, bot);
        let (status, out) = c.call("POST", "/v1/submissions", t["token"].as_str(), Some(&wasm));
        assert_eq!(status, 201, "{}", String::from_utf8_lossy(&out));
    }
    // A series of six games for every pair (later, one round of the matchmaker: a pair meets at most twice
    // a day).
    let names: Vec<&str> = teams.iter().map(|(t, _)| *t).collect();
    let mut pairs = Vec::new();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            pairs.push(json!([a, b]));
        }
    }
    let mut series: Vec<Value> = c.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": pairs, "games": 6}),
    )["series"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(series.len(), 6);

    // A trusted worker and an untrusted one; the untrusted one is killed (no drain, no goodbye) while it
    // holds leases.
    let started = Instant::now();
    let trusted = worker(&c, &runner, "trusted", TRUSTED_REG);
    let mut doomed = worker(&c, &runner, "doomed", REG);
    let held = |c: &Coordinator| {
        c.admin("GET", "/v1/admin/workers", Value::Null)["workers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["name"] == "doomed" && w["leased_jobs"].as_i64() > Some(0))
    };
    c.wait(
        "the doomed worker to hold a lease",
        Duration::from_secs(60),
        held,
    );
    doomed.0.kill().unwrap();
    doomed.0.wait().unwrap();

    let drained = |c: &Coordinator| {
        let queued: f64 = ["tournament", "ranked", "unranked", "bench"]
            .iter()
            .map(|class| c.metric(&format!("ibc_queue_depth{{class=\"{class}\"}}")))
            .sum();
        queued == 0.0 && c.metric("ibc_jobs_leased") == 0.0 && c.metric("ibc_verify_pending") == 0.0
    };
    c.wait("every job to finish", Duration::from_secs(180), drained);
    eprintln!(
        "e2e: {} results in {:.1} s (including one 3 s lease expiry), {} leases expired",
        c.metric("ibc_jobs_completed_total"),
        started.elapsed().as_secs_f64(),
        c.metric("ibc_leases_expired_total")
    );
    // Then a round of the matchmaker: four free teams, two series.
    let round = c.admin("POST", "/v1/admin/ranked-batch", json!({}))["series"].clone();
    assert_eq!(round.as_array().unwrap().len(), 2, "{round}");
    series.extend(round.as_array().unwrap().iter().cloned());
    c.wait("the matchmaker's series", Duration::from_secs(60), drained);

    // The killed worker's leases ran out and someone else finished those jobs.
    assert!(c.metric("ibc_leases_expired_total") >= 1.0);
    assert_eq!(c.metric("ibc_jobs_dead"), 0.0);
    // Every series is closed and rated; every game has a downloadable replay that hashes to its record.
    for id in &series {
        let s = c.json(
            "GET",
            &format!("/v1/series/{}", id.as_str().unwrap()),
            None,
            Value::Null,
        );
        assert_eq!(
            (s["state"].as_str(), s["rated"].as_bool()),
            (Some("done"), Some(true)),
            "{s}"
        );
        for m in s["matches"].as_array().unwrap() {
            assert!(
                ["a", "b", "draw"].contains(&m["outcome"].as_str().unwrap()),
                "{m}"
            );
            let (status, replay) = c.call("GET", m["replay_url"].as_str().unwrap(), None, None);
            assert_eq!(status, 200);
            assert_eq!(
                &ibc_worker::sha256_hex(&replay),
                m["replay_sha256"].as_str().unwrap()
            );
        }
    }
    // The ladder moved: everyone played, and the rusher beat the idle bot.
    let ladder = c.json("GET", "/v1/ladder", None, Value::Null);
    let row = |name: &str| {
        ladder
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["team_name"] == name)
            .unwrap()
            .clone()
    };
    assert!(
        names.iter().all(|n| row(n)["games"].as_i64() >= Some(6)),
        "{ladder}"
    );
    // Few series each: still provisional, so compare places, not (hidden) ratings.
    assert!(
        row("alpha")["rank"].as_i64() < row("gamma")["rank"].as_i64(),
        "{ladder}"
    );

    // The trusted worker drains on its drain file and exits cleanly.
    let mut trusted = trusted;
    std::fs::write(c.dir.join("trusted").with_extension("drain"), b"done").unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = trusted.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < end, "the trusted worker did not drain");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success());
}
