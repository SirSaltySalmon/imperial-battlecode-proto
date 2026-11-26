//! `cargo xtask docs [--check]`
//!
//! Writes the generated handbook in `docs/generated/` from its sources of truth, by computation rather than
//! by hand: the ruleset through the engine's own loader, the ABI and the SDK from their Rust source. Output is
//! deterministic (source order, no timestamps, LF), so `--check` can compare it with the committed files and
//! fail when someone changed a source without regenerating.

mod abi;
mod bundle;
mod md;
mod rules;
mod source;

use crate::util::{self, Args, Result};
use source::Module;
use std::path::Path;

/// Where the pages go, relative to the repository root.
pub const OUT_DIR: &str = "docs/generated";

const ABI: &str = "crates/abi/src/lib.rs";
const ENGINE: [&str; 2] = [
    "crates/engine/src/rules.rs",
    "crates/engine/src/economy/mod.rs",
];
const TOMLS: [&str; 4] = [
    "rules/core.toml",
    "rules/weapons.toml",
    "rules/abilities.toml",
    "rules/economy.toml",
];
const SDK: [&str; 4] = [
    "sdk/rust/src/lib.rs",
    "sdk/rust/src/geom.rs",
    "sdk/rust/src/nav.rs",
    "sdk/rust/src/tactics.rs",
];
/// Hand-written pages bundled into `llm.md`, in reading order. Generated pages and the SDK reference go
/// after `docs/ABI.md`.
const BEFORE: [&str; 5] = [
    "docs/GLOSSARY.md",
    "docs/RULES.md",
    "docs/rules/combat.md",
    "docs/rules/economy.md",
    "docs/rules/abilities.md",
];
/// Bundled when it exists (the participant guide is written by another part of the project).
const OPTIONAL: &str = "docs/PARTICIPANTS.md";

/// Every input file, read once with line ends normalised, so generation itself does no I/O.
pub struct Sources {
    files: Vec<(String, String)>,
}

impl Sources {
    pub fn load(root: &Path) -> Result<Sources> {
        let required = [ABI]
            .into_iter()
            .chain(ENGINE)
            .chain(TOMLS)
            .chain(SDK)
            .chain(BEFORE)
            .chain(["docs/ABI.md"]);
        let mut files = Vec::new();
        for rel in required {
            files.push((
                rel.to_string(),
                util::read_text(&root.join(rel))?.replace("\r\n", "\n"),
            ));
        }
        match std::fs::read_to_string(root.join(OPTIONAL)) {
            Ok(text) => files.push((OPTIONAL.to_string(), text.replace("\r\n", "\n"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{OPTIONAL}: {e}")),
        }
        Ok(Sources { files })
    }

    fn get(&self, rel: &str) -> &str {
        self.opt(rel)
            .unwrap_or_else(|| panic!("{rel} was not loaded"))
    }

    fn opt(&self, rel: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(p, _)| p == rel)
            .map(|(_, t)| t.as_str())
    }

    #[cfg(test)]
    fn set(&mut self, rel: &str, text: String) {
        match self.files.iter_mut().find(|(p, _)| p == rel) {
            Some(f) => f.1 = text,
            None => self.files.push((rel.to_string(), text)),
        }
    }
}

/// One generated page: its path relative to the repository root and its full text.
pub struct Page {
    pub path: String,
    pub text: String,
}

pub fn generate(src: &Sources) -> Result<Vec<Page>> {
    let abi = Module::parse(src.get(ABI), ABI)?;
    let engine = ENGINE
        .iter()
        .map(|p| Module::parse(src.get(p), p))
        .collect::<Result<Vec<_>>>()?;
    let [core, weapons, abilities, economy] = TOMLS.map(|p| src.get(p));
    let ruleset = ibc_engine::Ruleset::from_toml(core, weapons, abilities, economy)
        .map_err(|e| format!("rules/*.toml: {e}"))?;
    let rules_md = rules::render(&rules::Inputs {
        ruleset: &ruleset,
        abi: &abi,
        engine: &engine,
        tomls: TOMLS.map(|p| (p, src.get(p))),
    })?;
    let abi_md = abi::render(&abi);
    let sdk_md = bundle::sdk_reference(&SDK.map(|p| (p, src.get(p))));

    let rules_path = format!("{OUT_DIR}/rules.md");
    let abi_path = format!("{OUT_DIR}/abi.md");
    let mut parts: Vec<(&str, &str)> = BEFORE.iter().map(|p| (*p, src.get(p))).collect();
    parts.push(("docs/ABI.md", src.get("docs/ABI.md")));
    parts.push((&rules_path, &rules_md));
    parts.push((&abi_path, &abi_md));
    if let Some(text) = src.opt(OPTIONAL) {
        parts.push((OPTIONAL, text));
    }
    parts.push(("sdk/rust/src (Rust SDK quick reference)", &sdk_md));
    let llm_md = bundle::render(&parts);

    Ok(vec![
        Page {
            path: abi_path,
            text: abi_md,
        },
        Page {
            path: format!("{OUT_DIR}/llm.md"),
            text: llm_md,
        },
        Page {
            path: rules_path,
            text: rules_md,
        },
    ])
}

/// Pages whose committed copy under `root` differs from `pages`, and files in the output directory that
/// nothing generates. Empty when the committed handbook is fresh.
pub fn stale(root: &Path, pages: &[Page]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for p in pages {
        let on_disk = std::fs::read_to_string(root.join(&p.path)).map(|t| t.replace("\r\n", "\n"));
        if on_disk.ok().as_deref() != Some(p.text.as_str()) {
            out.push(p.path.clone());
        }
    }
    let dir = root.join(OUT_DIR);
    if dir.is_dir() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut extra: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| format!("{OUT_DIR}/{}", e.file_name().to_string_lossy()))
            .filter(|path| !pages.iter().any(|p| &p.path == path))
            .collect();
        extra.sort();
        out.extend(
            extra
                .into_iter()
                .map(|p| format!("{p} (not generated; delete it)")),
        );
    }
    Ok(out)
}

