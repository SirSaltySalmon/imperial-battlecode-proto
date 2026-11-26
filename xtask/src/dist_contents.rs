//! What goes into a release archive and into the pip wheel's data directory.
//!
//! A participant downloads one archive and has everything to play locally: the `ibc` binary, the built
//! replay viewer (with the engine wasm inside it, so `ibc view` works offline), the map pool, the game
//! documentation and the README. The server binaries and their systemd units ride along on Linux.
//!
//! `manifest` only decides which file goes where; `stage` copies. Keeping the decision pure is what lets
//! the listing be tested without building anything.

use crate::util::{self, Result};
use std::path::{Path, PathBuf};

/// One file of the archive: copied from `from` to `to` (relative, forward slashes) inside the staging dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub from: PathBuf,
    pub to: String,
}

/// Documents shipped when present (another part of the tree may not have written them yet).
const DOCS: &[&str] = &["docs/PARTICIPANTS.md", "docs/RULES.md", "docs/ABI.md"];
/// systemd units, shipped on Linux next to the binary they start.
const UNITS: &[(&str, &str)] = &[
    ("ibc-worker", "deploy/ibc-worker.service"),
    ("ibc-coordinator", "deploy/ibc-coordinator.service"),
];

pub struct Sources<'a> {
    /// Repository root.
    pub root: &'a Path,
    /// Where cargo put the release binaries for the target.
    pub built: &'a Path,
    pub bins: &'a [String],
    pub target: &'a str,
    /// A built viewer (`viewer/dist`); `None` leaves it out.
    pub viewer: Option<&'a Path>,
}

/// Every file of the archive, binaries first, then the rest sorted by destination.
pub fn manifest(src: &Sources) -> Result<Vec<Entry>> {
    let windows = src.target.contains("windows");
    let mut entries: Vec<Entry> = src
        .bins
        .iter()
        .map(|bin| {
            let file = util::exe_name(bin, windows);
            Entry {
                from: src.built.join(&file),
                to: file,
            }
        })
        .collect();
    let mut rest = participant_files(src.root, src.viewer)?;
    if src.target.contains("linux") {
        for (bin, unit) in UNITS {
            if src.bins.iter().any(|b| b == bin) {
                rest.push(Entry {
                    from: src.root.join(unit),
                    to: file_name(unit),
                });
            }
        }
    }
    rest.sort_by(|a, b| a.to.cmp(&b.to));
    entries.extend(rest);
    Ok(entries)
}

/// The platform-independent part: README, docs, maps and the viewer. Also what the wheel installs under
/// `<prefix>/share/imperial-battlecode/`.
pub fn participant_files(root: &Path, viewer: Option<&Path>) -> Result<Vec<Entry>> {
    let mut out = vec![Entry {
        from: root.join("README.md"),
        to: "README.md".to_string(),
    }];
    for doc in DOCS {
        let from = root.join(doc);
        if from.is_file() {
            out.push(Entry {
                from,
                to: doc.to_string(),
            });
        }
    }
    let rules = root.join("docs/rules");
    if rules.is_dir() {
        out.extend(
            walk(&rules, "docs/rules")?
                .into_iter()
                .filter(|e| e.to.ends_with(".md")),
        );
    }
    // The map pool: the `.ibcmap` files at the top of `maps/` (renders and drafts stay home).
    let mut maps = Vec::new();
    for entry in read_dir(&root.join("maps"))? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".ibcmap") && entry.path().is_file() {
            maps.push(Entry {
                from: entry.path(),
                to: format!("maps/{name}"),
            });
        }
    }
    if maps.is_empty() {
        return Err(format!("{}: no .ibcmap files", root.join("maps").display()));
    }
    out.extend(maps);
    if let Some(viewer) = viewer {
        out.extend(walk(viewer, "viewer")?);
    }
    out.sort_by(|a, b| a.to.cmp(&b.to));
    Ok(out)
}

