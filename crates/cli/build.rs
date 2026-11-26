//! Packs the participant kit into the `ibc` binary: the Rust SDK and the ABI crate it depends on, the C++
//! headers, the map pool, the participant docs and the ruleset they link to, the reference bots the `example` templates come from, and
//! `templates/`. `ibc init` and `ibc new` write these out, so a workspace always matches the engine and ABI
//! of the binary that made it.
//!
//! Emits `$OUT_DIR/kit_files.rs`: `static FILES: &[(&str, &str)]`, sorted by path (relative to the repo
//! root, `/`-separated), each file pulled in with `include_str!`.

use std::path::{Path, PathBuf};

/// (directory relative to the repo root, file pattern). A pattern is `*.ext` (files directly in the
/// directory), `**/*.ext` (also in subdirectories), `**` (every file below) or an exact file name.
const KIT: &[(&str, &str)] = &[
    ("sdk/rust", "Cargo.toml"),
    ("sdk/rust", "README.md"),
    ("sdk/rust/src", "**/*.rs"),
    ("crates/abi", "Cargo.toml"),
    ("crates/abi/src", "**/*.rs"),
    ("sdk/cpp", "*.h"),
    ("sdk/cpp", "README.md"),
    ("maps", "*.ibcmap"),
    ("docs", "RULES.md"),
    ("docs", "ABI.md"),
    ("docs", "PARTICIPANTS.md"),
    ("docs/rules", "*.md"),
    ("rules", "*.toml"),
    ("bots/rusher/src", "*.rs"),
    ("bots/cpp-starter/src", "*.cpp"),
    ("crates/cli/templates", "**"),
];

fn matches(pattern: &str, name: &str) -> bool {
    if pattern == "**" {
        return true;
    }
    match pattern.trim_start_matches("**/").strip_prefix('*') {
        Some(ext) => name.ends_with(ext),
        None => pattern == name,
    }
}

fn walk(dir: &Path, rel: &str, pattern: &str, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|e| e.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let rel = format!("{rel}/{name}");
        if path.is_dir() {
            if pattern.starts_with("**") {
                walk(&path, &rel, pattern, out);
            }
        } else if matches(pattern, &name) {
            out.push((rel, path));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("..").join("..");
    let mut files = Vec::new();
    for (dir, pattern) in KIT {
        let at = root.join(dir);
        println!("cargo:rerun-if-changed={}", at.display());
        walk(&at, dir, pattern, &mut files);
    }
    files.sort();
    files.dedup_by(|a, b| a.0 == b.0);
    let mut src = String::from("static FILES: &[(&str, &str)] = &[\n");
    for (rel, path) in &files {
        src.push_str(&format!(
            "    ({rel:?}, include_str!({:?})),\n",
            path.display().to_string()
        ));
    }
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("kit_files.rs");
    std::fs::write(out, src).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
