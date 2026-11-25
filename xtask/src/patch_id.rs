//! The patch identity: engine version + ruleset hash + Wasmtime version + ABI version.
//!
//! Engine, ruleset and ABI come from the crates this task is linked with. The Wasmtime version is read
//! from the sources instead, so that this crate never has to compile Wasmtime; the three places that
//! state it (the constant, the `=` pin and the lockfile) must agree.

use crate::util::{self, Args, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchId {
    pub engine: String,
    pub ruleset_hash: String,
    pub wasmtime: String,
    pub abi: u32,
}

impl PatchId {
    pub fn one_line(&self) -> String {
        format!(
            "engine {} / ruleset {} / wasmtime {} / abi {}",
            self.engine, self.ruleset_hash, self.wasmtime, self.abi
        )
    }

    /// The fields that differ from `other`, as `name: self -> other`.
    pub fn diff(&self, other: &PatchId) -> Vec<String> {
        let mut out = Vec::new();
        let mut field = |name: &str, a: String, b: String| {
            if a != b {
                out.push(format!("{name}: {a} -> {b}"));
            }
        };
        field("engine", self.engine.clone(), other.engine.clone());
        field(
            "ruleset hash",
            self.ruleset_hash.clone(),
            other.ruleset_hash.clone(),
        );
        field("wasmtime", self.wasmtime.clone(), other.wasmtime.clone());
        field("abi", self.abi.to_string(), other.abi.to_string());
        out
    }
}

/// The text between the first pair of double quotes after `marker`.
fn quoted_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let rest = &rest[rest.find('"')? + 1..];
    Some(&rest[..rest.find('"')?])
}

/// The Wasmtime version, checked for agreement between constant, manifest pin and lockfile.
pub fn wasmtime_version(root: &Path) -> Result<String> {
    let lib = util::read_text(&root.join("crates/runner/src/lib.rs"))?;
    let manifest = util::read_text(&root.join("crates/runner/Cargo.toml"))?;
    let lock = util::read_text(&root.join("Cargo.lock"))?;
    wasmtime_version_from(&lib, &manifest, &lock)
}

fn wasmtime_version_from(lib: &str, manifest: &str, lock: &str) -> Result<String> {
    let constant = quoted_after(lib, "const WASMTIME_VERSION")
        .ok_or("crates/runner/src/lib.rs: WASMTIME_VERSION constant not found")?;
    let pin = manifest
        .lines()
        .find(|l| {
            l.trim_start().starts_with("wasmtime ") || l.trim_start().starts_with("wasmtime=")
        })
        .and_then(|l| quoted_after(l, "version").or_else(|| quoted_after(l, "=")))
        .ok_or("crates/runner/Cargo.toml: wasmtime dependency not found")?;
    let Some(pinned) = pin.strip_prefix('=') else {
        return Err(format!(
            "crates/runner/Cargo.toml: wasmtime must be pinned exactly (\"={constant}\"), found \"{pin}\""
        ));
    };
    let locked = lock
        .replace("\r\n", "\n")
        .split("[[package]]")
        .find(|p| p.trim_start().starts_with("name = \"wasmtime\"\n"))
        .and_then(|p| quoted_after(p, "version").map(String::from))
        .ok_or("Cargo.lock: package wasmtime not found")?;
    if constant != pinned || constant != locked {
        return Err(format!(
            "Wasmtime version disagrees: WASMTIME_VERSION says {constant}, crates/runner/Cargo.toml pins \
             {pinned}, Cargo.lock has {locked}. Fuel counts depend on the exact version; bump all three together."
        ));
    }
    Ok(constant.to_string())
}

/// The patch identity of the sources in this checkout.
pub fn source() -> Result<PatchId> {
    Ok(PatchId {
        engine: ibc_engine::ENGINE_VERSION.to_string(),
        ruleset_hash: ibc_replay::hash_hex(ibc_engine::Ruleset::builtin().hash()),
        wasmtime: wasmtime_version(&util::root())?,
        abi: ibc_abi::ABI_VERSION,
    })
}

/// The patch identity an `ibc` binary reports (`ibc version --json`).
pub fn of_binary(ibc: &Path) -> Result<PatchId> {
    let text = util::capture(Command::new(ibc).args(["version", "--json"]))?;
    serde_json::from_str(&text).map_err(|e| format!("`{} version --json`: {e}", ibc.display()))
}

/// Error unless the binary was built from sources with the same patch identity as this checkout.
pub fn check_binary(ibc: &Path) -> Result<PatchId> {
    let (src, bin) = (source()?, of_binary(ibc)?);
    let diff = src.diff(&bin);
    if diff.is_empty() {
        Ok(src)
    } else {
        Err(format!(
            "{} was not built from these sources (source -> binary): {}. Rebuild it: cargo build --release -p ibc-cli",
            ibc.display(),
            diff.join("; ")
        ))
    }
}

pub fn markdown(p: &PatchId) -> String {
    format!(
        "| | |\n|---|---|\n| engine | `{}` |\n| ruleset hash | `{}` |\n| wasmtime | `{}` |\n| ABI | `{}` |\n",
        p.engine, p.ruleset_hash, p.wasmtime, p.abi
    )
}

/// `cargo xtask patch-id [--json | --markdown] [--bin <ibc>]`
pub fn main(args: &[String]) -> Result<()> {
    let mut args = Args::new(args);
    let (json, md) = (args.switch("--json"), args.switch("--markdown"));
    let bin = args.value("--bin")?;
    if let Some(extra) = args.rest()?.first() {
        return Err(format!("patch-id: unexpected argument {extra}"));
    }
    let id = match bin {
        Some(bin) => check_binary(Path::new(&bin))?,
        None => source()?,
    };
    if json {
        println!("{}", serde_json::to_string(&id).map_err(|e| e.to_string())?);
    } else if md {
        print!("{}", markdown(&id));
    } else {
        println!(
            "engine {}\nruleset {}\nwasmtime {}\nabi {}",
            id.engine, id.ruleset_hash, id.wasmtime, id.abi
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = "/// doc\npub const WASMTIME_VERSION: &str = \"51.0.0\";\n";
    const MANIFEST: &str =
        "[dependencies]\nwasmtime = { version = \"=51.0.0\", default-features = false }\n";
    const LOCK: &str = "[[package]]\nname = \"wasmtime\"\nversion = \"51.0.0\"\n\n[[package]]\nname = \"wasmtime-environ\"\nversion = \"51.0.0\"\n";

    #[test]
    fn wasmtime_version_is_read_when_all_three_agree() {
        assert_eq!(
            wasmtime_version_from(LIB, MANIFEST, LOCK).unwrap(),
            "51.0.0"
        );
        assert_eq!(
            wasmtime_version_from(LIB, MANIFEST, &LOCK.replace('\n', "\r\n")).unwrap(),
            "51.0.0"
        );
    }

    #[test]
    fn a_half_done_wasmtime_bump_is_an_error() {
        let bumped_lock = LOCK.replacen("51.0.0", "52.0.0", 1);
        assert!(wasmtime_version_from(LIB, MANIFEST, &bumped_lock)
            .unwrap_err()
            .contains("disagrees"));
        let loose = MANIFEST.replace("=51", "51");
        assert!(wasmtime_version_from(LIB, &loose, LOCK)
            .unwrap_err()
            .contains("pinned exactly"));
    }

    #[test]
    fn the_checkout_has_a_consistent_patch_identity() {
        let id = source().unwrap();
        assert_eq!(id.ruleset_hash.len(), 16);
        assert!(id.diff(&id).is_empty());
    }
}
