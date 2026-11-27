//! Playing a schedule of games. Two backends behind `run_games`:
//!
//! * **wasm** (the default): one child `ibc run-batch --jsonl --jobs N` (docs/JOBS.md), the path every
//!   real match takes. Fuel is metered, so a bot that overruns its budget is cut short as it would be
//!   online. Bot directories are built in parallel first, and `balance` details are re-simulated from
//!   the replays on `N` threads afterwards;
//! * **native**: the built-in reference bots linked as Rust through `ibc-sim`, on worker threads. Fuel is
//!   NOT metered (a native build cannot count wasm instructions): every bot has unlimited fuel. Fast, for
//!   iteration and debugging; results from it are not the game as played.
//!
//! Both drive the same engine with the same inputs, so for the same (map, seed, slots) they must agree
//! on the final state hash as long as the wasm bots stay inside their fuel budget.

use crate::metrics::Collector;
use crate::model::{Bot, FuelRec, Game, GameResult, MapEntry};
use ibc_replay::{hash_hex, BotInfo, Header, MatchResult, Replay, ReplayWriter};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    /// Wasm modules under the fuel budget, as in a real match. The default.
    Wasm,
    /// Built-in bots linked as Rust: unmetered, unlimited fuel. Opt-in only.
    Native,
}

impl BackendKind {
    pub fn parse(s: &str) -> Result<BackendKind, String> {
        match s {
            "wasm" => Ok(BackendKind::Wasm),
            "native" => Ok(BackendKind::Native),
            other => Err(format!(
                "unknown backend `{other}` (wasm: fuel metered, the default; native: unmetered)"
            )),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            BackendKind::Wasm => "wasm",
            BackendKind::Native => "native",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunOpts {
    pub backend: BackendKind,
    /// Games in parallel.
    pub jobs: usize,
    /// Collect per-tick details (needed by `balance`). On the wasm backend this re-simulates each replay.
    pub detail: bool,
    /// The `ibc` runner binary; `None` = `$IBC_BIN`, then `target/release/ibc`, then `ibc` on the PATH.
    pub ibc: Option<PathBuf>,
    /// Where wasm games write their replay, summary and logs (`<out_dir>/games/g<id>/`).
    pub out_dir: PathBuf,
    /// Keep the per-game directories of wasm games (replays are what you read when a loss looks wrong).
    pub keep_replays: bool,
    pub max_ticks: u32,
}

impl RunOpts {
    pub fn new(out_dir: PathBuf) -> RunOpts {
        RunOpts {
            backend: BackendKind::Wasm,
            jobs: default_jobs(),
            detail: false,
            ibc: None,
            out_dir,
            keep_replays: true,
            max_ticks: ibc_sim::DEFAULT_MAX_TICKS,
        }
    }
}

pub fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

pub struct Run {
    /// One result per scheduled game, in schedule order.
    pub results: Vec<GameResult>,
    pub warnings: Vec<String>,
}

/// Play every game of the schedule. Games that cannot be played come back with `error` set.
pub fn run_games(
    bots: &[Bot],
    maps: &[MapEntry],
    games: &[Game],
    opts: &RunOpts,
) -> Result<Run, String> {
    let mut slots: Vec<Option<GameResult>> = vec![None; games.len()];
    let mut warnings = Vec::new();
    let played = match opts.backend {
        BackendKind::Native => {
            for g in games {
                if let Some(bot) = [&bots[g.a], &bots[g.b]]
                    .into_iter()
                    .find(|b| b.native.is_none())
                {
                    return Err(format!(
                        "`{}` is not a built-in bot, so it cannot run on the native backend (built in: {}); \
                         drop --backend native",
                        bot.label,
                        ibc_sim::BOTS.join(", ")
                    ));
                }
            }
            run_native(bots, maps, games, opts)
        }
        BackendKind::Wasm if games.is_empty() => Vec::new(),
        BackendKind::Wasm => run_wasm(bots, maps, games, opts, &mut warnings)?,
    };
    for (pos, r) in played {
        slots[pos] = Some(r);
    }
    let results = slots
        .into_iter()
        .zip(games)
        .map(|(slot, g)| {
            slot.unwrap_or_else(|| {
                let err = "the runner produced no result for this game".to_string();
                GameResult::failed(
                    g,
                    &maps[g.map].name,
                    &bots[g.a].label,
                    &bots[g.b].label,
                    "wasm",
                    err,
                )
            })
        })
        .collect();
    Ok(Run { results, warnings })
}

// ---- native ----

fn native_game(bots: &[Bot], maps: &[MapEntry], g: &Game, opts: &RunOpts) -> GameResult {
    let (a, b, map) = (&bots[g.a], &bots[g.b], &maps[g.map]);
    let mut r = GameResult::failed(g, &map.name, &a.label, &b.label, "native", String::new());
    let (na, nb) = (
        a.native.as_deref().unwrap_or(""),
        b.native.as_deref().unwrap_or(""),
    );
    let mut collector = opts.detail.then(Collector::new);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ibc_sim::run_with(&map.text, g.seed, na, nb, opts.max_ticks, |m| {
            if let Some(c) = collector.as_mut() {
                c.observe(m);
            }
        })
    }));
    match outcome {
        Ok(Ok(sim)) => {
            match serde_json::from_str::<Value>(&sim.summary_json) {
                Ok(v) => r.apply_engine_summary(&v),
                Err(e) => r.error = Some(format!("engine summary is not JSON: {e}")),
            }
            r.act = Some([
                [sim.acts[0].calls, sim.acts[0].errors],
                [sim.acts[1].calls, sim.acts[1].errors],
            ]);
            r.detail = collector.map(Collector::finish);
            if r.error.as_deref() == Some("") {
                r.error =
                    (!sim.finished).then(|| format!("not finished after {} ticks", sim.ticks));
            }
        }
        Ok(Err(e)) => r.error = Some(e),
        Err(_) => r.error = Some("panicked (see stderr)".to_string()),
    }
    r
}

