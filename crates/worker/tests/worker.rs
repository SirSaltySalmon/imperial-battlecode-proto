//! The worker against a mock coordinator, with the REAL runner binary and real wasm bots.
//!
//! Needs `IBC_RUNNER=/path/to/ibc` (a built `ibc` binary). Without it every test here is skipped with a
//! message. The reference bots are built to wasm with `ibc build` on first use.

mod common;

use common::{match_job, sha256_hex, Mock};
use ibc_worker::{Config, Shutdown, Stats};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const REG: &str = "registration-secret";

struct Fixture {
    runner: PathBuf,
    patch: Value,
    rusher: Vec<u8>,
    holder: Vec<u8>,
    crossfire: Vec<u8>,
    tunnels: Vec<u8>,
}

/// `None` (after saying why) when the runner binary is not available.
fn fixture() -> Option<&'static Fixture> {
    static F: OnceLock<Option<Fixture>> = OnceLock::new();
    F.get_or_init(|| {
        let Some(runner) = std::env::var_os("IBC_RUNNER").map(PathBuf::from).filter(|p| p.is_file()) else {
            eprintln!("SKIPPED: set IBC_RUNNER to a built `ibc` binary to run the worker integration tests");
            return None;
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let build = |bot: &str| {
            let dir = root.join("bots").join(bot);
            let out = std::process::Command::new(&runner).arg("build").arg(&dir).output().expect("running `ibc build`");
            assert!(out.status.success(), "`ibc build {}` failed:\n{}", dir.display(), String::from_utf8_lossy(&out.stderr));
            std::fs::read(dir.join(".ibc/bot.wasm")).expect("built bot.wasm")
        };
        let map = |name: &str| std::fs::read(root.join("maps").join(name)).expect("map file");
        let patch = serde_json::to_value(ibc_worker::runner::probe_version(&runner).expect("ibc version --json")).unwrap();
        Some(Fixture { patch, rusher: build("rusher"), holder: build("holder"), crossfire: map("crossfire.ibcmap"), tunnels: map("tunnels.ibcmap"), runner })
    })
    .as_ref()
}

/// A mock coordinator holding the reference blobs, and a fast-paced worker config pointed at it.
struct Rig {
    mock: Mock,
    cfg: Config,
    rusher: String,
    holder: String,
    crossfire: String,
    tunnels: String,
}

