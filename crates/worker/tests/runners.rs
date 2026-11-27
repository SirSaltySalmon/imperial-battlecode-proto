//! Side-by-side runners and `--allow-runner-fetch`: a worker serving a patch other than its own runner's.
//!
//! The fetch path downloads a native binary from the coordinator and executes it, so these tests pin down
//! what it must and must not do: nothing is downloaded without the flag or for another platform, nothing is
//! installed unless its SHA-256 matches, a binary plays only after its own `version --json` names the job's
//! patch, a binary that names another patch is deleted, and nothing is written outside the runners
//! directory.
//!
//! Needs `IBC_RUNNER` (a built `ibc`, like `tests/worker.rs`) and `rustc` on PATH; skipped without them,
//! or failed when `CI` is set.
//! The "other patch" runner is a few lines of Rust compiled here: it reports a made-up patch, logs how it
//! was called to `calls.log` beside itself, and hands everything else to the real `ibc`.

mod common;

use common::{match_job, Mock};
use ibc_worker::runner::{platform, RUNNER_FILE};
use ibc_worker::{Config, Shutdown, Stats};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const REG: &str = "registration-secret";

struct Fixture {
    patch: Value,
    runner: PathBuf,
    rusher: Vec<u8>,
    map: Vec<u8>,
    /// A runner that says it is `fake_patch(.., "a")`, and one that says it is `fake_patch(.., "b")`.
    fake_a: Vec<u8>,
    fake_b: Vec<u8>,
}

fn fake_patch(base: &Value, tag: &str) -> Value {
    let mut p = base.clone();
    p["engine"] = json!(format!("0.0.0-fake-{tag}"));
    p
}

/// Compile a runner that reports `patch` and forwards every other command to `real`.
fn compile_fake(real: &Path, patch: &Value, dir: &Path) -> Option<Vec<u8>> {
    let src = format!(
        r#"use std::io::Write;
fn main() {{
    let args: Vec<String> = std::env::args().skip(1).collect();
    let log = std::env::current_exe().unwrap().with_file_name("calls.log");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log).unwrap();
    writeln!(f, "{{}}", args.join(" ")).unwrap();
    if args.first().map(String::as_str) == Some("version") {{
        println!("{{}}", {patch:?});
        return;
    }}
    let status = std::process::Command::new({real:?}).args(&args).status().expect("running the real ibc");
    std::process::exit(status.code().unwrap_or(1));
}}
"#,
        patch = patch.to_string(),
        real = real.to_str().expect("UTF-8 runner path"),
    );
    std::fs::create_dir_all(dir).unwrap();
    let (src_path, exe) = (dir.join("fake.rs"), dir.join(RUNNER_FILE));
    std::fs::write(&src_path, src).unwrap();
    let out = std::process::Command::new("rustc")
        .args(["--edition", "2021", "-o"])
        .arg(&exe)
        .arg(&src_path)
        .output();
    match out {
        Ok(o) if o.status.success() => Some(std::fs::read(&exe).unwrap()),
        Ok(o) => panic!(
            "compiling the fake runner:\n{}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => common::skip(&format!("`rustc` is needed to build the fake runner ({e})")),
    }
}

fn fixture() -> Option<&'static Fixture> {
    static F: OnceLock<Option<Fixture>> = OnceLock::new();
    F.get_or_init(|| {
        let runner = common::runner("the runner tests")?.canonicalize().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let bot = root.join("bots/rusher");
        let out = std::process::Command::new(&runner)
            .arg("build")
            .arg(&bot)
            .output()
            .expect("running `ibc build`");
        assert!(
            out.status.success(),
            "`ibc build`: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let patch =
            serde_json::to_value(ibc_worker::runner::probe_version(&runner).unwrap()).unwrap();
        let fakes =
            std::env::temp_dir().join(format!("ibc-worker-fake-runners-{}", std::process::id()));
        let fake_a = compile_fake(&runner, &fake_patch(&patch, "a"), &fakes.join("a"))?;
        let fake_b = compile_fake(&runner, &fake_patch(&patch, "b"), &fakes.join("b"))?;
        let _ = std::fs::remove_dir_all(&fakes);
        Some(Fixture {
            rusher: std::fs::read(bot.join(".ibc/bot.wasm")).unwrap(),
            map: std::fs::read(root.join("maps/crossfire.ibcmap")).unwrap(),
            patch,
            runner,
            fake_a,
            fake_b,
        })
    })
    .as_ref()
}

