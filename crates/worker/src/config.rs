//! Worker configuration and its command line.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    pub coordinator: String,
    /// Shared secret used once, to register. Not needed when `token_file` already holds a worker token.
    pub registration_token: Option<String>,
    /// Where the per-worker bearer token is persisted (mode 600).
    pub token_file: PathBuf,
    pub name: String,
    pub labels: Vec<String>,
    /// Matches run in parallel; also reported as `cores` at registration.
    pub concurrency: usize,
    pub cache_dir: PathBuf,
    pub cache_cap_bytes: u64,
    pub max_blob_bytes: u64,
    /// The `ibc` binary of the current patch.
    pub runner: PathBuf,
    /// Side-by-side runners of other patches: `<runners_dir>/<engine>-<ruleset>-<wasmtime>/ibc`.
    pub runners_dir: PathBuf,
    pub allow_runner_fetch: bool,
    /// Wall-clock limit of one match; past it the runner is killed and the job failed as retryable, with
    /// `code: "timeout"`.
    pub job_timeout: Duration,
    /// Longest gap between heartbeats; a shorter lease (`lease_secs`) brings it down to a sixth of the lease.
    pub heartbeat_interval: Duration,
    /// How long a lease lasts from grant and from each heartbeat when the coordinator does not say
    /// (`lease_secs`, protocol 1.1), and the most it is ever assumed to last (180 s in protocol v1).
    pub lease_duration: Duration,
    pub poll_wait_ms: u64,
    pub http_timeout: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// Draining starts when this file appears.
    pub drain_file: Option<PathBuf>,
    /// How long a drain waits for running jobs before killing and releasing them.
    pub drain_timeout: Duration,
    /// Exit once the queue is empty (a lease poll returns nothing while no job is running).
    pub once: bool,
}