fn run_native(
    bots: &[Bot],
    maps: &[MapEntry],
    games: &[Game],
    opts: &RunOpts,
) -> Vec<(usize, GameResult)> {
    let next = AtomicUsize::new(0);
    let done = Mutex::new(Vec::with_capacity(games.len()));
    std::thread::scope(|scope| {
        for _ in 0..opts.jobs.clamp(1, games.len().max(1)) {
            scope.spawn(|| loop {
                let pos = next.fetch_add(1, Ordering::Relaxed);
                let Some(game) = games.get(pos) else { break };
                let r = native_game(bots, maps, game, opts);
                done.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((pos, r));
            });
        }
    });
    done.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// Play one native game and record it as a replay, so `why` and `dump` work on native games too.
/// The driving loop mirrors `ibc_sim::run_custom`; a test pins the two to the same final hash.
pub fn record_native(
    map_text: &str,
    seed: u64,
    bot_a: &str,
    bot_b: &str,
    max_ticks: u32,
) -> Result<Replay, String> {
    use ibc_engine::{Map, Match, Ruleset};
    let names = [bot_a, bot_b];
    for name in names {
        if !ibc_sim::BOTS.contains(&name) {
            return Err(format!(
                "unknown built-in bot '{name}' (known: {})",
                ibc_sim::BOTS.join(", ")
            ));
        }
    }
    let rules = Ruleset::builtin();
    let header_rules = ibc_replay::HeaderRules::of(&rules);
    let map = Map::parse(map_text)?;
    let info = |name: &str| BotInfo {
        name: name.to_string(),
        sha256: String::new(),
        // Native bots run unmetered and uncapped: no language row applies.
        fuel_multiplier: 1.0,
        language: String::new(),
        memory_multiplier: 1.0,
    };
    let ruleset_hash = hash_hex(rules.hash());
    let map_hash = hash_hex(map.hash());
    let mut m = Match::new(rules, map, seed);
    let mut writer = ReplayWriter::new(Header {
        engine_version: ibc_engine::ENGINE_VERSION.to_string(),
        ruleset_hash,
        map_hash,
        map_text: map_text.to_string(),
        seed,
        wasmtime_version: String::new(),
        abi_version: ibc_abi::ABI_VERSION,
        bots: [info(bot_a), info(bot_b)],
        result: MatchResult {
            winner: None,
            score: [0, 0],
            ticks: 0,
            finished: false,
        },
        team_size: m.unit_count() / 2,
        redacted: [false, false],
        rules: header_rules,
    });
    let mut bots: Vec<Option<Box<dyn ibc::Bot>>> = (0..m.unit_count()).map(|_| None).collect();
    while !m.finished() && m.tick() < max_ticks {
        for unit in m.turn_order() {
            let team = (m.unit_team(unit) & 1) as usize;
            let slot = &mut bots[unit as usize];
            ibc::native::with_host(&mut m, unit, || {
                slot.get_or_insert_with(|| {
                    ibc_sim::make_bot(names[team]).expect("name checked above")
                })
                .turn();
            });
        }
        writer.inputs(m.take_inputs());
        writer.draws(m.take_debug());
        m.step();
        writer.end_tick(m.state_hashes());
    }
    let result = MatchResult {
        winner: m.winner(),
        score: m.score(),
        ticks: m.tick(),
        finished: m.finished(),
    };
    Ok(writer.finish(result))
}

// ---- wasm ----

/// The runner binary: explicit path, `$IBC_BIN`, a local release build, or `ibc` on the PATH.
pub fn find_ibc(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Some(p) = std::env::var_os("IBC_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    for local in ["target/release/ibc.exe", "target/release/ibc"] {
        if Path::new(local).is_file() {
            return PathBuf::from(local);
        }
    }
    PathBuf::from("ibc")
}

fn ibc_output(ibc: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(ibc)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| {
            format!(
                "cannot run `{}` ({e}); pass --ibc <path> or set IBC_BIN",
                ibc.display()
            )
        })?;
    if !out.status.success() {
        return Err(format!(
            "`{} {}` failed ({})",
            ibc.display(),
            args.join(" "),
            out.status
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A warning when the runner was built from different rules than this harness: native and wasm games
/// would then be different games, and replays could not be re-simulated for details.
fn patch_warning(ibc: &Path) -> Result<Option<String>, String> {
    let text = ibc_output(ibc, &["version", "--json"])?;
    let v: Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("`ibc version --json`: {e}"))?;
    let ours = hash_hex(ibc_engine::Ruleset::builtin().hash());
    let theirs = v["ruleset_hash"].as_str().unwrap_or("");
    Ok((theirs != ours).then(|| {
        format!(
            "runner `{}` has ruleset {theirs} but this ibc-bench build has {ours}: rebuild both from the same \
             tree. Native and wasm results are NOT comparable and replay details are unavailable",
            ibc.display()
        )
    }))
}

/// The `.wasm` of a bot: the file itself, or `ibc build <dir>` for a bot directory.
fn resolve_wasm(ibc: &Path, bot: &Bot) -> Result<PathBuf, String> {
    let path = bot.path.as_ref().ok_or_else(|| {
        format!(
            "`{}` has no bot directory for the wasm backend (looked under --bots-dir); pass a path",
            bot.label
        )
    })?;
    if !path.is_dir() {
        return Ok(path.clone());
    }
    let dir = path.to_string_lossy();
    let out = ibc_output(ibc, &["build", &dir])?;
    let wasm = out
        .lines()
        .next()
        .map(str::trim)
        .filter(|l| l.ends_with(".wasm"));
    wasm.map(PathBuf::from)
        .ok_or_else(|| format!("`ibc build {dir}` did not print a wasm path"))
}

/// The `.wasm` of every bot the games use, resolved in parallel: `ibc build` of a bot directory takes
/// seconds even when nothing changed. The first error, in pool order, wins.
fn resolve_all(
    ibc: &Path,
    bots: &[Bot],
    games: &[Game],
) -> Result<BTreeMap<usize, PathBuf>, String> {
    let used: BTreeSet<usize> = games.iter().flat_map(|g| [g.a, g.b]).collect();
    let panicked = || Err("ibc build panicked".to_string());
    let built: Vec<(usize, Result<PathBuf, String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = used
            .iter()
            .map(|&i| (i, scope.spawn(move || resolve_wasm(ibc, &bots[i]))))
            .collect();
        handles
            .into_iter()
            .map(|(i, h)| (i, h.join().unwrap_or_else(|_| panicked())))
            .collect()
    });
    built.into_iter().map(|(i, r)| r.map(|p| (i, p))).collect()
}

fn fuel_of(team: &Value) -> FuelRec {
    let f = &team["fuel"];
    let n = |k: &str| f[k].as_u64().unwrap_or(0);
    FuelRec {
        total: n("total"),
        granted: n("granted"),
        max_per_tick: n("max_per_tick"),
        max_unit_turn: n("max_unit_turn"),
        turns: n("turns"),
        out_of_fuel_turns: n("out_of_fuel_turns"),
    }
}

/// Turn a `finished` event into a result: read `summary.json` (details come later, see `add_details`).
fn finish_wasm(r: &mut GameResult, event: &Value) -> Result<(), String> {
    let summary_path = event["summary_path"]
        .as_str()
        .ok_or("finished event without summary_path")?;
    let text = std::fs::read_to_string(summary_path).map_err(|e| format!("{summary_path}: {e}"))?;
    let summary: Value = serde_json::from_str(&text).map_err(|e| format!("{summary_path}: {e}"))?;
    r.apply_engine_summary(&summary["game"]);
    r.fuel = Some([fuel_of(&summary["teams"][0]), fuel_of(&summary["teams"][1])]);
    let faults = |i: usize| summary["teams"][i]["faults"].as_u64().unwrap_or(0) as u32;
    r.faults = [faults(0), faults(1)];
    r.replay = event["replay_path"].as_str().map(str::to_string);
    if summary["outcome"] == "unfinished" {
        return Err(format!("not finished after {} ticks", r.ticks));
    }
    Ok(())
}

/// Per-tick details of finished wasm games, by re-simulating their replays natively on `jobs` threads
/// (one replay costs about what the native game does, so doing them one by one would dominate a run).
fn add_details(results: &mut BTreeMap<usize, GameResult>, jobs: usize, warnings: &mut Vec<String>) {
    let todo: Vec<Mutex<&mut GameResult>> = results
        .values_mut()
        .filter(|r| r.ok())
        .map(Mutex::new)
        .collect();
    let next = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, todo.len().max(1)) {
            scope.spawn(|| loop {
                let Some(slot) = todo.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                let mut r = slot.lock().unwrap_or_else(|e| e.into_inner());
                let path = r.replay.clone().unwrap_or_default();
                let observed = std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| Replay::decode(&bytes))
                    .and_then(|replay| {
                        let mut c = Collector::new();
                        replay.simulate(|m| c.observe(m)).map(|_| c.finish())
                    });
                match observed {
                    Ok(d) => r.detail = Some(d),
                    Err(e) => failed
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((r.id, path, e)),
                }
            });
        }
    });
    let mut failed = failed.into_inner().unwrap_or_else(|e| e.into_inner());
    failed.sort();
    for (id, path, e) in failed {
        warnings.push(format!(
            "game {id} ({path}): no details, replay did not re-simulate: {e}"
        ));
    }
}

