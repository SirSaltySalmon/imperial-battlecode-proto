//! `cargo xtask dist --target <triple> [--builder cargo|zigbuild|cross] [--out dist] [--no-build]`
//! `cargo xtask dist sums [dir]`
//!
//! Builds the release binaries that exist in this checkout (`ibc`, plus the worker and coordinator when
//! their crates are present), stripped, and packs them into `dist/ibc-<version>-<target>.tar.gz` (`.zip`
//! for Windows) next to the README and a `PATCH.json` with the patch identity. `sums` writes `SHA256SUMS`
//! over everything in the directory, in the format `sha256sum -c` reads.

use crate::patch_id;
use crate::util::{self, Args, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Packages whose binaries are shipped, when they exist.
const SHIPPED: &[&str] = &["ibc-cli", "ibc-worker", "ibc-coordinator"];

struct Shipped {
    version: String,
    packages: Vec<String>,
    bins: Vec<String>,
}

/// Ask cargo which of the shipped packages exist and which binaries they build.
fn shipped() -> Result<Shipped> {
    let text =
        util::capture(util::cargo().args(["metadata", "--no-deps", "--format-version", "1"]))?;
    let meta: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("cargo metadata: {e}"))?;
    shipped_from(&meta)
}

fn shipped_from(meta: &serde_json::Value) -> Result<Shipped> {
    let mut found = Shipped {
        version: String::new(),
        packages: Vec::new(),
        bins: Vec::new(),
    };
    for wanted in SHIPPED {
        let package = meta["packages"]
            .as_array()
            .and_then(|ps| ps.iter().find(|p| p["name"] == *wanted));
        let Some(package) = package else { continue };
        if *wanted == "ibc-cli" {
            found.version = package["version"].as_str().unwrap_or_default().to_string();
        }
        found.packages.push(wanted.to_string());
        for target in package["targets"].as_array().into_iter().flatten() {
            let is_bin = target["kind"]
                .as_array()
                .is_some_and(|k| k.iter().any(|v| v == "bin"));
            if let (true, Some(name)) = (is_bin, target["name"].as_str()) {
                found.bins.push(name.to_string());
            }
        }
    }
    if found.version.is_empty() || found.bins.is_empty() {
        return Err(
            "package ibc-cli (the `ibc` binary) was not found in the workspace".to_string(),
        );
    }
    Ok(found)
}

fn build(builder: &str, target: &str, packages: &[String]) -> Result<()> {
    let mut cmd = match builder {
        "cargo" => {
            let mut c = util::cargo();
            c.arg("build");
            c
        }
        "zigbuild" => {
            let mut c = util::cargo();
            c.arg("zigbuild");
            c
        }
        "cross" => {
            let mut c = Command::new("cross");
            c.current_dir(util::root()).arg("build");
            c
        }
        other => {
            return Err(format!(
                "dist: unknown builder {other:?} (cargo, zigbuild, cross)"
            ))
        }
    };
    cmd.args(["--release", "--locked", "--target", target]);
    for p in packages {
        cmd.args(["-p", p]);
    }
    // Strip through the profile, so no target-specific strip tool is needed when cross-compiling.
    cmd.env("CARGO_PROFILE_RELEASE_STRIP", "symbols");
    util::run(&mut cmd)
}

/// PATCH.json is written from the sources, so the binary next to it must have been built from them.
/// A binary for another platform cannot be asked; that case is said out loud instead of passing silently.
fn check_staged_ibc(ibc: &Path, source: &patch_id::PatchId) -> Result<()> {
    let started = Command::new(ibc)
        .args(["version", "--json"])
        .stdin(std::process::Stdio::null())
        .output();
    let Ok(out) = started else {
        println!("note: {} does not run on this host; its patch identity was not checked against the sources", ibc.display());
        return Ok(());
    };
    let reported: patch_id::PatchId = serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "`{} version --json` did not print a patch identity: {e}",
            ibc.display()
        )
    })?;
    let diff = source.diff(&reported);
    if diff.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} was not built from these sources (source -> binary): {}. Build without --no-build",
            ibc.display(),
            diff.join("; ")
        ))
    }
}

/// Pack directory `name` (inside `parent`) into an archive next to it and return the archive path.
fn archive(parent: &Path, name: &str, windows: bool) -> Result<PathBuf> {
    if !windows {
        let file = format!("{name}.tar.gz");
        util::run(
            Command::new("tar")
                .current_dir(parent)
                .args(["-czf", &file, name]),
        )?;
        return Ok(parent.join(file));
    }
    let file = format!("{name}.zip");
    let _ = std::fs::remove_file(parent.join(&file));
    if cfg!(windows) {
        // The tar that ships with Windows is bsdtar, which writes zip; a `tar` from Git Bash would not.
        let system_tar =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
                .join("System32/tar.exe");
        util::run(
            Command::new(system_tar)
                .current_dir(parent)
                .args(["-a", "-cf", &file, name]),
        )?;
    } else {
        util::run(
            Command::new("zip")
                .current_dir(parent)
                .args(["-qr", &file, name]),
        )?;
    }
    Ok(parent.join(file))
}

