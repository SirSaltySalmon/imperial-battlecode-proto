//! `cargo xtask engine-wasm [--require-node] [replay.ibcr ...]`
//!
//! Builds the browser engine (`ibc-engine-wasm` for wasm32-unknown-unknown) and checks that replays load
//! with the native engine and, when `node` is available, with the wasm module instantiated in Node, with
//! the same frame count, the same last frame and the same bytes for every frame. This is the
//! native-vs-wasm engine equivalence check: the viewer must show exactly what the runner simulated.

use crate::util::{self, Args, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "wasm32-unknown-unknown";

#[derive(Debug, PartialEq, Eq)]
struct Loaded {
    frames: usize,
    last_frame_sha256: String,
    all_frames_sha256: String,
    header_sha256: String,
}

#[derive(Deserialize)]
struct NodeLine {
    engine: String,
    error: Option<String>,
    frames: Option<usize>,
    last_frame_sha256: Option<String>,
    all_frames_sha256: Option<String>,
    header_sha256: Option<String>,
}

fn load_native(path: &Path) -> Result<Loaded> {
    let (frames, header) = ibc_engine_wasm::load(&util::read(path)?)
        .map_err(|e| format!("{}: the native engine rejects it: {e}", util::rel(path)))?;
    let mut all = String::new();
    for f in &frames {
        all.push_str(f);
        all.push('\n');
    }
    Ok(Loaded {
        frames: frames.len(),
        last_frame_sha256: ibc_replay::sha256_hex(
            frames.last().map_or("", String::as_str).as_bytes(),
        ),
        all_frames_sha256: ibc_replay::sha256_hex(all.as_bytes()),
        header_sha256: ibc_replay::sha256_hex(header.as_bytes()),
    })
}

fn build() -> Result<PathBuf> {
    util::run(util::cargo().args([
        "build",
        "-p",
        "ibc-engine-wasm",
        "--release",
        "--target",
        TARGET,
    ]))?;
    let wasm = util::target_dir()
        .join(TARGET)
        .join("release/ibc_engine_wasm.wasm");
    let size = std::fs::metadata(&wasm)
        .map_err(|e| format!("{}: {e}", wasm.display()))?
        .len();
    println!("built {} ({} KiB)", util::rel(&wasm), size / 1024);
    Ok(wasm)
}

fn load_in_node(wasm: &Path, replays: &[PathBuf]) -> Result<Vec<Result<Loaded>>> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("js/engine_check.mjs");
    let out = util::capture(Command::new("node").arg(script).arg(wasm).args(replays))?;
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() != replays.len() {
        return Err(format!(
            "node printed {} result lines for {} replays",
            lines.len(),
            replays.len()
        ));
    }
    lines
        .iter()
        .map(|line| {
            let l: NodeLine =
                serde_json::from_str(line).map_err(|e| format!("node output {line:?}: {e}"))?;
            if l.engine != ibc_engine::ENGINE_VERSION {
                return Err(format!(
                    "the wasm reports engine {} but this checkout is {}",
                    l.engine,
                    ibc_engine::ENGINE_VERSION
                ));
            }
            Ok(
                match (
                    l.error,
                    l.frames,
                    l.last_frame_sha256,
                    l.all_frames_sha256,
                    l.header_sha256,
                ) {
                    (
                        None,
                        Some(frames),
                        Some(last_frame_sha256),
                        Some(all_frames_sha256),
                        Some(header_sha256),
                    ) => Ok(Loaded {
                        frames,
                        last_frame_sha256,
                        all_frames_sha256,
                        header_sha256,
                    }),
                    (error, ..) => {
                        Err(error.unwrap_or_else(|| "incomplete result line".to_string()))
                    }
                },
            )
        })
        .collect()
}

pub fn main(args: &[String]) -> Result<()> {
    let mut args = Args::new(args);
    let require_node = args.switch("--require-node");
    let mut replays: Vec<PathBuf> = args.rest()?.into_iter().map(PathBuf::from).collect();
    let fixture = util::root().join("fixtures/sample.ibcr");
    replays.insert(0, fixture);

    let wasm = build()?;
    let mut native = Vec::new();
    for r in &replays {
        let loaded = load_native(r)?;
        println!(
            "native  {}: {} frames, last frame sha256 {}",
            util::rel(r),
            loaded.frames,
            &loaded.last_frame_sha256[..16]
        );
        native.push(loaded);
    }

    if !util::has_program("node") {
        if require_node {
            return Err("node not found, and --require-node was given".to_string());
        }
        println!("node not found: the wasm module was built but NOT instantiated; only the native load was checked");
        return Ok(());
    }
    let mut failures = 0;
    for ((replay, native), node) in replays
        .iter()
        .zip(&native)
        .zip(load_in_node(&wasm, &replays)?)
    {
        match node {
            Ok(node) if node == *native => {
                println!(
                    "wasm    {}: {} frames, identical to native (every frame and the header)",
                    util::rel(replay),
                    node.frames
                );
            }
            Ok(node) => {
                failures += 1;
                println!(
                    "FAIL    {}: native {native:?}\n        wasm   {node:?}",
                    util::rel(replay)
                );
            }
            Err(e) => {
                failures += 1;
                println!(
                    "FAIL    {}: loads natively but the wasm engine rejects it: {e}",
                    util::rel(replay)
                );
            }
        }
    }
    if failures == 0 {
        println!(
            "engine-wasm OK: native and wasm engines agree on {} replay(s)",
            replays.len()
        );
        Ok(())
    } else {
        Err(format!(
            "engine-wasm FAILED: native and wasm engines disagree on {failures} replay(s)"
        ))
    }
}