fn run_wasm(
    bots: &[Bot],
    maps: &[MapEntry],
    games: &[Game],
    opts: &RunOpts,
    warnings: &mut Vec<String>,
) -> Result<Vec<(usize, GameResult)>, String> {
    let ibc = find_ibc(opts.ibc.as_deref());
    warnings.extend(patch_warning(&ibc)?);
    let wasm_of = resolve_all(&ibc, bots, games)?;
    let games_dir = opts.out_dir.join("games");
    let mut lines = String::new();
    let mut out: BTreeMap<usize, GameResult> = BTreeMap::new();
    for (pos, g) in games.iter().enumerate() {
        let (a, b, map) = (&bots[g.a], &bots[g.b], &maps[g.map]);
        let job = json!({
            "id": pos, "bot_a": wasm_of[&g.a], "bot_b": wasm_of[&g.b], "map": map.path, "seed": g.seed,
            "out_dir": games_dir.join(format!("g{}", g.id)), "max_ticks": opts.max_ticks,
            "name_a": a.label, "name_b": b.label,
        });
        lines.push_str(&job.to_string());
        lines.push('\n');
        let err = "the runner produced no result for this game".to_string();
        out.insert(
            pos,
            GameResult::failed(g, &map.name, &a.label, &b.label, "wasm", err),
        );
    }

    let mut child = Command::new(&ibc)
        .args([
            "run-batch",
            "--jsonl",
            "--jobs",
            &opts.jobs.max(1).to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| {
            format!(
                "cannot run `{}` ({e}); pass --ibc <path> or set IBC_BIN",
                ibc.display()
            )
        })?;
    let mut stdin = child.stdin.take().ok_or("runner has no stdin")?;
    let stdout = child.stdout.take().ok_or("runner has no stdout")?;
    // Feed from a thread: the runner applies back-pressure on stdin while we must keep draining stdout.
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(lines.as_bytes());
    });
    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("reading runner output: {e}"));
            }
        };
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(r) = event["id"]
            .as_u64()
            .and_then(|pos| out.get_mut(&(pos as usize)))
        else {
            continue;
        };
        match event["event"].as_str() {
            Some("finished") => r.error = finish_wasm(r, &event).err(),
            Some("failed") => {
                r.error = Some(event["error"].as_str().unwrap_or("failed").to_string())
            }
            _ => {}
        }
    }
    let _ = feeder.join();
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        warnings.push(format!("runner exited with {status}"));
    }
    if opts.detail {
        add_details(&mut out, opts.jobs, warnings);
    }
    if !opts.keep_replays {
        for r in out.values_mut() {
            r.replay = None;
        }
        let _ = std::fs::remove_dir_all(&games_dir);
    }
    Ok(out.into_iter().collect())
}