fn config(f: &Fixture, url: &str, tag: &str) -> Config {
    let dir = std::env::temp_dir().join(format!("ibc-worker-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = Config::new(url, &f.runner, dir);
    cfg.registration_token = Some(REG.into());
    cfg.name = format!("test-{tag}");
    cfg.labels = vec!["test".into()];
    cfg.concurrency = 2;
    cfg.poll_wait_ms = 100;
    cfg.backoff_min = Duration::from_millis(50);
    cfg.backoff_max = Duration::from_millis(300);
    cfg.once = true;
    cfg
}

fn rig(f: &Fixture, tag: &str) -> Rig {
    let mock = Mock::start(0, REG);
    let cfg = config(f, &mock.url, tag);
    let (rusher, holder) = (mock.add_blob(&f.rusher), mock.add_blob(&f.holder));
    let (crossfire, tunnels) = (mock.add_blob(&f.crossfire), mock.add_blob(&f.tunnels));
    Rig {
        mock,
        cfg,
        rusher,
        holder,
        crossfire,
        tunnels,
    }
}

impl Rig {
    fn push(&self, f: &Fixture, id: &str, seed: u64) {
        self.mock.push_job(match_job(
            id,
            &f.patch,
            &self.rusher,
            &self.holder,
            &self.crossfire,
            seed,
        ));
    }

    fn run(&self) -> Stats {
        ibc_worker::run(self.cfg.clone(), Arc::new(Shutdown::default())).expect("worker run")
    }

    fn result(&self, id: &str) -> Value {
        self.mock
            .state()
            .completed
            .iter()
            .find(|(j, _)| j == id)
            .unwrap_or_else(|| panic!("{id} was not completed"))
            .1
            .clone()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.cfg.cache_dir);
    }
}

#[test]
fn happy_path_runs_real_matches_and_identical_jobs_give_identical_replays() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "happy");
    r.push(f, "j1", 7);
    r.push(f, "j2", 7);
    let mut j3 = match_job("j3", &f.patch, &r.holder, &r.rusher, &r.tunnels, u64::MAX);
    j3["max_ticks"] = json!(40);
    j3["some_future_field"] = json!({"ignored": true});
    r.mock.push_job(j3);

    assert_eq!(
        r.run(),
        Stats {
            completed: 3,
            failed: 0,
            released: 0,
            dropped: 0
        }
    );
    let (a, b, c) = (r.result("j1"), r.result("j2"), r.result("j3"));
    assert_eq!(
        a["replay_sha256"], b["replay_sha256"],
        "the same job must give the same replay"
    );
    // Everything but the measured wall time is a function of the job.
    let deterministic = |mut v: Value| {
        assert!(v["wall_ms"].is_u64(), "{v}");
        v.as_object_mut().unwrap().remove("wall_ms");
        v
    };
    assert_eq!(deterministic(a.clone()), deterministic(b.clone()));
    assert_eq!(
        (a["outcome"].as_str(), c["outcome"].as_str()),
        (a["summary"]["outcome"].as_str(), Some("unfinished"))
    );
    assert_ne!(a["replay_sha256"], c["replay_sha256"]);
    assert_eq!(c["ticks"], 40);
    assert_eq!(c["summary"]["outcome"], "unfinished");
    for res in [&a, &c] {
        let sha = res["replay_sha256"].as_str().unwrap();
        let st = r.mock.state();
        // The replay was uploaded before `complete`, and it hashes to what was reported.
        assert_eq!(sha256_hex(&st.blobs[sha]), sha);
        assert_eq!(res["summary"]["replay_sha256"], sha);
        assert_eq!(res["summary"]["winner"], res["winner"]);
    }
    // The seed went through without touching a double.
    assert_eq!(c["summary"]["seed"].as_u64(), Some(u64::MAX));

    // Registered once; the token is persisted (owner-only) and reused; blobs come from the cache.
    let token: Value =
        serde_json::from_str(&std::fs::read_to_string(&r.cfg.token_file).unwrap()).unwrap();
    assert_eq!(token["token"], "worker-token-1");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&r.cfg.token_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let gets = r.mock.state().blob_gets;
    assert_eq!(gets, 4, "each distinct blob is downloaded once");
    r.push(f, "j4", 7);
    let mut again = r.cfg.clone();
    again.registration_token = None;
    assert_eq!(
        ibc_worker::run(again, Arc::new(Shutdown::default()))
            .unwrap()
            .completed,
        1
    );
    assert_eq!(r.result("j4")["replay_sha256"], a["replay_sha256"]);
    let registrations = r.mock.state().registrations;
    assert_eq!((registrations, r.mock.state().blob_gets), (1, gets));
    // Nothing but the cache survives a run.
    assert!(!r.cfg.cache_dir.join("work").exists());
    assert_eq!(
        std::fs::read_dir(r.cfg.cache_dir.join("tmp"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn artifact_with_wrong_hash_is_rejected() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "corrupt");
    r.mock.state().corrupt.insert(r.rusher.clone());
    r.push(f, "j1", 1);
    assert_eq!(
        r.run(),
        Stats {
            completed: 0,
            failed: 1,
            released: 0,
            dropped: 0
        }
    );
    let st = r.mock.state();
    let (_, error, retryable) = &st.failed[0];
    assert!(error.contains("hash mismatch") && *retryable, "{error}");
    assert!(st.completed.is_empty());
    assert!(
        !r.cfg.cache_dir.join("blobs").join(&r.rusher).exists(),
        "a bad download must not enter the cache"
    );
}

/// A missing blob is a bad input (`fail`, not retryable); another patch or an unknown kind is "not for me"
/// (`release`, which spends no attempt).
#[test]
fn missing_blob_is_failed_wrong_patch_and_unknown_kind_are_released() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "refuse");
    r.mock.push_job(match_job(
        "missing",
        &f.patch,
        &r.rusher,
        &"0".repeat(64),
        &r.crossfire,
        1,
    ));
    let mut other = f.patch.clone();
    other["engine"] = json!("9.9.9");
    r.mock.push_job(match_job(
        "patch",
        &other,
        &r.rusher,
        &r.holder,
        &r.crossfire,
        1,
    ));
    r.mock
        .push_job(json!({"job_id": "kind", "kind": "tournament-bracket", "anything": 1}));
    assert_eq!(
        r.run(),
        Stats {
            completed: 0,
            failed: 1,
            released: 2,
            dropped: 0
        }
    );
    let st = r.mock.state();
    assert_eq!(
        st.failed,
        [(
            "missing".to_string(),
            format!("blob {} not found", "0".repeat(64)),
            false
        )]
    );
    let released = |id: &str| {
        st.released
            .iter()
            .find(|(j, _)| j == id)
            .map(|(_, reason)| reason.as_str())
            .unwrap()
    };
    assert_eq!(released("patch"), "patch mismatch");
    assert!(released("kind").contains("unknown job kind"));
}

