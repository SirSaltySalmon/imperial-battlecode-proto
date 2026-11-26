//! `ibc init [DIR]`: make (or refresh) a participant workspace.
//!
//! ```text
//! ibc.toml        version pin, `ibc run` defaults          written once, then only the pin moves
//! bots/starter/   a starter bot                            only when bots/ has no bot yet
//! maps/           the bundled map pool                     missing maps added, existing files kept
//! docs/           rules, ABI, SDK and participant docs     refreshed
//! .ibc/           SDK crates, C++ headers, VERSION marker  refreshed (kit-owned: do not edit)
//! .gitignore, AGENTS.md                                    only when missing
//! ```
//!
//! Re-running it after updating `ibc` moves the workspace to the new version without touching bots or
//! custom maps.

use crate::new::{self, Lang, Template};
use crate::{embedded, workspace};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct InitArgs {
    /// Workspace directory (created if missing)
    #[arg(default_value = ".")]
    dir: PathBuf,
    /// Language of the starter bot written into an empty bots/
    #[arg(long, value_enum, default_value_t = Lang::Rust)]
    lang: Lang,
}

const STARTER: &str = "starter";
const DEFAULT_MAP: &str = "crossfire.ibcmap";

/// Write `content` unless the file already holds it (keeps mtimes, and with them cargo's cache, quiet).
fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if std::fs::read(path).ok().as_deref() == Some(content.as_bytes()) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

