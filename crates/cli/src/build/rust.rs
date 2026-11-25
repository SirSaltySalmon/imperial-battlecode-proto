//! Rust bots: `cargo build --target wasm32-wasip1` in the bot directory.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const TARGET: &str = "wasm32-wasip1";

/// Build the crate in `dir` and return the path of the wasm that cargo produced.
pub fn build(dir: &Path, release: bool) -> Result<PathBuf> {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(dir).args(["build", "--target", TARGET, "--message-format=json-render-diagnostics"]);
    if release {
        cmd.arg("--release");
    }
    // A bot is its own workspace: do not let the caller's cargo settings redirect or reconfigure the build.
    for var in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "CARGO_BUILD_TARGET_DIR", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"] {
        cmd.env_remove(var);
    }
    // stdout carries cargo's JSON messages; diagnostics and progress go straight to our stderr.
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running cargo in {} (is Rust installed?)", dir.display()))?;
    if !out.status.success() {
        bail!(
            "cargo build failed in {} ({}). If the target is missing: rustup target add {TARGET}",
            dir.display(),
            out.status
        );
    }
    // The last binary artifact that is a .wasm file is the bot.
    let mut wasm = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if msg["reason"] != "compiler-artifact" {
            continue;
        }
        let is_bin = msg["target"]["kind"].as_array().is_some_and(|k| k.iter().any(|v| v == "bin"));
        for f in msg["filenames"].as_array().into_iter().flatten() {
            if let Some(path) = f.as_str().filter(|p| p.ends_with(".wasm")) {
                if is_bin {
                    wasm = Some(PathBuf::from(path));
                }
            }
        }
    }
    match wasm {
        Some(p) => Ok(p),
        None => bail!(
            "cargo built {} but produced no wasm binary: the bot crate needs a `src/main.rs` (a command module with `_start`)",
            dir.display()
        ),
    }
}