struct Rig {
    mock: Mock,
    cfg: Config,
    rusher: String,
    map: String,
}

fn rig(f: &Fixture, tag: &str, allow_runner_fetch: bool) -> Rig {
    let mock = Mock::start(0, REG);
    let dir = std::env::temp_dir().join(format!("ibc-worker-runners-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = Config::new(&mock.url, &f.runner, dir.join("cache"));
    cfg.registration_token = Some(REG.into());
    cfg.name = format!("test-{tag}");
    cfg.concurrency = 2;
    cfg.poll_wait_ms = 100;
    cfg.backoff_min = Duration::from_millis(50);
    cfg.backoff_max = Duration::from_millis(300);
    cfg.once = true;
    cfg.allow_runner_fetch = allow_runner_fetch;
    let (rusher, map) = (mock.add_blob(&f.rusher), mock.add_blob(&f.map));
    Rig {
        mock,
        cfg,
        rusher,
        map,
    }
}

impl Rig {
    /// A short rusher mirror for `patch`, naming `runner_blobs` when given.
    fn push(&self, id: &str, patch: &Value, runner_blobs: Option<Value>) {
        let mut job = match_job(id, patch, &self.rusher, &self.rusher, &self.map, 1);
        job["max_ticks"] = json!(300);
        if let Some(b) = runner_blobs {
            job["runner_blobs"] = b;
        }
        self.mock.push_job(job);
    }

    fn run(&self) -> Stats {
        ibc_worker::run(self.cfg.clone(), Arc::new(Shutdown::default())).expect("worker run")
    }

    /// Where this worker keeps the runner of `patch`.
    fn slot(&self, patch: &Value) -> PathBuf {
        let name = format!(
            "{}-{}-{}",
            patch["engine"].as_str().unwrap(),
            patch["ruleset_hash"].as_str().unwrap(),
            patch["wasmtime"].as_str().unwrap()
        );
        self.cfg.runners_dir.join(name).join(RUNNER_FILE)
    }

    fn place(&self, patch: &Value, bytes: &[u8]) {
        let slot = self.slot(patch);
        std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
        std::fs::write(&slot, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&slot, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// The commands the runner in `patch`'s slot directory was started with.
    fn calls(&self, patch: &Value) -> Vec<String> {
        let log = self.slot(patch).with_file_name("calls.log");
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn downloads(&self, sha: &str) -> usize {
        self.mock
            .state()
            .blob_log
            .iter()
            .filter(|s| *s == sha)
            .count()
    }

    fn released(&self) -> Vec<(String, String)> {
        self.mock.state().released.clone()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.cfg.cache_dir.parent().unwrap());
    }
}

fn stats(completed: u64, failed: u64, released: u64) -> Stats {
    Stats {
        completed,
        failed,
        released,
        dropped: 0,
    }
}

fn this_platform(sha: &str) -> Option<Value> {
    Some(json!({ platform(): sha }))
}

fn mismatch(id: &str) -> Vec<(String, String)> {
    vec![(id.to_string(), "patch mismatch".to_string())]
}

#[test]
fn a_side_by_side_runner_plays_its_own_patch_and_the_primary_keeps_playing_its_own() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "side", false);
    let a = fake_patch(&f.patch, "a");
    r.place(&a, &f.fake_a);
    r.push("other", &a, None);
    r.push("own", &f.patch, None);
    assert_eq!(r.run(), stats(2, 0, 0));
    let calls = r.calls(&a);
    assert_eq!(calls[0], "version --json", "probed before use: {calls:?}");
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("run-batch")).count(),
        1,
        "exactly the one job of its patch went to the side-by-side runner: {calls:?}"
    );
}

#[test]
fn a_side_by_side_runner_that_names_another_patch_is_not_trusted() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "misnamed", false);
    let a = fake_patch(&f.patch, "a");
    r.place(&a, &f.fake_b);
    r.push("other", &a, None);
    assert_eq!(r.run(), stats(0, 0, 1));
    assert_eq!(r.released(), mismatch("other"));
    assert!(r.calls(&a).iter().all(|c| c == "version --json"));
}

