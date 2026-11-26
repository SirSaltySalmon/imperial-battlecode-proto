//! Wasm goldens: the same cases through `ibc run` (Wasmtime, fuel metering, the replay writer).
//!
//! The pinned modules in `fixtures/golden/bots` make the inputs byte-identical on every machine, so the
//! SHA-256 of the replay file must be too: that is the cross-platform determinism proof. A second pass
//! builds the reference bots from source and requires the same state hashes, which catches SDK, bot and
//! toolchain changes that alter behaviour.

use super::{compare_state, Case, Observed, Outcome, Recorded, Report, TickHashes};
use crate::patch_id::{self, PatchId};
use crate::util::{self, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Deserialize)]
struct WasmGolden {
    patch: PatchId,
    /// SHA-256 of each pinned module in `fixtures/golden/bots`.
    bots: BTreeMap<String, String>,
    cases: Vec<Recorded>,
}

/// The `ibc` binary under test: `$IBC_BIN`, else a release build of this checkout.
pub fn find_ibc() -> Result<PathBuf> {
    if let Some(bin) = std::env::var_os("IBC_BIN").filter(|v| !v.is_empty()) {
        // Absolute: the binary is started from the repository root, not from the caller's directory.
        let bin = std::path::absolute(PathBuf::from(bin)).map_err(|e| format!("IBC_BIN: {e}"))?;
        return if bin.is_file() {
            Ok(bin)
        } else {
            Err(format!("IBC_BIN={}: no such file", bin.display()))
        };
    }
    util::run(util::cargo().args(["build", "--release", "-p", "ibc-cli"]))?;
    let bin = util::target_dir()
        .join("release")
        .join(util::exe_name("ibc", cfg!(windows)));
    if bin.is_file() {
        Ok(bin)
    } else {
        Err(format!(
            "{}: not found after building ibc-cli",
            bin.display()
        ))
    }
}

fn pinned_dir() -> PathBuf {
    super::dir().join("bots")
}

fn pinned_bot(bot: &str) -> PathBuf {
    pinned_dir().join(format!("{bot}.wasm"))
}

fn hashes_path(case: &Case) -> PathBuf {
    super::dir()
        .join("wasm")
        .join(format!("{}.hashes", case.name))
}

fn bots_of(cases: &[Case]) -> BTreeSet<String> {
    cases
        .iter()
        .flat_map(|c| [c.a.clone(), c.b.clone()])
        .collect()
}

/// `ibc build bots/<bot>`: the module path and its SHA-256.
fn build_bot(ibc: &Path, bot: &str) -> Result<(PathBuf, String)> {
    let dir = util::root().join("bots").join(bot);
    if !dir.is_dir() {
        return Err(format!("bots/{bot}: no such reference bot"));
    }
    let out = util::capture(
        Command::new(ibc)
            .current_dir(util::root())
            .arg("build")
            .arg(&dir),
    )?;
    let line = out
        .lines()
        .next()
        .ok_or_else(|| format!("`ibc build bots/{bot}` printed nothing"))?;
    let path = util::root().join(line.trim());
    let sha = ibc_replay::sha256_hex(&util::read(&path)?);
    Ok((path, sha))
}

struct WasmRun {
    observed: Observed,
    replay_sha256: String,
    /// Problems with the artifacts themselves, independent of any golden.
    self_check: Vec<String>,
}

/// Run one case through `ibc run` and read the result back from the replay it wrote.
fn play(ibc: &Path, case: &Case, bot_a: &Path, bot_b: &Path, out_dir: &Path) -> Result<WasmRun> {
    // A stale replay must never be mistaken for the result of this run.
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    }
    let mut cmd = Command::new(ibc);
    cmd.current_dir(util::root())
        .arg("run")
        .arg(bot_a)
        .arg(bot_b);
    cmd.arg("--map")
        .arg(&case.map)
        .arg("--seed")
        .arg(case.seed.to_string())
        .arg("-o")
        .arg(out_dir);
    util::capture(&mut cmd).map_err(|e| format!("case {}: {e}", case.name))?;

    let bytes = util::read(&out_dir.join("replay.ibcr"))?;
    let replay_sha256 = ibc_replay::sha256_hex(&bytes);
    let replay = ibc_replay::Replay::decode(&bytes)
        .map_err(|e| format!("case {}: replay does not decode: {e}", case.name))?;
    let hashes: TickHashes = replay
        .ticks
        .iter()
        .enumerate()
        .map(|(i, t)| (i as u32 + 1, t.state_hash()))
        .collect();
    let result = &replay.header.result;
    let outcome = Outcome {
        finished: result.finished,
        winner: result.winner,
        score: result.score,
        ticks: result.ticks,
        final_state_hash: hashes
            .last()
            .map_or_else(String::new, |(_, h)| ibc_replay::hash_hex(*h)),
    };

    let mut self_check = Vec::new();
    if let Err(e) = ibc_replay::verify(&replay) {
        self_check.push(format!(
            "the native engine of this checkout does not reproduce the replay `ibc run` wrote: {e}"
        ));
    }
    let summary: serde_json::Value =
        serde_json::from_slice(&util::read(&out_dir.join("summary.json"))?)
            .map_err(|e| format!("case {}: summary.json: {e}", case.name))?;
    if summary["replay_sha256"].as_str() != Some(replay_sha256.as_str()) {
        self_check.push(format!(
            "summary.json says replay_sha256 {} but replay.ibcr hashes to {replay_sha256}",
            summary["replay_sha256"]
        ));
    }
    Ok(WasmRun {
        observed: Observed { outcome, hashes },
        replay_sha256,
        self_check,
    })
}

