//! `ibc run-batch`: many matches on worker threads. Two front ends:
//!
//! * a text manifest (`botA botB map seed outdir` per line) with human-readable output, and
//! * `--jsonl`: jobs as JSON lines on stdin, machine events as JSON lines on stdout (see docs/JOBS.md).

use crate::job::{execute, resolve_bot, result_line, wasm_bot, BotInput, JobSpec};
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// Run `work` on every item from `rx` with `jobs` threads.
fn pool<T: Send + 'static>(jobs: usize, rx: Receiver<T>, work: impl Fn(T) + Send + Sync + 'static) {
    let rx = Arc::new(Mutex::new(rx));
    let work = Arc::new(work);
    let handles: Vec<_> = (0..jobs.max(1))
        .map(|_| {
            let (rx, work) = (rx.clone(), work.clone());
            std::thread::spawn(move || loop {
                let item = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                match item {
                    Ok(item) => work(item),
                    Err(_) => break,
                }
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
}

/// `execute`, with a panic turned into an error so one bad job cannot take the batch down.
fn execute_guarded(job: &JobSpec) -> Result<crate::job::JobResult> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(job))) {
        Ok(r) => r,
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
            Err(anyhow!("internal error (panic): {}", msg.unwrap_or_else(|| "unknown".into())))
        }
    }
}

// ---- text manifest ----

pub fn run_manifest(manifest: &Path, jobs: usize) -> Result<()> {
    let text = std::fs::read_to_string(manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let base = manifest.parent().unwrap_or(Path::new("."));
    let at = |p: &str| if Path::new(p).is_absolute() { PathBuf::from(p) } else { base.join(p) };

    // Parse everything and build each bot directory once before any match starts.
    let mut bots: HashMap<PathBuf, BotInput> = HashMap::new();
    let mut specs = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 5 {
            bail!("{}:{}: expected `botA botB map seed outdir`, got {} fields", manifest.display(), i + 1, f.len());
        }
        let seed: u64 = f[3].parse().map_err(|_| anyhow!("{}:{}: bad seed {:?}", manifest.display(), i + 1, f[3]))?;
        let mut bot = |arg: &str| -> Result<BotInput> {
            let path = at(arg);
            if let Some(b) = bots.get(&path) {
                return Ok(b.clone());
            }
            let b = resolve_bot(&path).with_context(|| format!("{}:{}", manifest.display(), i + 1))?;
            bots.insert(path, b.clone());
            Ok(b)
        };
        specs.push(JobSpec {
            bot_a: bot(f[0])?,
            bot_b: bot(f[1])?,
            map: at(f[2]),
            seed,
            out_dir: at(f[4]),
            max_ticks: None,
            frames: false,
        });
    }

    let total = specs.len();
    let failed = Arc::new(Mutex::new(0usize));
    let (tx, rx) = sync_channel(total.max(1));
    for (i, spec) in specs.into_iter().enumerate() {
        tx.send((i, spec)).expect("queue holds every job");
    }
    drop(tx);
    let started = Instant::now();
    let failed2 = failed.clone();
    pool(jobs, rx, move |(i, spec): (usize, JobSpec)| {
        let line = match execute_guarded(&spec) {
            Ok(r) => format!("[{}/{total}] {}: {}", i + 1, spec.out_dir.display(), result_line(&r.summary)),
            Err(e) => {
                *failed2.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                format!("[{}/{total}] {}: FAILED: {e:#}", i + 1, spec.out_dir.display())
            }
        };
        println!("{line}");
    });
    let failed = *failed.lock().unwrap_or_else(|e| e.into_inner());
    println!("{} matches, {failed} failed, {:.1}s", total, started.elapsed().as_secs_f64());
    if failed > 0 {
        bail!("{failed} of {total} matches failed");
    }
    Ok(())
}

// ---- JSONL protocol ----

#[derive(Deserialize)]
struct JsonJob {
    id: Value,
    bot_a: PathBuf,
    bot_b: PathBuf,
    map: PathBuf,
    seed: u64,
    out_dir: PathBuf,
    #[serde(default)]
    fuel_mult_a: Option<f64>,
    #[serde(default)]
    fuel_mult_b: Option<f64>,
    #[serde(default)]
    max_ticks: Option<u32>,
    #[serde(default)]
    name_a: Option<String>,
    #[serde(default)]
    name_b: Option<String>,
}

fn emit(event: Value) {
    let mut out = std::io::stdout().lock();
    // One write per event keeps lines whole even with several worker threads.
    let _ = writeln!(out, "{event}");
    let _ = out.flush();
}

fn run_json_job(job: JsonJob) {
    let id = job.id.clone();
    emit(json!({"event": "started", "id": id}));
    let started = Instant::now();
    let run = || -> Result<Value> {
        // Bot names end up in the replay header. Unless the job names them, derive the name from the module
        // content (not from its path), so the same job gives the same replay bytes on every machine.
        let bot = |path: &Path, mult: Option<f64>, name: &Option<String>| -> Result<BotInput> {
            let mut b = wasm_bot(path)?;
            b.name = match name {
                Some(n) => n.clone(),
                None => {
                    let wasm = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
                    ibc_replay::sha256_hex(&wasm)[..12].to_string()
                }
            };
            if let Some(m) = mult {
                b.fuel_multiplier = m;
            }
            Ok(b)
        };
        let spec = JobSpec {
            bot_a: bot(&job.bot_a, job.fuel_mult_a, &job.name_a)?,
            bot_b: bot(&job.bot_b, job.fuel_mult_b, &job.name_b)?,
            map: job.map.clone(),
            seed: job.seed,
            out_dir: job.out_dir.clone(),
            max_ticks: job.max_ticks,
            frames: false,
        };
        let r = execute_guarded(&spec)?;
        Ok(json!({
            "event": "finished",
            "id": id,
            "winner": r.summary.winner,
            "outcome": r.summary.outcome,
            "score": r.summary.score,
            "ticks": r.summary.ticks,
            "replay_sha256": r.summary.replay_sha256,
            "summary_path": r.summary_path.to_string_lossy(),
            "replay_path": r.replay_path.to_string_lossy(),
            "wall_ms": started.elapsed().as_millis() as u64,
        }))
    };
    match run() {
        Ok(event) => emit(event),
        Err(e) => {
            eprintln!("job {id}: {e:#}");
            emit(json!({"event": "failed", "id": id, "error": format!("{e:#}")}));
        }
    }
}

/// Read jobs from stdin until end of file; stdout carries only events.
pub fn run_jsonl(jobs: usize) -> Result<()> {
    // A small queue: stdin is read only as fast as workers take jobs, so a feeder gets back-pressure.
    let (tx, rx) = sync_channel::<JsonJob>(jobs.max(1));
    let reader = std::thread::spawn(move || {
        for (n, line) in std::io::stdin().lock().lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("stdin: {e}");
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JsonJob>(&line) {
                Ok(job) => {
                    if tx.send(job).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    // Not a job we can name reliably: report with whatever id the line carries.
                    let id = serde_json::from_str::<Value>(&line).ok().map(|v| v["id"].clone()).unwrap_or(Value::Null);
                    eprintln!("stdin line {}: {e}", n + 1);
                    emit(json!({"event": "failed", "id": id, "error": format!("invalid job: {e}")}));
                }
            }
        }
    });
    pool(jobs, rx, run_json_job);
    let _ = reader.join();
    Ok(())
}