#[test]
fn without_the_flag_a_runner_named_by_the_job_is_never_downloaded() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "noflag", false);
    let sha = r.mock.add_blob(&f.fake_a);
    let a = fake_patch(&f.patch, "a");
    r.push("other", &a, this_platform(&sha));
    assert_eq!(r.run(), stats(0, 0, 1));
    assert_eq!(r.released(), mismatch("other"));
    assert_eq!(r.downloads(&sha), 0);
    assert!(!r.cfg.runners_dir.exists());
}

#[test]
fn a_runner_named_for_another_platform_is_not_downloaded() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "platform", true);
    let sha = r.mock.add_blob(&f.fake_a);
    let a = fake_patch(&f.patch, "a");
    r.push("other", &a, Some(json!({"plan9-mips": sha})));
    assert_eq!(r.run(), stats(0, 0, 1));
    assert_eq!(r.released(), mismatch("other"));
    assert_eq!(r.downloads(&sha), 0);
}

#[test]
fn with_the_flag_a_named_runner_is_downloaded_once_installed_and_played() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "fetch", true);
    let sha = r.mock.add_blob(&f.fake_a);
    let a = fake_patch(&f.patch, "a");
    for id in ["j1", "j2", "j3"] {
        r.push(id, &a, this_platform(&sha));
    }
    assert_eq!(r.run(), stats(3, 0, 0));
    assert_eq!(
        r.downloads(&sha),
        1,
        "installed once, then resolved from the runners directory"
    );
    assert_eq!(std::fs::read(r.slot(&a)).unwrap(), f.fake_a);
    let dir = r.slot(&a).parent().unwrap().to_path_buf();
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["calls.log", RUNNER_FILE],
        "no temporary files left behind"
    );
    let calls = r.calls(&a);
    assert_eq!(calls[0], "version --json", "probed before use: {calls:?}");
    assert!(
        calls.iter().any(|c| c.starts_with("run-batch")),
        "{calls:?}"
    );
}

#[test]
fn a_named_runner_with_the_wrong_hash_is_never_installed() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "corrupt", true);
    let sha = r.mock.add_blob(&f.fake_a);
    r.mock.state().corrupt.insert(sha.clone());
    let a = fake_patch(&f.patch, "a");
    r.push("other", &a, this_platform(&sha));
    assert_eq!(r.run(), stats(0, 1, 0));
    let failed = r.mock.state().failed.clone();
    assert!(failed[0].1.contains("hash mismatch"), "{failed:?}");
    assert!(!r.slot(&a).exists());
    assert!(!r.cfg.cache_dir.join("blobs").join(&sha).exists());
}

#[test]
fn a_named_runner_that_reports_another_patch_is_deleted_unused() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "wrongpatch", true);
    let sha = r.mock.add_blob(&f.fake_b);
    let a = fake_patch(&f.patch, "a");
    r.push("other", &a, this_platform(&sha));
    assert_eq!(r.run(), stats(0, 0, 1));
    assert_eq!(r.released(), mismatch("other"));
    assert!(
        !r.slot(&a).exists(),
        "a runner that is not the job's patch must not stay installed"
    );
    assert!(r.calls(&a).iter().all(|c| c == "version --json"));
}

#[test]
fn a_patch_that_is_not_a_plain_directory_name_downloads_and_writes_nothing() {
    let Some(f) = fixture() else { return };
    let r = rig(f, "escape", true);
    let sha = r.mock.add_blob(&f.fake_a);
    let mut evil = f.patch.clone();
    evil["engine"] = json!("../../escape");
    r.push("other", &evil, this_platform(&sha));
    assert_eq!(r.run(), stats(0, 0, 1));
    assert_eq!(r.released(), mismatch("other"));
    assert_eq!(r.downloads(&sha), 0);
    let outside = r.cfg.cache_dir.parent().unwrap();
    let entries: Vec<_> = std::fs::read_dir(outside)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["cache"], "nothing beside the cache directory");
}