impl Config {
    pub fn new(
        coordinator: impl Into<String>,
        runner: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Config {
        let cache_dir = cache_dir.into();
        Config {
            coordinator: coordinator.into(),
            registration_token: None,
            token_file: cache_dir.join("worker-token.json"),
            name: default_name(),
            labels: Vec::new(),
            concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            runners_dir: cache_dir.join("runners"),
            cache_dir,
            cache_cap_bytes: 2 << 30,
            max_blob_bytes: 64 << 20,
            runner: runner.into(),
            allow_runner_fetch: false,
            job_timeout: Duration::from_secs(300),
            heartbeat_interval: Duration::from_secs(30),
            lease_duration: Duration::from_secs(180),
            poll_wait_ms: 25_000,
            http_timeout: Duration::from_secs(30),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            drain_file: None,
            drain_timeout: Duration::from_secs(90),
            once: false,
        }
    }
}

fn default_name() -> String {
    ["IBC_WORKER_NAME", "HOSTNAME", "COMPUTERNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "worker".into())
}

pub const USAGE: &str = "\
ibc-worker: lease Imperial Battlecode match jobs from a coordinator and run them

Usage: ibc-worker --coordinator URL --runner PATH [options]

  --coordinator URL             coordinator base URL, https://...            [env IBC_COORDINATOR]
  --registration-token T        shared secret, used once to register         [env IBC_REGISTRATION_TOKEN]
  --registration-token-file F   read the registration token from a file (keeps it out of argv)
  --token-file F                where the worker token is persisted, mode 600 [default CACHE/worker-token.json]
  --name N                      worker name                    [env IBC_WORKER_NAME, default hostname]
  --labels a,b                  routing labels, e.g. trusted,x86_64-linux    [env IBC_WORKER_LABELS]
  --concurrency C               matches in parallel            [env IBC_WORKER_CONCURRENCY, default cores]
  --cache DIR                   blob cache, runners and scratch space        [default ./ibc-worker-cache]
  --cache-max-mb N              blob cache size cap                          [default 2048]
  --runner PATH                 `ibc` binary of the current patch            [env IBC_RUNNER]
  --runners-dir DIR             other patches: DIR/<engine>-<ruleset>-<wasmtime>/ibc [default CACHE/runners]
  --allow-runner-fetch          install a runner the job names as a blob (runs code the coordinator serves)
  --job-timeout-secs S          wall-clock limit per match                   [default 300]
  --drain-file F                start draining when this file appears
  --drain-timeout-secs S        wait this long for running jobs when draining [default 90]
  --once                        exit when the queue is empty
  --poll-ms MS                  lease long-poll wait                         [default 25000]
  --heartbeat-secs S            at most this long between heartbeats         [default 30]
  --lease-secs S                lease length when the coordinator does not say (`lease_secs`), and the
                                most a lease is ever assumed to last          [default 180]
                                Heartbeats come every min(heartbeat-secs, lease / 6).
  --version, --help

Signals: SIGTERM / Ctrl-C drains (stop leasing, finish running jobs); a second one aborts.
Logs: JSON lines on stderr; IBC_LOG=debug|info|warn|error.
";

fn secs(v: &str, flag: &str) -> Result<Duration, String> {
    // Capped at a year: larger values only overflow `Instant` arithmetic later.
    let year = 365.0 * 86_400.0;
    v.parse::<f64>()
        .ok()
        .filter(|s| (0.0..=year).contains(s))
        .map(Duration::from_secs_f64)
        .ok_or_else(|| format!("{flag}: bad duration {v:?}"))
}

fn int<T: std::str::FromStr>(v: &str, flag: &str) -> Result<T, String> {
    v.parse().map_err(|_| format!("{flag}: bad number {v:?}"))
}

/// Parse the command line (without the program name). Environment variables fill in what flags omit.
pub fn parse_args(args: &[String]) -> Result<Config, String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let cache = flag_value(args, "--cache")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ibc-worker-cache"));
    let mut cfg = Config::new(
        env("IBC_COORDINATOR").unwrap_or_default(),
        env("IBC_RUNNER").unwrap_or_default(),
        cache,
    );
    cfg.registration_token = env("IBC_REGISTRATION_TOKEN");
    let labels = |v: &str| {
        v.split(',')
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect()
    };
    if let Some(v) = env("IBC_WORKER_LABELS") {
        cfg.labels = labels(&v);
    }
    if let Some(v) = env("IBC_WORKER_CONCURRENCY") {
        cfg.concurrency = int(&v, "IBC_WORKER_CONCURRENCY")?;
    }
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let flag = flag.as_str();
        match flag {
            "--allow-runner-fetch" => cfg.allow_runner_fetch = true,
            "--once" => cfg.once = true,
            _ => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("{flag}: missing value (or unknown flag)"))?
                    .as_str();
                match flag {
                    "--coordinator" => cfg.coordinator = v.into(),
                    "--registration-token" => cfg.registration_token = Some(v.into()),
                    "--registration-token-file" => {
                        let t =
                            std::fs::read_to_string(v).map_err(|e| format!("{flag}: {v}: {e}"))?;
                        cfg.registration_token = Some(t.trim().to_string());
                    }
                    "--token-file" => cfg.token_file = v.into(),
                    "--name" => cfg.name = v.into(),
                    "--labels" => cfg.labels = labels(v),
                    "--concurrency" => cfg.concurrency = int(v, flag)?,
                    "--cache" => {}
                    "--cache-max-mb" => {
                        cfg.cache_cap_bytes = int::<u64>(v, flag)?.saturating_mul(1 << 20)
                    }
                    "--runner" => cfg.runner = v.into(),
                    "--runners-dir" => cfg.runners_dir = v.into(),
                    "--job-timeout-secs" => cfg.job_timeout = secs(v, flag)?,
                    "--drain-file" => cfg.drain_file = Some(v.into()),
                    "--drain-timeout-secs" => cfg.drain_timeout = secs(v, flag)?,
                    "--poll-ms" => cfg.poll_wait_ms = int(v, flag)?,
                    "--heartbeat-secs" => cfg.heartbeat_interval = secs(v, flag)?,
                    "--lease-secs" => cfg.lease_duration = secs(v, flag)?,
                    _ => return Err(format!("unknown flag {flag}")),
                }
            }
        }
    }
    // An emptied token file means "already registered", not an empty secret.
    cfg.registration_token = cfg.registration_token.filter(|t| !t.is_empty());
    if cfg.coordinator.is_empty() {
        return Err("--coordinator (or IBC_COORDINATOR) is required".into());
    }
    if !(cfg.coordinator.starts_with("https://") || cfg.coordinator.starts_with("http://")) {
        return Err(format!(
            "--coordinator: {:?} is not an http(s) URL",
            cfg.coordinator
        ));
    }
    if cfg.runner.as_os_str().is_empty() {
        return Err("--runner (or IBC_RUNNER) is required".into());
    }
    if cfg.concurrency == 0 {
        return Err("--concurrency must be at least 1".into());
    }
    Ok(cfg)
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_the_documented_lifecycle_flags() {
        let c = parse_args(&args(
            "--coordinator https://c.example --registration-token T --name n1 --labels trusted,x86_64-linux \
             --concurrency 3 --cache /var/cache/ibc --runner /usr/bin/ibc --once --cache-max-mb 10 --job-timeout-secs 2.5",
        ))
        .unwrap();
        assert_eq!(
            (
                c.coordinator.as_str(),
                c.name.as_str(),
                c.concurrency,
                c.once
            ),
            ("https://c.example", "n1", 3, true)
        );
        assert_eq!(c.labels, ["trusted", "x86_64-linux"]);
        assert_eq!(c.registration_token.as_deref(), Some("T"));
        assert_eq!(
            c.token_file,
            PathBuf::from("/var/cache/ibc").join("worker-token.json")
        );
        assert_eq!(
            c.runners_dir,
            PathBuf::from("/var/cache/ibc").join("runners")
        );
        assert_eq!(
            (c.cache_cap_bytes, c.job_timeout),
            (10 << 20, Duration::from_millis(2500))
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_args(&args("--coordinator ftp://x --runner r")).is_err());
        assert!(parse_args(&args("--coordinator http://x --runner r --concurrency 0")).is_err());
        assert!(parse_args(&args("--coordinator http://x --runner r --bogus 1")).is_err());
        assert!(parse_args(&args("--coordinator http://x --runner r --name")).is_err());
        assert!(parse_args(&args("--coordinator http://x --runner r --lease-secs 1e30")).is_err());
    }
}
