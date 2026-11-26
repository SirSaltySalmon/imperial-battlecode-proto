//! The participant kit inside the binary (packed by `build.rs`): what `ibc init` writes into `.ibc/`,
//! `maps/` and `docs/`, and the templates `ibc new` renders. Because the files are compiled in, a
//! workspace's SDK always matches the engine and ABI of the `ibc` that wrote it.

use sha2::{Digest, Sha256};

include!(concat!(env!("OUT_DIR"), "/kit_files.rs"));

/// The embedded file at `path` (relative to the repo root). Every caller names a path `build.rs` packs,
/// so a miss is a bug in this binary.
pub fn file(path: &str) -> &'static str {
    match FILES.binary_search_by(|(p, _)| p.cmp(&path)) {
        Ok(i) => FILES[i].1,
        Err(_) => panic!("{path} is not embedded in this ibc (see crates/cli/build.rs)"),
    }
}

/// Embedded files under `dir/`, as (path relative to `dir`, content), sorted.
pub fn under(dir: &str) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
    FILES
        .iter()
        .filter_map(move |(p, c)| p.strip_prefix(dir)?.strip_prefix('/').map(|rel| (rel, *c)))
}

/// A file `ibc init` owns: written on every init, never edited by the participant.
pub struct KitFile {
    /// Relative to the workspace root, `/`-separated.
    pub path: String,
    pub content: String,
}

/// Where the vendored SDK lives in a workspace.
pub const SDK_DIR: &str = ".ibc/sdk";
pub const INCLUDE_DIR: &str = ".ibc/include";
/// Marker naming the ibc that wrote `.ibc/` (see `marker`).
pub const MARKER: &str = ".ibc/VERSION";

/// Text replacements applied to a file on its way into a workspace. Each one must apply (a test checks),
/// so a moved line in the source shows up as a failing test rather than a broken kit.
const FIXUPS: &[(&str, &str, &str)] = &[
    // The SDK crate depends on the ABI crate by path; in a workspace it sits next to it.
    ("sdk/rust/Cargo.toml", "\"../../crates/abi\"", "\"../abi\""),
    // Quickstarts written for the repo layout.
    (
        "sdk/rust/README.md",
        "\"../../sdk/rust\"",
        "\"../../.ibc/sdk/rust\"",
    ),
    (
        "docs/ABI.md",
        "(../crates/abi/src/lib.rs)",
        "(../.ibc/sdk/abi/src/lib.rs)",
    ),
];

fn fixed(src: &str) -> String {
    FIXUPS
        .iter()
        .filter(|(p, _, _)| *p == src)
        .fold(file(src).to_string(), |text, (_, from, to)| {
            text.replace(from, to)
        })
}