#[test]
fn a_coordinator_without_release_gets_a_retryable_fail_instead() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "norelease");
    r.mock.state().no_release = true;
    let mut other = f.patch.clone();
    other["engine"] = json!("9.9.9");
    r.mock.push_job(match_job(
        "patch",
        &other,
        &r.rusher,
        &r.holder,
        &r.crossfire,
        1,
    ));
    assert_eq!(
        r.run(),
        Stats {
            completed: 0,
            failed: 1,
            released: 0,
            dropped: 0
        }
    );
    assert_eq!(
        r.mock.state().failed,
        [("patch".to_string(), "patch mismatch".to_string(), true)]
    );
}

#[test]
fn heartbeats_are_sent_while_a_job_is_in_hand() {
    let Some(f) = fixture() else { return };
    let mut r = rig(f, "heartbeat");
    r.cfg.heartbeat_interval = Duration::from_millis(40);
    r.mock.state().blob_delay = Duration::from_millis(150); // three downloads keep the job in hand ~450 ms
    r.push(f, "j1", 3);
    assert_eq!(r.run().completed, 1);
    let beats = r.mock.state().heartbeats["j1"];
    assert!(
        beats >= 4,
        "expected a heartbeat every 40 ms over ~450 ms, saw {beats}"
    );
}

#[test]
fn a_409_on_heartbeat_stops_work_and_reports_nothing() {
    let Some(f) = fixture() else { return };
    let mut r = rig(f, "lost");
    r.cfg.heartbeat_interval = Duration::from_millis(30);
    r.mock.state().blob_delay = Duration::from_millis(200);
    r.mock.state().heartbeat_conflict = true;
    r.push(f, "j1", 3);
    assert_eq!(
        r.run(),
        Stats {
            completed: 0,
            failed: 0,
            released: 0,
            dropped: 1
        }
    );
    let st = r.mock.state();
    assert!(st.completed.is_empty() && st.failed.is_empty());
}

#[test]
fn a_match_over_the_wall_clock_limit_is_killed_and_failed_retryable_with_the_timeout_code() {
    let Some(f) = fixture() else { return };
    let mut r = rig(f, "timeout");
    // Download the blobs first, so the match itself (not a download) runs out of time.
    r.push(f, "warm", 3);
    assert_eq!(r.run().completed, 1);
    r.cfg.job_timeout = Duration::from_millis(1);
    r.push(f, "j1", 3);
    assert_eq!(
        r.run(),
        Stats {
            completed: 0,
            failed: 1,
            released: 0,
            dropped: 0
        }
    );
    let st = r.mock.state();
    assert!(
        st.failed[0].1.starts_with("timeout: match exceeded") && st.failed[0].2,
        "{:?}",
        st.failed[0]
    );
    assert_eq!(st.fail_codes["j1"], "timeout");
}

#[test]
fn a_runner_failed_event_becomes_a_fail_and_the_next_job_still_runs() {
    let Some(f) = fixture() else { return };
    let mut r = rig(f, "badwasm");
    r.cfg.concurrency = 1;
    let junk = r.mock.add_blob(b"this is not a wasm module");
    r.mock.push_job(match_job(
        "bad",
        &f.patch,
        &junk,
        &r.holder,
        &r.crossfire,
        1,
    ));
    r.push(f, "good", 1);
    assert_eq!(
        r.run(),
        Stats {
            completed: 1,
            failed: 1,
            released: 0,
            dropped: 0
        }
    );
    let st = r.mock.state();
    let (id, error, retryable) = &st.failed[0];
    assert_eq!(id, "bad");
    assert!(
        !retryable,
        "an invalid module is a bad input, not an environment problem: {error}"
    );
}