fn out_dir(pass: &str, case: &Case) -> PathBuf {
    util::root().join("out/golden").join(pass).join(&case.name)
}

fn play_pinned(ibc: &Path, case: &Case) -> Result<WasmRun> {
    // Relative paths: nothing about this machine reaches the command line of the run.
    let bot = |name: &str| PathBuf::from(format!("fixtures/golden/bots/{name}.wasm"));
    play(
        ibc,
        case,
        &bot(&case.a),
        &bot(&case.b),
        &out_dir("wasm", case),
    )
}

fn play_fresh(
    ibc: &Path,
    case: &Case,
    fresh: &BTreeMap<String, (PathBuf, String)>,
) -> Result<WasmRun> {
    play(
        ibc,
        case,
        &fresh[&case.a].0,
        &fresh[&case.b].0,
        &out_dir("fresh", case),
    )
}

fn same_state(a: &Observed, b: &Observed) -> bool {
    a.outcome == b.outcome && a.hashes == b.hashes
}

/// True if every case ends in the same state with the pinned modules as with freshly built bots.
/// A pinned module that is missing or can no longer run counts as "behaves differently".
fn pinned_still_match(
    ibc: &Path,
    cases: &[Case],
    fresh: &BTreeMap<String, (PathBuf, String)>,
) -> Result<bool> {
    if !bots_of(cases).iter().all(|b| pinned_bot(b).is_file()) {
        return Ok(false);
    }
    for case in cases {
        let pinned = match play_pinned(ibc, case) {
            Ok(run) if run.self_check.is_empty() => run,
            Ok(_) | Err(_) => return Ok(false),
        };
        if !same_state(&pinned.observed, &play_fresh(ibc, case, fresh)?.observed) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Keep the pinned modules when freshly built bots still behave exactly like them (a module embeds host
/// paths, so re-pinning on every record would churn committed binaries for no reason); otherwise rewrite
/// them from the fresh build. Modules no case uses are removed. Returns the SHA-256 of each pinned module.
fn pin_bots(ibc: &Path, cases: &[Case]) -> Result<BTreeMap<String, String>> {
    let bots = bots_of(cases);
    let mut fresh = BTreeMap::new();
    for bot in &bots {
        fresh.insert(bot.clone(), build_bot(ibc, bot)?);
    }
    if pinned_still_match(ibc, cases, &fresh)? {
        println!("pinned bot modules kept: freshly built bots behave identically in every case");
    } else {
        for (bot, (path, _)) in &fresh {
            util::write(&pinned_bot(bot), &util::read(path)?)?;
        }
        println!(
            "pinned bot modules rewritten from a fresh build: {}",
            bots.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    if let Ok(entries) = std::fs::read_dir(pinned_dir()) {
        for path in entries.flatten().map(|e| e.path()) {
            let used = path
                .file_stem()
                .is_some_and(|s| bots.contains(&*s.to_string_lossy()));
            if path.extension().is_some_and(|x| x == "wasm") && !used {
                std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
    }
    let mut shas = BTreeMap::new();
    for bot in &bots {
        shas.insert(
            bot.clone(),
            ibc_replay::sha256_hex(&util::read(&pinned_bot(bot))?),
        );
    }
    Ok(shas)
}

pub fn record(cases: &[Case]) -> Result<()> {
    let ibc = find_ibc()?;
    let patch = patch_id::check_binary(&ibc)?;
    let bots = pin_bots(&ibc, cases)?;

    let mut recorded = Vec::new();
    for case in cases {
        let run = play_pinned(&ibc, case)?;
        if let Some(problem) = run.self_check.first() {
            return Err(format!("case {}: {problem}", case.name));
        }
        if !run.observed.outcome.finished {
            return Err(format!(
                "case {}: the match did not finish; a golden must be a complete match",
                case.name
            ));
        }
        util::write(
            &hashes_path(case),
            super::hashes_to_text(&run.observed.hashes).as_bytes(),
        )?;
        println!(
            "recorded wasm {}: {} ticks, final hash {}, replay sha256 {}",
            case.name,
            run.observed.outcome.ticks,
            run.observed.outcome.final_state_hash,
            run.replay_sha256
        );
        recorded.push(Recorded {
            case: case.clone(),
            outcome: run.observed.outcome,
            replay_sha256: Some(run.replay_sha256),
        });
    }
    super::prune_hashes(&super::dir().join("wasm"), cases)?;
    super::write_json(
        &super::dir().join("wasm.json"),
        &WasmGolden {
            patch,
            bots,
            cases: recorded,
        },
    )
}

pub fn verify(cases: &[Case], fresh_pass: bool, report: &mut Report) -> Result<()> {
    let golden: WasmGolden = super::read_json(&super::dir().join("wasm.json"))?;
    let ibc = find_ibc()?;
    let binary = patch_id::of_binary(&ibc)?;
    println!("ibc under test: {} ({})", ibc.display(), binary.one_line());
    let stale = patch_id::source()?.diff(&binary);
    if !stale.is_empty() {
        report.problem(format!(
            "{} was not built from this checkout (source -> binary): {}. Rebuild it or unset IBC_BIN",
            ibc.display(),
            stale.join("; ")
        ));
    }
    let moved = golden.patch.diff(&binary);
    if !moved.is_empty() {
        report.problem(format!(
            "wasm goldens were recorded for another patch (recorded -> this binary): {}",
            moved.join("; ")
        ));
    }
    // Only cases.json is validated, and case names become directories: never play an unlisted case.
    if golden.cases.iter().map(|r| &r.case).collect::<Vec<_>>() != cases.iter().collect::<Vec<_>>()
    {
        report.problem("fixtures/golden/cases.json and wasm.json list different wasm cases; nothing was played".to_string());
        return Ok(());
    }
    for (bot, want) in &golden.bots {
        let have = ibc_replay::sha256_hex(&util::read(&pinned_bot(bot))?);
        if &have != want {
            report.problem(format!(
                "fixtures/golden/bots/{bot}.wasm has sha256 {have}, wasm.json says {want}"
            ));
        }
    }

    for rec in &golden.cases {
        let text = util::read_text(&hashes_path(&rec.case))?;
        let expected_hashes = super::hashes_from_text(&text, &util::rel(&hashes_path(&rec.case)))?;
        let run = play_pinned(&ibc, &rec.case)?;
        let mut problems = compare_state(&rec.outcome, &expected_hashes, &run.observed);
        let want_sha = rec.replay_sha256.as_deref().unwrap_or("(none recorded)");
        if problems.is_empty() && want_sha != run.replay_sha256 {
            problems.push(format!(
                "every state hash matches but the replay bytes differ: replay_sha256 expected {want_sha}, got {}. \
                 Something outside the simulation changed (replay format, header fields, input encoding)",
                run.replay_sha256
            ));
        }
        problems.extend(run.self_check);
        report.case("wasm", &rec.case, &run.observed.outcome, &problems);
    }
    if fresh_pass {
        verify_fresh(&ibc, &golden, report)?;
    }
    Ok(())
}

/// Second pass: reference bots built from source must reach the recorded state hashes.
fn verify_fresh(ibc: &Path, golden: &WasmGolden, report: &mut Report) -> Result<()> {
    let mut fresh = BTreeMap::new();
    for bot in golden.bots.keys() {
        fresh.insert(bot.clone(), build_bot(ibc, bot)?);
    }
    let differs = |bot: &String| golden.bots.get(bot) != fresh.get(bot).map(|(_, sha)| sha);
    if !golden.bots.keys().any(differs) {
        println!("ok    freshly built bots are byte-identical to the pinned modules");
        return Ok(());
    }
    println!("note: freshly built bot modules differ in bytes from the pinned ones (they embed host paths); comparing state hashes");
    for rec in &golden.cases {
        if !differs(&rec.case.a) && !differs(&rec.case.b) {
            continue;
        }
        let text = util::read_text(&hashes_path(&rec.case))?;
        let expected_hashes = super::hashes_from_text(&text, &util::rel(&hashes_path(&rec.case)))?;
        let run = play_fresh(ibc, &rec.case, &fresh)?;
        let mut problems = compare_state(&rec.outcome, &expected_hashes, &run.observed);
        if !problems.is_empty() {
            problems.push(
                "the pinned modules still match, so the bots, the SDK or the toolchain changed behaviour (or this \
                 host compiles them differently)"
                    .to_string(),
            );
        }
        problems.extend(run.self_check);
        report.case(
            "wasm, fresh bots",
            &rec.case,
            &run.observed.outcome,
            &problems,
        );
    }
    Ok(())
}