/// The ABI crate's manifest inherits from the repo workspace; a vendored copy states the values itself and
/// is its own workspace, so cargo never goes looking for a parent one.
fn standalone_abi_manifest() -> String {
    let edition = file("sdk/rust/Cargo.toml")
        .lines()
        .find(|l| l.starts_with("edition"))
        .expect("sdk/rust/Cargo.toml states its edition")
        .to_string();
    let mut out = String::new();
    for line in file("crates/abi/Cargo.toml").lines() {
        let line = match line.split_once(".workspace = true") {
            Some(("version", _)) => format!("version = \"{}\"", env!("CARGO_PKG_VERSION")),
            Some(("license", _)) => format!("license = \"{}\"", env!("CARGO_PKG_LICENSE")),
            Some(("edition", _)) => edition.clone(),
            _ => line.to_string(),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("\n[workspace]\n");
    out
}

/// Everything `ibc init` writes under `.ibc/` except the marker: the Rust SDK and ABI crates, the C++ headers.
pub fn sdk_files() -> Vec<KitFile> {
    let mut out = Vec::new();
    for (rel, _) in under("sdk/rust").filter(|(r, _)| *r == "Cargo.toml" || r.starts_with("src/")) {
        out.push(KitFile {
            path: format!("{SDK_DIR}/rust/{rel}"),
            content: fixed(&format!("sdk/rust/{rel}")),
        });
    }
    for (rel, content) in under("crates/abi").filter(|(r, _)| r.starts_with("src/")) {
        out.push(KitFile {
            path: format!("{SDK_DIR}/abi/{rel}"),
            content: content.to_string(),
        });
    }
    out.push(KitFile {
        path: format!("{SDK_DIR}/abi/Cargo.toml"),
        content: standalone_abi_manifest(),
    });
    for (rel, content) in under("sdk/cpp").filter(|(r, _)| r.ends_with(".h")) {
        out.push(KitFile {
            path: format!("{INCLUDE_DIR}/{rel}"),
            content: content.to_string(),
        });
    }
    out
}

/// The reference files `ibc init` refreshes: the participant docs under `docs/`, and the ruleset of this
/// patch under `rules/` (the docs link to it; the engine uses its built-in copy, so editing it changes nothing).
pub fn doc_files() -> Vec<KitFile> {
    let mut out: Vec<KitFile> = under("docs")
        .map(|(rel, _)| KitFile {
            path: format!("docs/{rel}"),
            content: fixed(&format!("docs/{rel}")),
        })
        .collect();
    out.push(KitFile {
        path: "docs/SDK-RUST.md".into(),
        content: fixed("sdk/rust/README.md"),
    });
    out.push(KitFile {
        path: "docs/SDK-CPP.md".into(),
        content: file("sdk/cpp/README.md").to_string(),
    });
    out.extend(under("rules").map(|(rel, content)| KitFile {
        path: format!("rules/{rel}"),
        content: content.to_string(),
    }));
    out
}

/// The bundled map pool, as (file name, content).
pub fn maps() -> impl Iterator<Item = (&'static str, &'static str)> {
    under("maps")
}

/// What `.ibc/VERSION` holds: the identity of this binary's patch plus a hash of the files `sdk_files`
/// writes, so two builds of the same version with a different SDK are told apart.
pub fn marker() -> String {
    let mut h = Sha256::new();
    for KitFile { path, content } in sdk_files() {
        h.update((path.len() as u64).to_le_bytes());
        h.update(path.as_bytes());
        h.update((content.len() as u64).to_le_bytes());
        h.update(content.as_bytes());
    }
    let kit: String = h
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!(
        "ibc {}\nengine {}\nabi {}\nruleset {}\nkit {kit}\n",
        env!("CARGO_PKG_VERSION"),
        ibc_engine::ENGINE_VERSION,
        ibc_abi::ABI_VERSION,
        ibc_replay::hash_hex(ibc_engine::Ruleset::builtin().hash()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_sorted_and_unique() {
        assert!(
            FILES.windows(2).all(|w| w[0].0 < w[1].0),
            "build.rs must emit sorted, unique paths"
        );
    }

    #[test]
    fn every_fixup_applies() {
        for (src, from, to) in FIXUPS {
            assert!(
                file(src).contains(from),
                "{src} no longer contains {from:?}: update FIXUPS"
            );
            assert!(!fixed(src).contains(from) && fixed(src).contains(to));
        }
    }

    #[test]
    fn vendored_manifests_point_inside_the_kit() {
        let files = sdk_files();
        let manifest = |p: &str| &files.iter().find(|f| f.path == p).unwrap().content;
        let sdk = manifest(".ibc/sdk/rust/Cargo.toml");
        assert!(sdk.contains("path = \"../abi\"") && !sdk.contains("crates/abi"));
        let abi = manifest(".ibc/sdk/abi/Cargo.toml");
        let parsed: toml::Table = toml::from_str(abi).expect("vendored abi manifest parses");
        assert!(!abi.contains("workspace = true"), "{abi}");
        assert_eq!(parsed["package"]["name"].as_str(), Some("ibc-abi"));
        assert_eq!(
            parsed["package"]["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert!(parsed.contains_key("workspace"));
        assert!(files.iter().any(|f| f.path == ".ibc/sdk/rust/src/lib.rs"));
        assert!(files.iter().any(|f| f.path == ".ibc/sdk/abi/src/lib.rs"));
        assert!(files.iter().any(|f| f.path == ".ibc/include/ibc_abi.h"));
    }

    #[test]
    fn kit_has_maps_and_docs() {
        assert!(maps().any(|(name, _)| name == "crossfire.ibcmap"));
        let docs = doc_files();
        for want in [
            "docs/RULES.md",
            "docs/ABI.md",
            "docs/PARTICIPANTS.md",
            "docs/SDK-RUST.md",
            "docs/SDK-CPP.md",
        ] {
            assert!(
                docs.iter().any(|d| d.path == want),
                "{want} missing from the kit"
            );
        }
        assert!(docs.iter().any(|d| d.path.starts_with("docs/rules/")));
        assert!(docs.iter().any(|d| d.path == "rules/economy.toml"));
    }
}