#[test]
fn coordinator_down_then_back_the_worker_backs_off_and_recovers() {
    let Some(f) = fixture() else { return };
    // Reserve a port, then leave nothing listening on it while the worker starts.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cfg = config(f, &format!("http://127.0.0.1:{port}"), "down");
    let cache = cfg.cache_dir.clone();
    let worker = std::thread::spawn(move || ibc_worker::run(cfg, Arc::new(Shutdown::default())));
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        !worker.is_finished(),
        "the worker must keep trying while the coordinator is down"
    );

    let mock = Mock::start(port, REG);
    let ids = [&f.rusher, &f.holder, &f.crossfire].map(|b| mock.add_blob(b));
    // Come back flaky: the first two `complete` calls get a 503 and must be retried.
    mock.state().fail_completes = 2;
    mock.push_job(match_job("j1", &f.patch, &ids[0], &ids[1], &ids[2], 5));
    assert_eq!(
        worker.join().unwrap().unwrap(),
        Stats {
            completed: 1,
            failed: 0,
            released: 0,
            dropped: 0
        }
    );
    let st = mock.state();
    assert_eq!((st.completed.len(), st.fail_completes), (1, 0));
    drop(st);
    let _ = std::fs::remove_dir_all(cache);
}

#[test]
fn drain_finishes_in_flight_work_and_leases_nothing_more() {
    let Some(f) = fixture() else { return };
    let mut r = rig(f, "drain");
    let drain_file = r.cfg.cache_dir.with_extension("drain");
    let _ = std::fs::remove_file(&drain_file);
    r.cfg.once = false;
    r.cfg.concurrency = 1;
    r.cfg.drain_file = Some(drain_file.clone());
    r.mock.state().blob_delay = Duration::from_millis(200); // the first job is in hand for ~600 ms
    for i in 0..5 {
        r.push(f, &format!("j{i}"), i);
    }
    let cfg = r.cfg.clone();
    let worker = std::thread::spawn(move || ibc_worker::run(cfg, Arc::new(Shutdown::default())));
    r.mock
        .wait_for("the first lease", Duration::from_secs(20), |st| {
            !st.leased.is_empty()
        });
    std::fs::write(&drain_file, b"spot interruption notice").unwrap();
    let stats = worker.join().unwrap().unwrap();
    let _ = std::fs::remove_file(&drain_file);
    assert_eq!(
        stats,
        Stats {
            completed: 1,
            failed: 0,
            released: 0,
            dropped: 0
        }
    );
    let st = r.mock.state();
    assert_eq!(
        (st.completed.len(), st.queue.len()),
        (1, 4),
        "the job in hand is finished, the rest stay queued"
    );
}

/// A drain that begins during a long poll cannot cancel the poll; whatever it brings back has not started
/// and is given back at once, so a machine about to disappear does not start matches it cannot finish.
#[test]
fn a_job_from_a_poll_open_when_the_drain_began_is_released_unstarted() {
    let Some(f) = fixture() else { return };
    let mut r = rig(f, "drainpoll");
    r.cfg.once = false;
    r.cfg.poll_wait_ms = 5_000;
    let (cfg, shutdown) = (r.cfg.clone(), Arc::new(Shutdown::default()));
    let handle = shutdown.clone();
    let worker = std::thread::spawn(move || ibc_worker::run(cfg, handle));
    r.mock
        .wait_for("the first poll", Duration::from_secs(20), |st| st.polls > 0);
    shutdown.request_drain();
    r.push(f, "j1", 1);
    assert_eq!(
        worker.join().unwrap().unwrap(),
        Stats {
            completed: 0,
            failed: 0,
            released: 1,
            dropped: 0
        }
    );
    let st = r.mock.state();
    assert_eq!(st.released, [("j1".to_string(), "worker shutting down".to_string())]);
    assert_eq!(st.blob_gets, 0, "nothing was downloaded for it");
}

