//! Native goldens: the engine driven by the reference bots linked as Rust (`ibc-sim`), in this process.

use super::{compare_state, Case, Observed, Outcome, Recorded, Report, TickHashes};
use crate::util::{self, Result};
use serde::{Deserialize, Serialize};

/// The part of the patch identity that the native path depends on (no Wasmtime).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct NativePatch {
    engine: String,
    ruleset_hash: String,
    abi: u32,
}

#[derive(Serialize, Deserialize)]
struct NativeGolden {
    patch: NativePatch,
    cases: Vec<Recorded>,
}

fn current_patch() -> NativePatch {
    NativePatch {
        engine: ibc_engine::ENGINE_VERSION.to_string(),
        ruleset_hash: ibc_replay::hash_hex(ibc_engine::Ruleset::builtin().hash()),
        abi: ibc_abi::ABI_VERSION,
    }
}

fn play(case: &Case) -> Result<Observed> {
    let map_text = util::read_text(&util::root().join(&case.map))?;
    let mut hashes = TickHashes::new();
    let r = ibc_sim::run_with(
        &map_text,
        case.seed,
        &case.a,
        &case.b,
        ibc_sim::DEFAULT_MAX_TICKS,
        |m| hashes.push((m.tick(), m.state_hash())),
    )
    .map_err(|e| format!("case {}: {e}", case.name))?;
    let outcome = Outcome {
        finished: r.finished,
        winner: r.winner,
        score: r.score,
        ticks: r.ticks,
        final_state_hash: ibc_replay::hash_hex(r.final_hash),
    };
    Ok(Observed { outcome, hashes })
}

fn hashes_path(case: &Case) -> std::path::PathBuf {
    super::dir()
        .join("native")
        .join(format!("{}.hashes", case.name))
}

pub fn record(cases: &[Case]) -> Result<()> {
    let mut recorded = Vec::new();
    for case in cases {
        let run = play(case)?;
        if !run.outcome.finished {
            return Err(format!(
                "case {}: the match did not finish; a golden must be a complete match",
                case.name
            ));
        }
        util::write(
            &hashes_path(case),
            super::hashes_to_text(&run.hashes).as_bytes(),
        )?;
        println!(
            "recorded native {}: {} ticks, final hash {}",
            case.name, run.outcome.ticks, run.outcome.final_state_hash
        );
        recorded.push(Recorded {
            case: case.clone(),
            outcome: run.outcome,
            replay_sha256: None,
        });
    }
    super::prune_hashes(&super::dir().join("native"), cases)?;
    super::write_json(
        &super::dir().join("native.json"),
        &NativeGolden {
            patch: current_patch(),
            cases: recorded,
        },
    )
}

pub fn verify(cases: &[Case], report: &mut Report) -> Result<()> {
    let golden: NativeGolden = super::read_json(&super::dir().join("native.json"))?;
    let now = current_patch();
    if golden.patch != now {
        let mut changed = Vec::new();
        if golden.patch.ruleset_hash != now.ruleset_hash {
            changed.push(format!(
                "ruleset hash {} -> {}",
                golden.patch.ruleset_hash, now.ruleset_hash
            ));
        }
        if golden.patch.engine != now.engine {
            changed.push(format!("engine {} -> {}", golden.patch.engine, now.engine));
        }
        if golden.patch.abi != now.abi {
            changed.push(format!("abi {} -> {}", golden.patch.abi, now.abi));
        }
        report.problem(format!(
            "native goldens were recorded for another patch (recorded -> this checkout): {}",
            changed.join("; ")
        ));
    }
    let listed: Vec<&Case> = golden.cases.iter().map(|r| &r.case).collect();
    if listed != cases.iter().collect::<Vec<_>>() {
        report.problem("fixtures/golden/cases.json and native.json list different native cases; nothing was played".to_string());
        return Ok(());
    }
    for rec in &golden.cases {
        let text = util::read_text(&hashes_path(&rec.case))?;
        let expected_hashes = super::hashes_from_text(&text, &util::rel(&hashes_path(&rec.case)))?;
        let run = play(&rec.case)?;
        let problems = compare_state(&rec.outcome, &expected_hashes, &run);
        report.case("native", &rec.case, &run.outcome, &problems);
    }
    Ok(())
}