/// A built viewer directory, checked: `index.html` and at least one engine build in `engine/`.
pub fn check_viewer(dir: &Path) -> Result<()> {
    if !dir.join("index.html").is_file() {
        return Err(format!(
            "{} is not a built viewer (no index.html). Build it: cargo build -p ibc-engine-wasm --release \
             --target wasm32-unknown-unknown && (cd viewer && npm ci && npm run build); or pass --no-viewer",
            dir.display()
        ));
    }
    let has_engine = read_dir(&dir.join("engine"))
        .unwrap_or_default()
        .iter()
        .any(|e| e.file_name().to_string_lossy().ends_with(".wasm"));
    if !has_engine {
        return Err(format!(
            "{} has no engine/*.wasm: the viewer could not open replays. Build ibc-engine-wasm before the viewer",
            dir.display()
        ));
    }
    Ok(())
}

/// Copy every entry into `dest` (which is replaced).
pub fn stage(entries: &[Entry], dest: &Path) -> Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    for e in entries {
        util::write(&dest.join(&e.to), &util::read(&e.from)?)?;
    }
    Ok(())
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn read_dir(dir: &Path) -> Result<Vec<std::fs::DirEntry>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .collect::<std::io::Result<_>>()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    Ok(entries)
}

/// Every file under `dir`, as entries below `prefix`.
fn walk(dir: &Path, prefix: &str) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for entry in read_dir(dir)? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let to = format!("{prefix}/{name}");
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path, &to)?);
        } else {
            out.push(Entry { from: path, to });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ibc-xtask-contents-{}-{}",
            std::process::id(),
            files.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for f in files {
            util::write(&dir.join(f), f.as_bytes()).unwrap();
        }
        dir
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.to.as_str()).collect()
    }

    #[test]
    fn the_archive_lists_binaries_docs_maps_viewer_and_linux_units() {
        let root = tree(&[
            "README.md",
            "docs/RULES.md",
            "docs/ABI.md",
            "docs/rules/combat.md",
            "docs/rules/notes.txt",
            "docs/TODO.md",
            "maps/crossfire.ibcmap",
            "maps/agora.ibcmap",
            "maps/renders/agora.png",
            "deploy/ibc-worker.service",
            "deploy/ibc-coordinator.service",
            "viewer/dist/index.html",
            "viewer/dist/engine/0.1.0.wasm",
            "viewer/dist/assets/app.js",
        ]);
        let viewer = root.join("viewer/dist");
        check_viewer(&viewer).unwrap();
        let bins = ["ibc".to_string(), "ibc-worker".to_string()];
        let built = root.join("built");
        let linux = Sources {
            root: &root,
            built: &built,
            bins: &bins,
            target: "x86_64-unknown-linux-musl",
            viewer: Some(&viewer),
        };
        let entries = manifest(&linux).unwrap();
        assert_eq!(
            names(&entries),
            [
                "ibc",
                "ibc-worker",
                "README.md",
                "docs/ABI.md",
                "docs/RULES.md",
                "docs/rules/combat.md",
                "ibc-worker.service",
                "maps/agora.ibcmap",
                "maps/crossfire.ibcmap",
                "viewer/assets/app.js",
                "viewer/engine/0.1.0.wasm",
                "viewer/index.html",
            ]
        );
        assert_eq!(entries[0].from, built.join("ibc"));

        let windows = Sources {
            target: "x86_64-pc-windows-msvc",
            viewer: None,
            ..linux
        };
        let entries = manifest(&windows).unwrap();
        assert_eq!(names(&entries)[..2], ["ibc.exe", "ibc-worker.exe"]);
        assert!(!names(&entries).iter().any(|n| n.ends_with(".service")));
        assert!(!names(&entries).iter().any(|n| n.starts_with("viewer/")));

        // A participants' guide is picked up once it exists.
        util::write(&root.join("docs/PARTICIPANTS.md"), b"guide").unwrap();
        let files = participant_files(&root, None).unwrap();
        assert!(names(&files).contains(&"docs/PARTICIPANTS.md"));

        stage(&files, &root.join("stage")).unwrap();
        assert!(root.join("stage/maps/agora.ibcmap").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_viewer_without_an_engine_or_index_is_refused() {
        let root = tree(&["viewer/dist/index.html", "maps/a.ibcmap"]);
        assert!(check_viewer(&root.join("viewer/dist"))
            .unwrap_err()
            .contains("engine"));
        assert!(check_viewer(&root.join("maps"))
            .unwrap_err()
            .contains("index.html"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
