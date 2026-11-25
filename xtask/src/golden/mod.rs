//! Golden determinism: `cargo xtask golden record | verify [--native] [--wasm]`.
//!
//! `fixtures/golden/cases.json` lists (map, seed, bot pair) cases; it is the only file edited by hand.
//! `record` plays them and writes what happened; `verify` plays them again and reports the first tick at
//! which anything differs.
//!
//! - native (`native.json`, `native/*.hashes`): the engine driven by the reference bots linked as Rust
//!   (`ibc-sim`), no Wasmtime. Seconds to build and run; this is the check every PR gets.
//! - wasm (`wasm.json`, `wasm/*.hashes`, `bots/*.wasm`): `ibc run` with the pinned reference bot modules,
//!   compared down to the SHA-256 of the replay file, then again with freshly built bots, compared by
//!   state hash (a fresh module embeds host paths, so its bytes are not expected to be identical).
//!
//! A ruleset or mechanics change legitimately changes the goldens: run `record`, commit `fixtures/golden`,
//! and the PR diff shows the new hashes. See docs/release.md.

mod native;
mod wasm;

use crate::util::{self, Args, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn dir() -> PathBuf {
    util::root().join("fixtures/golden")
}

/// One case of `cases.json`. `a` and `b` are reference bot names (directories under `bots/`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    pub name: String,
    pub map: String,
    pub seed: u64,
    pub a: String,
    pub b: String,
}

#[derive(Deserialize)]
struct Cases {
    native: Vec<Case>,
    wasm: Vec<Case>,
}

/// What a match ended with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub finished: bool,
    pub winner: Option<i32>,
    pub score: [i32; 2],
    pub ticks: u32,
    pub final_state_hash: String,
}

/// A case together with its recorded outcome, as stored in `native.json` / `wasm.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    #[serde(flatten)]
    pub case: Case,
    #[serde(flatten)]
    pub outcome: Outcome,
    /// Wasm cases only: SHA-256 of the bytes of `replay.ibcr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_sha256: Option<String>,
}

/// (tick, state hash after that tick), in tick order.
pub type TickHashes = Vec<(u32, u64)>;

/// What one run of a case produced.
pub struct Observed {
    pub outcome: Outcome,
    pub hashes: TickHashes,
}

fn load_cases() -> Result<Cases> {
    let path = dir().join("cases.json");
    let cases: Cases = serde_json::from_str(&util::read_text(&path)?)
        .map_err(|e| format!("{}: {e}", util::rel(&path)))?;
    for list in [&cases.native, &cases.wasm] {
        for (i, c) in list.iter().enumerate() {
            let plain = !c.name.is_empty()
                && c.name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
            if !plain {
                return Err(format!(
                    "cases.json: case name {:?} must be letters, digits, '-' or '_'",
                    c.name
                ));
            }
            if list[..i].iter().any(|other| other.name == c.name) {
                return Err(format!("cases.json: duplicate case name {:?}", c.name));
            }
        }
    }
    Ok(cases)
}

pub fn hashes_to_text(hashes: &TickHashes) -> String {
    hashes
        .iter()
        .map(|(tick, hash)| format!("{tick} {hash:016x}\n"))
        .collect()
}

pub fn hashes_from_text(text: &str, what: &str) -> Result<TickHashes> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let (tick, hash) = line
                .trim()
                .split_once(' ')
                .ok_or_else(|| format!("{what}: bad line {line:?}"))?;
            let tick = tick
                .parse()
                .map_err(|_| format!("{what}: bad tick in {line:?}"))?;
            Ok((
                tick,
                ibc_replay::parse_hash_hex(hash).map_err(|e| format!("{what}: {e}"))?,
            ))
        })
        .collect()
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    util::write(path, text.as_bytes())
}

pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "{}: {e}. Record the goldens first: cargo xtask golden record",
            util::rel(path)
        )
    })?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", util::rel(path)))
}