/// Delete files under `root/dir` that are not in `keep` (paths relative to `root`, `/`-separated):
/// whatever an older kit wrote there and this one does not.
fn prune(root: &Path, dir: &str, keep: &BTreeSet<String>) -> Result<()> {
    let at = root.join(dir);
    let Ok(entries) = std::fs::read_dir(&at) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", at.display()))?;
        let rel = format!("{dir}/{}", entry.file_name().to_string_lossy());
        if entry.file_type()?.is_dir() {
            prune(root, &rel, keep)?;
            // Gone with the files it held; fails (and stays) when something is left in it.
            let _ = std::fs::remove_dir(entry.path());
        } else if !keep.contains(&rel) {
            std::fs::remove_file(entry.path())
                .with_context(|| format!("removing {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// `.ibc/`: the SDK crates, the headers, then the marker last (so an interrupted init still reads as stale).
fn write_sdk(root: &Path) -> Result<bool> {
    let files = embedded::sdk_files();
    let mut changed = false;
    for f in &files {
        changed |= write_if_changed(&root.join(&f.path), &f.content)?;
    }
    let keep: BTreeSet<String> = files.into_iter().map(|f| f.path).collect();
    prune(root, embedded::SDK_DIR, &keep)?;
    prune(root, embedded::INCLUDE_DIR, &keep)?;
    changed |= write_if_changed(&root.join(embedded::MARKER), &embedded::marker())?;
    Ok(changed)
}

/// Add bundled maps that are missing; leave any existing file alone. Returns notes for the participant.
fn write_maps(root: &Path) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    for (name, content) in embedded::maps() {
        let path = root.join("maps").join(name);
        match std::fs::read(&path) {
            Ok(bytes) if bytes != content.as_bytes() => notes.push(format!(
                "kept maps/{name} (it differs from the bundled one; delete it to get that)"
            )),
            Ok(_) => {}
            Err(_) => {
                write_if_changed(&path, content)?;
                notes.push(format!("added maps/{name}"));
            }
        }
    }
    Ok(notes)
}

/// Directory names under `root/bots`, sorted.
fn bots(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join("bots"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn write_config(root: &Path, opponent: Option<&str>) -> Result<Option<String>> {
    let path = root.join(workspace::CONFIG);
    let version = env!("CARGO_PKG_VERSION");
    if path.exists() {
        return Ok(workspace::set_pin(root, version)?
            .map(|old| format!("moved the ibc.toml pin from {old} to {version}")));
    }
    let map = if embedded::maps().any(|(n, _)| n == DEFAULT_MAP) {
        DEFAULT_MAP.to_string()
    } else {
        embedded::maps()
            .next()
            .map(|(n, _)| n.to_string())
            .unwrap_or_default()
    };
    let text = embedded::file("crates/cli/templates/workspace/ibc.toml")
        .replace("{{version}}", version)
        .replace("{{map}}", &format!("maps/{map}"))
        .replace(
            "{{opponent}}",
            &format!("bots/{}", opponent.unwrap_or(STARTER)),
        );
    write_if_changed(&path, &text)?;
    Ok(Some("wrote ibc.toml".into()))
}

/// Make or refresh the workspace at `root`. Returns what it did, one line each.
pub fn init(root: &Path, lang: Lang) -> Result<Vec<String>> {
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let mut notes = Vec::new();
    if write_sdk(root)? {
        notes.push(format!(
            "wrote .ibc/ (SDK for ibc {})",
            env!("CARGO_PKG_VERSION")
        ));
    }
    let mut docs_changed = false;
    for doc in embedded::doc_files() {
        docs_changed |= write_if_changed(&root.join(&doc.path), &doc.content)?;
    }
    if docs_changed {
        notes.push("wrote docs/ and rules/ (reference: refreshed by every `ibc init`)".into());
    }
    notes.extend(write_maps(root)?);
    if bots(root).is_empty() {
        new::create(root, STARTER, lang, Template::Starter)?;
        notes.push(format!("created bots/{STARTER}"));
    }
    notes.extend(write_config(root, bots(root).first().map(String::as_str))?);
    for (name, template) in [(".gitignore", "gitignore"), ("AGENTS.md", "AGENTS.md")] {
        let path = root.join(name);
        if !path.exists() {
            write_if_changed(
                &path,
                embedded::file(&format!("crates/cli/templates/workspace/{template}")),
            )?;
        }
    }
    Ok(notes)
}

pub fn run(args: InitArgs) -> Result<()> {
    let notes = init(&args.dir, args.lang)?;
    for note in &notes {
        println!("{note}");
    }
    if notes.is_empty() {
        println!("{} is up to date", args.dir.display());
    }
    if let Some(bot) = bots(&args.dir).first() {
        println!(
            "next: cd {} && ibc build bots/{bot} && ibc run bots/{bot}",
            args.dir.display()
        );
    }
    println!("guide: docs/PARTICIPANTS.md");
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh empty directory under the system temp dir (outside any workspace).
    pub fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ibc-cli-test-{}-{tag}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn init_writes_the_workspace() {
        let root = temp_dir("init");
        init(&root, Lang::Rust).unwrap();
        for want in [
            "ibc.toml",
            ".gitignore",
            "AGENTS.md",
            "maps/crossfire.ibcmap",
            "docs/RULES.md",
            "docs/ABI.md",
            "docs/PARTICIPANTS.md",
            "rules/economy.toml",
            "bots/starter/Cargo.toml",
            "bots/starter/src/lib.rs",
            ".ibc/VERSION",
            ".ibc/sdk/rust/Cargo.toml",
            ".ibc/sdk/rust/src/lib.rs",
            ".ibc/sdk/abi/Cargo.toml",
            ".ibc/sdk/abi/src/lib.rs",
            ".ibc/include/ibc.h",
            ".ibc/include/ibc_abi.h",
        ] {
            assert!(root.join(want).is_file(), "{want} missing");
        }
        assert_eq!(
            workspace::find(&root.join("bots/starter")),
            Some(std::path::absolute(&root).unwrap())
        );
        assert!(workspace::staleness(&root.join("bots/starter")).is_empty());
        let config = read(&root.join("ibc.toml"));
        assert!(config.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))));
        assert!(
            config.contains("map = \"maps/crossfire.ibcmap\"")
                && config.contains("opponent = \"bots/starter\"")
        );
        // Nothing the workspace builds from points back into the repo.
        for manifest in [
            ".ibc/sdk/rust/Cargo.toml",
            ".ibc/sdk/abi/Cargo.toml",
            "bots/starter/Cargo.toml",
        ] {
            let text = read(&root.join(manifest));
            assert!(
                !text.contains("crates/") && !text.contains("../../sdk"),
                "{manifest}: {text}"
            );
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reinit_keeps_user_files_and_refreshes_the_kit() {
        let root = temp_dir("reinit");
        init(&root, Lang::Cpp).unwrap();
        assert!(root.join("bots/starter/src/main.cpp").is_file());
        let user_bot = root.join("bots/starter/src/main.cpp");
        std::fs::write(&user_bot, "// mine\n").unwrap();
        std::fs::write(root.join("maps/custom.ibcmap"), "mine").unwrap();
        std::fs::write(root.join("maps/crossfire.ibcmap"), "edited").unwrap();
        std::fs::remove_file(root.join("maps/tunnels.ibcmap")).unwrap();
        std::fs::write(root.join(".gitignore"), "mine\n").unwrap();
        let config = read(&root.join("ibc.toml"))
            .replace(env!("CARGO_PKG_VERSION"), "0.0.0")
            .replace("bots/starter", "bots/other");
        std::fs::write(root.join("ibc.toml"), &config).unwrap();
        // An old kit: a stale header, a file this kit no longer has, a different marker.
        std::fs::write(root.join(".ibc/include/ibc.h"), "old").unwrap();
        std::fs::write(root.join(".ibc/sdk/rust/src/gone.rs"), "old").unwrap();
        std::fs::create_dir_all(root.join(".ibc/sdk/rust/src/old")).unwrap();
        std::fs::write(root.join(".ibc/sdk/rust/src/old/mod.rs"), "old").unwrap();
        std::fs::write(root.join(".ibc/VERSION"), "ibc 0.0.0\n").unwrap();
        assert_eq!(workspace::staleness(&root.join("bots/starter")).len(), 2);

        let notes = init(&root, Lang::Rust).unwrap();
        assert_eq!(read(&user_bot), "// mine\n");
        assert!(
            !root.join("bots/starter/Cargo.toml").exists(),
            "re-init must not add a bot when there is one"
        );
        assert_eq!(read(&root.join("maps/custom.ibcmap")), "mine");
        assert_eq!(read(&root.join("maps/crossfire.ibcmap")), "edited");
        assert!(root.join("maps/tunnels.ibcmap").is_file());
        assert_eq!(read(&root.join(".gitignore")), "mine\n");
        assert_eq!(
            read(&root.join(".ibc/include/ibc.h")),
            embedded::file("sdk/cpp/ibc.h")
        );
        assert!(!root.join(".ibc/sdk/rust/src/gone.rs").exists());
        assert!(!root.join(".ibc/sdk/rust/src/old").exists());
        assert_eq!(read(&root.join(".ibc/VERSION")), embedded::marker());
        assert_eq!(
            read(&root.join("ibc.toml")),
            config.replace("0.0.0", env!("CARGO_PKG_VERSION"))
        );
        assert!(workspace::staleness(&root.join("bots/starter")).is_empty());
        assert!(
            notes.iter().any(|n| n == "added maps/tunnels.ibcmap"),
            "{notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("kept maps/crossfire.ibcmap")),
            "{notes:?}"
        );

        // A third run has nothing left to do.
        assert!(init(&root, Lang::Rust)
            .unwrap()
            .iter()
            .all(|n| n.starts_with("kept ")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn new_bots_land_in_the_workspace() {
        let root = temp_dir("new");
        init(&root, Lang::Rust).unwrap();
        let dir = new::create(&root, "cbot", Lang::Cpp, Template::Example).unwrap();
        assert!(dir.join("src/main.cpp").is_file() && dir.join("compile_flags.txt").is_file());
        assert!(
            new::create(&root, "cbot", Lang::Rust, Template::Starter).is_err(),
            "must not overwrite a bot"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
