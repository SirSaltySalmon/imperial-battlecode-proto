//! Keeping participants on the server's patch.
//!
//! - `ibc update [--server URL]` asks the server (`GET /v1/distribution`) which `ibc` it runs and prints
//!   the exact command that upgrades this installation: `pip install --upgrade ...` for a pip install, or
//!   the download URL and SHA-256 of the archive for this platform. It never downloads or replaces anything
//!   itself: installing a binary stays a decision of the person at the keyboard.
//! - Before other commands, `check_before` does the same check at most once a day (the answer is cached
//!   in the per-user cache directory) and prints a one-line warning on stderr when this `ibc` is behind.
//!   It is silent and costs nothing without a server (`--server`, `IBC_SERVER`), offline, or with
//!   `IBC_NO_UPDATE_CHECK=1`; it never runs for `run-batch`, the workers' hot path.
//! - A workspace may pin its engine in `ibc.toml` (`[engine] version = "x.y.z"`, found in the current
//!   directory or above). A binary that differs is warned about loudly on every command, because local
//!   results then differ from the ladder's.

use crate::view::pip_share_dirs;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SERVER_ENV: &str = "IBC_SERVER";
const NO_CHECK_ENV: &str = "IBC_NO_UPDATE_CHECK";
const CHECK_EVERY_SECS: u64 = 86_400;
const CACHE_FILE: &str = "update-check.json";
const PIP_PACKAGE: &str = "imperial-battlecode";
const PIN_FILE: &str = "ibc.toml";
/// Commands that never check: the workers' runner path, the scripted identity, and `update` itself.
const NO_CHECK_COMMANDS: &[&str] = &["run-batch", "version", "update", "help"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub target: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Distribution {
    pub version: String,
    #[serde(default)]
    pub patch: serde_json::Value,
    #[serde(default)]
    pub pip: Option<String>,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    server: String,
    checked_at: u64,
    /// `None`: the last attempt failed (offline, no distribution); not retried before the day is over.
    distribution: Option<Distribution>,
}

/// What this binary is and how it was installed.
pub struct Installed {
    pub version: String,
    pub ruleset_hash: String,
    /// The release target this binary corresponds to, if it is one of the released platforms.
    pub target: Option<&'static str>,
    pub exe: Option<PathBuf>,
    pub pip: bool,
}

impl Installed {
    pub fn this() -> Installed {
        let exe = std::env::current_exe().ok();
        Installed {
            version: env!("CARGO_PKG_VERSION").to_string(),
            ruleset_hash: ibc_replay::hash_hex(ibc_engine::Ruleset::builtin().hash()),
            target: host_target(),
            pip: exe.as_deref().is_some_and(installed_by_pip),
            exe,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    UpToDate,
    /// The server runs a newer version.
    Outdated,
    /// This ibc is newer than the server's (installed before the ladder switched patch).
    Newer,
    /// Same version number, other ruleset: a rebuilt or patched binary on one side.
    OtherPatch,
}

/// `x.y.z[-pre][+build]` -> (`[x, y, z]`, `pre`); numeric parts that do not parse count as 0.
fn version_parts(v: &str) -> (Vec<u64>, Option<&str>) {
    let v = v.trim().trim_start_matches('v');
    let v = v.split('+').next().unwrap_or(v);
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (v, None),
    };
    (
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
        pre,
    )
}

/// Semver order: numeric parts first, then a pre-release sorts before its release.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let ((mut a, a_pre), (mut b, b_pre)) = (version_parts(a), version_parts(b));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a.cmp(&b).then_with(|| match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.cmp(y),
    })
}

pub fn status(installed: &Installed, dist: &Distribution) -> Status {
    match compare_versions(&dist.version, &installed.version) {
        Ordering::Greater => Status::Outdated,
        Ordering::Less => Status::Newer,
        Ordering::Equal => match dist.patch["ruleset_hash"].as_str() {
            Some(hash) if hash != installed.ruleset_hash => Status::OtherPatch,
            _ => Status::UpToDate,
        },
    }
}