/// The real binary and real SIGTERMs, which `docker stop`, systemd and orchestrators relaying a spot reclaim
/// send: the first drains (the job in hand finishes, nothing more is leased, exit 0), a second aborts (the
/// job in hand is released). The tests around this one cover the drain logic through the drain file and
/// in-process calls; this one covers the signal wiring, which only exists on Unix.
#[cfg(unix)]
#[test]
fn sigterm_drains_the_worker_process_and_a_second_one_aborts() {
    use std::process::{Command, Stdio};
    use std::time::Instant;
    let Some(f) = fixture() else { return };
    for (signals, completed, released) in [(1, 1, 0), (2, 0, 1)] {
        let r = rig(f, &format!("sigterm{signals}"));
        // Three blobs at 400 ms each: the first job stays in hand for over a second.
        r.mock.state().blob_delay = Duration::from_millis(400);
        for i in 0..3 {
            r.push(f, &format!("j{i}"), i);
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_ibc-worker"))
            .args(["--coordinator", &r.mock.url, "--registration-token", REG])
            .args(["--concurrency", "1", "--poll-ms", "100"])
            .arg("--runner")
            .arg(&f.runner)
            .arg("--cache")
            .arg(&r.cfg.cache_dir)
            .stdin(Stdio::null())
            .spawn()
            .expect("starting ibc-worker");
        r.mock
            .wait_for("the first lease", Duration::from_secs(20), |st| {
                !st.leased.is_empty()
            });
        for _ in 0..signals {
            let pid = child.id().to_string();
            let kill = Command::new("kill").args(["-TERM", &pid]).status();
            assert!(kill.expect("running kill").success());
            std::thread::sleep(Duration::from_millis(100));
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            assert!(Instant::now() < deadline, "the worker did not exit after SIGTERM");
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(status.success(), "{signals} SIGTERM(s): exit {status}");
        let st = r.mock.state();
        assert_eq!(
            (st.completed.len(), st.released.len(), st.queue.len()),
            (completed, released, 2),
            "{signals} SIGTERM(s): the job in hand is finished (one) or released (two), the rest stay queued"
        );
    }
}

#[test]
fn abort_releases_the_job_in_hand() {
    let Some(f) = fixture() else { return };
    let mut r = rig(f, "abort");
    r.cfg.once = false;
    r.cfg.drain_timeout = Duration::ZERO;
    r.mock.state().blob_delay = Duration::from_millis(300);
    r.push(f, "j1", 1);
    let (cfg, shutdown) = (r.cfg.clone(), Arc::new(Shutdown::default()));
    let handle = shutdown.clone();
    let worker = std::thread::spawn(move || ibc_worker::run(cfg, handle));
    r.mock
        .wait_for("the first lease", Duration::from_secs(20), |st| {
            !st.leased.is_empty()
        });
    shutdown.request_drain();
    assert_eq!(
        worker.join().unwrap().unwrap(),
        Stats {
            completed: 0,
            failed: 0,
            released: 1,
            dropped: 0
        }
    );
    assert_eq!(
        r.mock.state().released,
        [("j1".to_string(), "worker shutting down".to_string())]
    );
}

#[test]
fn a_bad_registration_token_is_fatal_not_retried() {
    let Some(f) = fixture() else { return };
    let mut r = rig(f, "badreg");
    r.cfg.registration_token = Some("wrong".into());
    let e = ibc_worker::run(r.cfg.clone(), Arc::new(Shutdown::default())).unwrap_err();
    assert!(e.contains("registration refused"), "{e}");
    assert!(!r.cfg.token_file.exists());
}

#[test]
fn a_stale_stored_token_leads_to_a_new_registration() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "stale");
    std::fs::create_dir_all(&r.cfg.cache_dir).unwrap();
    std::fs::write(
        &r.cfg.token_file,
        json!({"coordinator": r.mock.url, "token": "revoked-long-ago"}).to_string(),
    )
    .unwrap();
    r.push(f, "j1", 2);
    assert_eq!(r.run().completed, 1);
    assert_eq!(r.mock.state().registrations, 1);
    let token: Value =
        serde_json::from_str(&std::fs::read_to_string(&r.cfg.token_file).unwrap()).unwrap();
    assert_eq!(token["token"], "worker-token-1");
}