/// Remove `*.hashes` files in `dir` that no case of this recording owns.
pub fn prune_hashes(dir: &Path, keep: &[Case]) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let stale = path.extension().is_some_and(|x| x == "hashes")
            && !keep.iter().any(|c| {
                path.file_stem()
                    .is_some_and(|s| s.to_string_lossy() == c.name)
            });
        if stale {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// The state part of a comparison: where the per-tick hashes first differ, then the outcome fields.
/// Returns one line per difference; empty means identical.
pub fn compare_state(
    expected: &Outcome,
    expected_hashes: &TickHashes,
    got: &Observed,
) -> Vec<String> {
    let mut out = Vec::new();
    let first = expected_hashes
        .iter()
        .zip(&got.hashes)
        .position(|(e, g)| e != g);
    if let Some(i) = first {
        let ((tick, want), (got_tick, have)) = (expected_hashes[i], got.hashes[i]);
        if tick != got_tick {
            out.push(format!(
                "tick numbering differs: expected tick {tick}, got tick {got_tick}"
            ));
        } else if tick == 0 {
            out.push(format!(
                "the INITIAL state differs (tick 0: expected {want:016x}, got {have:016x}): map, ruleset or spawn setup changed"
            ));
        } else if i == 0 {
            // Replays carry no hash of the initial state, so the first tick is the first thing compared.
            out.push(format!(
                "state differs at tick {tick}, the first one compared (expected {want:016x}, got {have:016x}): \
                 the initial state or the first tick changed"
            ));
        } else {
            out.push(format!(
                "state diverged at tick {tick}: expected {want:016x}, got {have:016x} (identical through tick {})",
                expected_hashes[i - 1].0
            ));
        }
    } else if expected_hashes.len() != got.hashes.len() {
        let common = expected_hashes.len().min(got.hashes.len());
        let through = expected_hashes[..common]
            .last()
            .map_or("no tick".to_string(), |(t, _)| format!("tick {t}"));
        out.push(format!(
            "state identical through {through}, but the match has {} recorded ticks and this run has {}",
            expected_hashes.len(),
            got.hashes.len()
        ));
    }
    let (e, g) = (expected, &got.outcome);
    if e.ticks != g.ticks {
        out.push(format!("ticks: expected {}, got {}", e.ticks, g.ticks));
    }
    if e.score != g.score {
        out.push(format!(
            "score: expected {}-{}, got {}-{}",
            e.score[0], e.score[1], g.score[0], g.score[1]
        ));
    }
    if e.winner != g.winner {
        out.push(format!(
            "winner: expected {:?}, got {:?}",
            e.winner, g.winner
        ));
    }
    if e.finished != g.finished {
        out.push(format!(
            "finished: expected {}, got {}",
            e.finished, g.finished
        ));
    }
    if e.final_state_hash != g.final_state_hash {
        out.push(format!(
            "final state hash: expected {}, got {}",
            e.final_state_hash, g.final_state_hash
        ));
    }
    out
}

/// Collects the result of a verify run and prints it case by case.
#[derive(Default)]
pub struct Report {
    pub checked: usize,
    pub failed: usize,
    /// Problems that are not about one case (patch identity, case list, stale binary).
    pub general: Vec<String>,
}

impl Report {
    pub fn case(&mut self, kind: &str, case: &Case, outcome: &Outcome, problems: &[String]) {
        self.checked += 1;
        let what = format!(
            "{kind} {} ({} vs {} on {}, seed {})",
            case.name, case.a, case.b, case.map, case.seed
        );
        if problems.is_empty() {
            println!(
                "ok    {what}: {} ticks, score {}-{}, final hash {}",
                outcome.ticks, outcome.score[0], outcome.score[1], outcome.final_state_hash
            );
        } else {
            self.failed += 1;
            println!("FAIL  {what}");
            for p in problems {
                println!("        {p}");
            }
        }
    }

    pub fn problem(&mut self, message: String) {
        println!("FAIL  {message}");
        self.general.push(message);
    }

    pub fn ok(&self) -> bool {
        self.failed == 0 && self.general.is_empty()
    }
}

const USAGE: &str = "usage: cargo xtask golden record|verify [--native] [--wasm] [--no-fresh]
  record       play every case and rewrite fixtures/golden (commit the result; the PR diff shows the new hashes)
  verify       play every case and report the first tick that differs from fixtures/golden
  --native     only the native cases (engine + reference bots as Rust, no Wasmtime)
  --wasm       only the wasm cases (`ibc run`; uses $IBC_BIN, else builds target/release/ibc)
  --no-fresh   verify: skip the second pass with freshly built bots";

/// `cargo xtask golden ...`
pub fn main(args: &[String]) -> Result<()> {
    let mut args = Args::new(args);
    let (only_native, only_wasm, no_fresh) = (
        args.switch("--native"),
        args.switch("--wasm"),
        args.switch("--no-fresh"),
    );
    let (do_native, do_wasm) = (only_native || !only_wasm, only_wasm || !only_native);
    let rest = args.rest().map_err(|e| format!("{e}\n{USAGE}"))?;
    let cases = load_cases()?;
    match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["record"] => {
            if do_native {
                native::record(&cases.native)?;
            }
            if do_wasm {
                wasm::record(&cases.wasm)?;
            }
            println!("goldens recorded in fixtures/golden. Review `git diff --stat fixtures/golden` and commit it.");
            Ok(())
        }
        ["verify"] => {
            let mut report = Report::default();
            if do_native {
                native::verify(&cases.native, &mut report)?;
            }
            if do_wasm {
                wasm::verify(&cases.wasm, !no_fresh, &mut report)?;
            }
            if report.ok() {
                println!("golden verify OK: {} checks", report.checked);
                return Ok(());
            }
            Err(format!(
                "golden verify FAILED: {} of {} checks differ{}.\n  \
                 Changed rules, mechanics, bots, the toolchain or Wasmtime on purpose? Then this is expected:\n    \
                 cargo xtask golden record     # then commit fixtures/golden and say so in the PR\n  \
                 Did not? Then behaviour changed by accident, or the same match no longer gives the same\n  \
                 result (a determinism bug). Find the cause; do not re-record.",
                report.failed,
                report.checked,
                if report.general.is_empty() { String::new() } else { format!(", plus {} general problem(s)", report.general.len()) }
            ))
        }
        _ => Err(USAGE.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(ticks: u32, hash: u64) -> Outcome {
        Outcome {
            finished: true,
            winner: Some(0),
            score: [7, 3],
            ticks,
            final_state_hash: format!("{hash:016x}"),
        }
    }

    fn run(hashes: &[u64]) -> (Outcome, TickHashes) {
        let list: TickHashes = hashes
            .iter()
            .enumerate()
            .map(|(i, h)| (i as u32, *h))
            .collect();
        (
            outcome(hashes.len() as u32 - 1, *hashes.last().unwrap()),
            list,
        )
    }

    #[test]
    fn identical_runs_have_no_differences() {
        let (o, h) = run(&[1, 2, 3]);
        assert!(compare_state(
            &o,
            &h,
            &Observed {
                outcome: o.clone(),
                hashes: h.clone()
            }
        )
        .is_empty());
    }

    #[test]
    fn the_first_diverging_tick_is_named() {
        let (o, h) = run(&[1, 2, 3, 4]);
        let (o2, h2) = run(&[1, 2, 9, 8]);
        let diff = compare_state(
            &o,
            &h,
            &Observed {
                outcome: o2,
                hashes: h2,
            },
        );
        assert!(diff[0].starts_with("state diverged at tick 2:"), "{diff:?}");
        assert!(diff[0].contains("identical through tick 1"));
        assert!(diff.iter().any(|d| d.starts_with("final state hash")));
    }

    #[test]
    fn a_different_initial_state_and_a_different_length_are_told_apart() {
        let (o, h) = run(&[1, 2, 3]);
        let (o2, h2) = run(&[7, 2, 3]);
        assert!(compare_state(
            &o,
            &h,
            &Observed {
                outcome: o2,
                hashes: h2
            }
        )[0]
        .contains("INITIAL state"));
        let (o3, h3) = run(&[1, 2, 3, 4]);
        let diff = compare_state(
            &o,
            &h,
            &Observed {
                outcome: o3,
                hashes: h3,
            },
        );
        assert!(
            diff[0].starts_with("state identical through tick 2, but"),
            "{diff:?}"
        );
        assert!(diff.iter().any(|d| d == "ticks: expected 2, got 3"));
    }

    #[test]
    fn a_replay_that_differs_at_its_first_tick_does_not_claim_tick_zero_matched() {
        let expected: TickHashes = vec![(1, 10), (2, 20)];
        let got = Observed {
            outcome: outcome(2, 20),
            hashes: vec![(1, 11), (2, 20)],
        };
        let diff = compare_state(&outcome(2, 20), &expected, &got);
        assert!(
            diff[0].starts_with("state differs at tick 1, the first one compared"),
            "{diff:?}"
        );
        assert!(!diff[0].contains("identical through"));
    }

    #[test]
    fn tick_hashes_round_trip_as_text() {
        let hashes: TickHashes = vec![(0, 0xdead_beef), (1, u64::MAX)];
        let text = hashes_to_text(&hashes);
        assert_eq!(text, "0 00000000deadbeef\n1 ffffffffffffffff\n");
        assert_eq!(hashes_from_text(&text, "t").unwrap(), hashes);
        assert!(hashes_from_text("nonsense\n", "t").is_err());
    }

    #[test]
    fn recorded_cases_are_flat_json() {
        let case = Case {
            name: "c".into(),
            map: "maps/m.ibcmap".into(),
            seed: 1,
            a: "x".into(),
            b: "y".into(),
        };
        let rec = Recorded {
            case,
            outcome: outcome(5, 1),
            replay_sha256: None,
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["name"], "c");
        assert_eq!(json["ticks"], 5);
        assert!(json.get("replay_sha256").is_none());
        assert_eq!(serde_json::from_value::<Recorded>(json).unwrap(), rec);
    }
}