/// `<sha256>  <file name>` for every file in `dir` except SHA256SUMS itself, sorted by name.
fn sums_text(dir: &Path) -> Result<String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && name != "SHA256SUMS" {
            names.push(name);
        }
    }
    names.sort();
    if names.is_empty() {
        return Err(format!("{}: nothing to sum", dir.display()));
    }
    let mut text = String::new();
    for name in names {
        text.push_str(&format!(
            "{}  {name}\n",
            ibc_replay::sha256_hex(&util::read(&dir.join(&name))?)
        ));
    }
    Ok(text)
}

fn sums(dir: &Path) -> Result<()> {
    let text = sums_text(dir)?;
    util::write(&dir.join("SHA256SUMS"), text.as_bytes())?;
    print!("{text}");
    Ok(())
}

pub fn main(args: &[String]) -> Result<()> {
    let mut args = Args::new(args);
    let target = args.value("--target")?;
    let builder = args
        .value("--builder")?
        .unwrap_or_else(|| "cargo".to_string());
    let out = args
        .value("--out")?
        .map_or_else(|| util::root().join("dist"), PathBuf::from);
    let no_build = args.switch("--no-build");
    let rest = args.rest()?;
    match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["sums"] => return sums(&out),
        ["sums", dir] => return sums(Path::new(dir)),
        [] => {}
        _ => return Err("usage: cargo xtask dist --target <triple> [--builder cargo|zigbuild|cross] [--out dir] [--no-build] | dist sums [dir]".to_string()),
    }
    let target = target
        .ok_or("dist: --target <triple> is required (for example x86_64-unknown-linux-musl)")?;
    let windows = target.contains("windows");
    let ship = shipped()?;
    let patch = patch_id::source()?;
    println!(
        "dist {} for {target}: packages {}; {}",
        ship.version,
        ship.packages.join(", "),
        patch.one_line()
    );
    if !no_build {
        build(&builder, &target, &ship.packages)?;
    }

    let name = format!("ibc-{}-{target}", ship.version);
    let stage = out.join(&name);
    if stage.exists() {
        std::fs::remove_dir_all(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    }
    let built = util::target_dir().join(&target).join("release");
    for bin in &ship.bins {
        let file = util::exe_name(bin, windows);
        util::write(&stage.join(&file), &util::read(&built.join(&file))?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(stage.join(&file), std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }
    }
    check_staged_ibc(&stage.join(util::exe_name("ibc", windows)), &patch)?;
    util::write(
        &stage.join("README.md"),
        &util::read(&util::root().join("README.md"))?,
    )?;
    let mut patch_json = serde_json::to_string_pretty(&patch).map_err(|e| e.to_string())?;
    patch_json.push('\n');
    util::write(&stage.join("PATCH.json"), patch_json.as_bytes())?;

    let packed = archive(&out, &name, windows)?;
    std::fs::remove_dir_all(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    println!("{}", packed.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_shipped_packages_that_exist_are_built() {
        let meta = serde_json::json!({"packages": [
            {"name": "ibc-engine", "version": "0.3.1", "targets": [{"kind": ["lib"], "name": "ibc_engine"}]},
            {"name": "ibc-cli", "version": "0.3.1", "targets": [{"kind": ["bin"], "name": "ibc"}]},
            {"name": "ibc-worker", "version": "0.3.1", "targets": [{"kind": ["lib"], "name": "w"}, {"kind": ["bin"], "name": "ibc-worker"}]},
        ]});
        let s = shipped_from(&meta).unwrap();
        assert_eq!(s.version, "0.3.1");
        assert_eq!(s.packages, ["ibc-cli", "ibc-worker"]);
        assert_eq!(s.bins, ["ibc", "ibc-worker"]);
        assert!(shipped_from(&serde_json::json!({"packages": []})).is_err());
    }

    #[test]
    fn sums_are_sorted_and_skip_the_sums_file() {
        let dir = std::env::temp_dir().join(format!("ibc-xtask-sums-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.zip"), b"b").unwrap();
        std::fs::write(dir.join("a.tar.gz"), b"abc").unwrap();
        std::fs::write(dir.join("SHA256SUMS"), b"old").unwrap();
        let text = sums_text(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            text,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a.tar.gz\n\
             3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d  b.zip\n"
        );
    }
}