/// The stderr line of the daily check, when there is something to say.
pub fn warning(installed: &Installed, dist: &Distribution) -> Option<String> {
    let what = match status(installed, dist) {
        Status::UpToDate => return None,
        Status::Outdated => format!(
            "ibc {} is out of date: the server runs {}",
            installed.version, dist.version
        ),
        Status::Newer => format!(
            "ibc {} is newer than the {} the server runs",
            installed.version, dist.version
        ),
        Status::OtherPatch => format!(
            "ibc {} has ruleset {}, the server's {} has ruleset {}",
            installed.version,
            installed.ruleset_hash,
            dist.version,
            dist.patch["ruleset_hash"].as_str().unwrap_or("?")
        ),
    };
    Some(format!(
        "warning: {what}; local matches will not match the ladder. Run `ibc update` for the command that \
         installs the server's version."
    ))
}

/// The commands that install the server's version (an upgrade, or a downgrade for a newer binary), the
/// one for this installation first.
pub fn instructions(installed: &Installed, dist: &Distribution) -> String {
    let pip_spec = dist
        .pip
        .clone()
        .unwrap_or_else(|| format!("{PIP_PACKAGE}=={}", dist.version));
    let pip = format!("  pip install --upgrade \"{pip_spec}\"\n");
    let artifact = installed
        .target
        .and_then(|t| dist.artifacts.iter().find(|a| a.target == t));
    let download = match artifact {
        Some(a) => {
            let replace = installed
                .exe
                .as_ref()
                .map(|e| format!("  then replace {} with the `ibc` inside\n", e.display()))
                .unwrap_or_default();
            format!(
                "  download {}\n  check    sha256 {}\n{replace}",
                a.url, a.sha256
            )
        }
        None => {
            let targets: Vec<&str> = dist.artifacts.iter().map(|a| a.target.as_str()).collect();
            format!(
                "  (no download for this platform; the server lists: {})\n",
                if targets.is_empty() {
                    "none".to_string()
                } else {
                    targets.join(", ")
                }
            )
        }
    };
    let v = &dist.version;
    if installed.pip {
        format!("Install ibc {v} (installed with pip):\n{pip}or download it instead:\n{download}")
    } else {
        format!("Install ibc {v} by downloading the release for this platform:\n{download}or with pip:\n{pip}")
    }
}

/// The released target triple matching this binary's platform.
fn host_target() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        _ => return None,
    })
}

/// The pip wheel installed its data next to this binary (`view::pip_share_dirs`).
fn installed_by_pip(exe: &Path) -> bool {
    pip_share_dirs(exe).iter().any(|d| d.is_dir())
}

// ---- the server ----