pub fn main(args: &[String]) -> Result<()> {
    let mut args = Args::new(args);
    let check = args.switch("--check");
    if let Some(extra) = args.rest()?.first() {
        return Err(format!(
            "docs: unexpected argument {extra:?} (usage: cargo xtask docs [--check])"
        ));
    }
    let root = util::root();
    let pages = generate(&Sources::load(&root)?)?;
    if check {
        let stale = stale(&root, &pages)?;
        if !stale.is_empty() {
            return Err(format!(
                "generated docs are out of date: {}\n  run `cargo xtask docs` and commit the result",
                stale.join(", ")
            ));
        }
        println!("docs OK: {} pages in {OUT_DIR} are up to date", pages.len());
        return Ok(());
    }
    for p in &pages {
        util::write(&root.join(&p.path), p.text.as_bytes())?;
        println!("wrote {}", p.path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn abi() -> Module {
        Module::parse(&util::read_text(&util::root().join(ABI)).unwrap(), ABI).unwrap()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = util::root().join("out/xtask-docs-test").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn write_all(root: &Path, pages: &[Page]) {
        for p in pages {
            util::write(&root.join(&p.path), p.text.as_bytes()).unwrap();
        }
    }

    #[test]
    fn generation_is_deterministic_and_lf_only() {
        let src = Sources::load(&util::root()).unwrap();
        let (a, b) = (generate(&src).unwrap(), generate(&src).unwrap());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.path, y.path);
            assert!(x.text == y.text, "{} differs between two runs", x.path);
            assert!(!x.text.contains('\r'), "{} has CR line ends", x.path);
        }
    }

    #[test]
    fn check_names_the_pages_a_rules_change_makes_stale() {
        let root = scratch("stale");
        let mut src = Sources::load(&util::root()).unwrap();
        let pages = generate(&src).unwrap();
        write_all(&root, &pages);
        assert!(stale(&root, &pages).unwrap().is_empty());

        let weapons = src
            .get("rules/weapons.toml")
            .replacen("damage = ", "damage = 1", 1);
        src.set("rules/weapons.toml", weapons);
        let changed = generate(&src).unwrap();
        let names = stale(&root, &changed).unwrap();
        assert_eq!(names, ["docs/generated/llm.md", "docs/generated/rules.md"]);

        util::write(&root.join(OUT_DIR).join("old.md"), b"x").unwrap();
        assert!(stale(&root, &pages).unwrap()[0].starts_with("docs/generated/old.md"));
    }

    #[test]
    fn rules_page_shows_loaded_values_and_names() {
        let src = Sources::load(&util::root()).unwrap();
        let pages = generate(&src).unwrap();
        let rules = &pages
            .iter()
            .find(|p| p.path.ends_with("rules.md"))
            .unwrap()
            .text;
        let ruleset = ibc_engine::Ruleset::builtin();
        assert!(rules.contains(&ibc_replay::hash_hex(ruleset.hash())));
        assert!(rules.contains(&format!("| `max_ticks` | {} |", ruleset.core.max_ticks)));
        assert!(rules.contains("| `effect` |") && rules.contains("(SMOKE)"));
        let llm = &pages
            .iter()
            .find(|p| p.path.ends_with("llm.md"))
            .unwrap()
            .text;
        assert!(
            llm.contains("<!-- part: docs/GLOSSARY.md -->")
                && llm.contains("# Rust SDK quick reference")
        );
    }

    #[test]
    fn optional_participant_guide_is_bundled_when_present() {
        let mut src = Sources::load(&util::root()).unwrap();
        src.set(OPTIONAL, "# Participants\n\nHello.\n".to_string());
        let pages = generate(&src).unwrap();
        let llm = &pages
            .iter()
            .find(|p| p.path.ends_with("llm.md"))
            .unwrap()
            .text;
        assert!(llm.contains("<!-- part: docs/PARTICIPANTS.md -->\n\n# Participants"));
    }
}