/// `--server`, else `IBC_SERVER`. `ibc login` will add its stored server here.
fn server(flag: Option<String>) -> Option<String> {
    flag.or_else(|| std::env::var(SERVER_ENV).ok())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

fn fetch(server: &str) -> Result<Distribution> {
    let url = format!("{server}/v1/distribution");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .redirects(0)
        .build();
    let body = match agent.get(&url).call() {
        Ok(res) => res
            .into_string()
            .with_context(|| format!("reading {url}"))?,
        Err(ureq::Error::Status(404, _)) => {
            bail!("{server} publishes no distribution ({url}: 404)")
        }
        Err(e) => bail!("{url}: {e}"),
    };
    serde_json::from_str(&body).with_context(|| format!("{url}: not a distribution"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn cache_path() -> Option<PathBuf> {
    crate::toolchain::cache_dir()
        .ok()
        .map(|d| d.join(CACHE_FILE))
}

fn read_cache(path: &Path) -> Option<Cache> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_cache(path: &Path, cache: &Cache) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_vec(cache) {
        let _ = std::fs::write(path, text);
    }
}

/// The cached answer when it is about `server` and younger than a day.
fn fresh(cache: Option<Cache>, server: &str, now: u64) -> Option<Cache> {
    cache.filter(|c| c.server == server && now.saturating_sub(c.checked_at) < CHECK_EVERY_SECS)
}

/// The server's distribution from the cache, or fetched (and cached) once the cache is a day old.
fn daily(server: &str, cache_file: Option<&Path>, now: u64) -> Option<Distribution> {
    if let Some(cached) = fresh(cache_file.and_then(read_cache), server, now) {
        return cached.distribution;
    }
    let distribution = fetch(server).ok();
    if let Some(path) = cache_file {
        write_cache(
            path,
            &Cache {
                server: server.to_string(),
                checked_at: now,
                distribution: distribution.clone(),
            },
        );
    }
    distribution
}

// ---- ibc.toml ----

/// The nearest `ibc.toml` at or above `start` and its `[engine] version`, if it pins one.
/// A bot's own `ibc.toml` (C++ build options) has no `[engine]`; the search goes on past it.
fn find_pin(start: &Path) -> Option<(PathBuf, String)> {
    start.ancestors().find_map(|dir| {
        let file = dir.join(PIN_FILE);
        let text = std::fs::read_to_string(&file).ok()?;
        let doc: toml::Table = toml::from_str(&text).ok()?;
        let version = doc.get("engine")?.get("version")?.as_str()?.to_string();
        Some((file, version))
    })
}

pub fn pin_message(file: &Path, pinned: &str, current: &str) -> Option<String> {
    if compare_versions(pinned, current) == Ordering::Equal {
        return None;
    }
    let line = "*".repeat(78);
    Some(format!(
        "warning: {line}\nwarning: {} pins engine {pinned}, but this ibc is {current}.\n\
         warning: matches you run here will not be the matches the ladder plays. Install ibc {pinned}\n\
         warning: (`pip install \"{PIP_PACKAGE}=={pinned}\"` or the release download) or change the pin.\n\
         warning: {line}",
        file.display()
    ))
}

fn warn_about_pin() {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    if let Some((file, pinned)) = find_pin(&cwd) {
        if let Some(message) = pin_message(&file, &pinned, env!("CARGO_PKG_VERSION")) {
            eprintln!("{message}");
        }
    }
}

// ---- entry points ----

/// Before a command (`command` is its name): the pin check, then the daily server check. Never fails.
pub fn check_before(command: Option<&str>) {
    let Some(command) = command.filter(|c| !c.starts_with('-')) else {
        return;
    };
    if NO_CHECK_COMMANDS.contains(&command) {
        return;
    }
    warn_about_pin();
    if std::env::var_os(NO_CHECK_ENV).is_some_and(|v| !v.is_empty() && v != "0") {
        return;
    }
    let Some(server) = server(None) else { return };
    if let Some(dist) = daily(&server, cache_path().as_deref(), now_secs()) {
        if let Some(w) = warning(&Installed::this(), &dist) {
            eprintln!("{w}");
        }
    }
}

/// `ibc update`: ask the server now (refreshing the daily cache) and print how to upgrade.
pub fn run(server_flag: Option<String>) -> Result<()> {
    warn_about_pin();
    let Some(server) = server(server_flag) else {
        bail!("no server to ask: pass --server URL or set {SERVER_ENV}");
    };
    let dist = fetch(&server)?;
    if let Some(path) = cache_path() {
        let cache = Cache {
            server: server.clone(),
            checked_at: now_secs(),
            distribution: Some(dist.clone()),
        };
        write_cache(&path, &cache);
    }
    let me = Installed::this();
    match status(&me, &dist) {
        Status::UpToDate => {
            println!(
                "ibc {} is up to date (the server {server} runs {}).",
                me.version, dist.version
            );
        }
        Status::Outdated | Status::Newer | Status::OtherPatch => {
            if let Some(w) = warning(&me, &dist) {
                eprintln!("{w}");
            }
            print!("{}", instructions(&me, &dist));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(version: &str, pip: bool) -> Installed {
        Installed {
            version: version.into(),
            ruleset_hash: "aaaa".into(),
            target: Some("x86_64-unknown-linux-musl"),
            exe: Some(PathBuf::from("/home/p/bin/ibc")),
            pip,
        }
    }

    fn dist(version: &str, ruleset: &str) -> Distribution {
        Distribution {
            version: version.into(),
            patch: serde_json::json!({"engine": version, "ruleset_hash": ruleset}),
            pip: Some(format!("imperial-battlecode=={version}")),
            artifacts: vec![Artifact {
                target: "x86_64-unknown-linux-musl".into(),
                url: format!("https://example.org/ibc-{version}-x86_64-unknown-linux-musl.tar.gz"),
                sha256: "ab".repeat(32),
            }],
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.2", "0.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("v1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0", "1.0.0-rc1"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0-rc1", "1.0.0-rc2"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0+build7", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.1.0", "0.1.1"), Ordering::Less);
    }

    #[test]
    fn status_and_warning_follow_the_server() {
        let me = installed("0.1.0", false);
        assert_eq!(status(&me, &dist("0.2.0", "aaaa")), Status::Outdated);
        assert_eq!(status(&me, &dist("0.1.0", "aaaa")), Status::UpToDate);
        assert_eq!(status(&me, &dist("0.0.9", "bbbb")), Status::Newer);
        assert_eq!(status(&me, &dist("0.1.0", "bbbb")), Status::OtherPatch);
        let w = warning(&me, &dist("0.2.0", "aaaa")).unwrap();
        assert!(
            w.contains("0.1.0 is out of date") && w.contains("ibc update"),
            "{w}"
        );
        assert!(warning(&me, &dist("0.1.0", "aaaa")).is_none());
        let w = warning(&me, &dist("0.0.9", "aaaa")).unwrap();
        assert!(w.contains("newer than the 0.0.9 the server runs"), "{w}");
    }

    #[test]
    fn instructions_name_the_command_for_this_installation_first() {
        let d = dist("0.2.0", "aaaa");
        let text = instructions(&installed("0.1.0", true), &d);
        assert!(text.starts_with("Install ibc 0.2.0 (installed with pip):\n  pip install --upgrade \"imperial-battlecode==0.2.0\""), "{text}");
        let text = instructions(&installed("0.1.0", false), &d);
        assert!(
            text.starts_with("Install ibc 0.2.0 by downloading"),
            "{text}"
        );
        assert!(text.contains("ibc-0.2.0-x86_64-unknown-linux-musl.tar.gz"));
        assert!(text.contains(&"ab".repeat(32)));
        let mut other = installed("0.1.0", false);
        other.target = Some("aarch64-apple-darwin");
        assert!(instructions(&other, &d).contains("no download for this platform"));
    }

    #[test]
    fn the_check_runs_at_most_once_a_day_per_server() {
        let dir = std::env::temp_dir().join(format!("ibc-update-{}", std::process::id()));
        let file = dir.join(CACHE_FILE);
        let d = dist("0.2.0", "aaaa");
        write_cache(
            &file,
            &Cache {
                server: "https://s".into(),
                checked_at: 1_000,
                distribution: Some(d.clone()),
            },
        );
        // Within the day the cached answer is used: no request is made to the (unreachable) server.
        assert_eq!(
            daily("https://s", Some(&file), 1_000 + 3_600),
            Some(d.clone())
        );
        assert!(fresh(read_cache(&file), "https://s", 1_000 + CHECK_EVERY_SECS).is_none());
        assert!(fresh(read_cache(&file), "https://other", 1_001).is_none());
        // A failed check is remembered too, so an offline laptop does not retry on every command.
        let unreachable = "http://127.0.0.1:9";
        assert_eq!(daily(unreachable, Some(&file), 5_000), None);
        let cache = read_cache(&file).unwrap();
        assert_eq!(
            (cache.server.as_str(), cache.checked_at, cache.distribution),
            (unreachable, 5_000, None)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_engine_pin_is_found_upwards_and_warns_when_it_differs() {
        let dir = std::env::temp_dir().join(format!("ibc-pin-{}", std::process::id()));
        let nested = dir.join("bots/mine");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(find_pin(&nested).is_none_or(|(f, _)| !f.starts_with(&dir)));
        std::fs::write(dir.join(PIN_FILE), "[engine]\nversion = \"0.3.0\"\n").unwrap();
        // A C++ bot's own ibc.toml (build options, no [engine]) does not hide the workspace pin.
        std::fs::write(nested.join(PIN_FILE), "[cpp]\nexceptions = true\n").unwrap();
        let (file, pinned) = find_pin(&nested).unwrap();
        assert_eq!((file, pinned.as_str()), (dir.join(PIN_FILE), "0.3.0"));
        let loud = pin_message(&dir.join(PIN_FILE), "0.3.0", "0.1.0").unwrap();
        assert!(
            loud.contains("pins engine 0.3.0, but this ibc is 0.1.0"),
            "{loud}"
        );
        assert!(pin_message(&dir.join(PIN_FILE), "0.1.0", "0.1.0").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
