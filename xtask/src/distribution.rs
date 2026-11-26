//! `distribution.json`: what the coordinator serves at `GET /v1/distribution` (set `IBC_DISTRIBUTION` to
//! the file) and what `ibc update` reads to tell a participant how to upgrade. One entry per release
//! archive found next to it, with its download URL and SHA-256.
//!
//! `cargo xtask dist` rewrites it after every archive it packs; `cargo xtask dist distribution [dir]`
//! rewrites it from whatever archives are in `dir` (the release job runs it once over all six targets).

use crate::patch_id::PatchId;
use crate::util::{self, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "distribution.json";
/// The pip package that installs the same `ibc` (packaging/pip).
pub const PIP_PACKAGE: &str = "imperial-battlecode";

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    pub target: String,
    pub file: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Distribution {
    /// Version of `ibc` (the engine version).
    pub version: String,
    pub patch: PatchId,
    /// `pip install --upgrade <pip>` installs exactly this version.
    pub pip: String,
    pub artifacts: Vec<Artifact>,
}

/// The default download location: the GitHub Release of tag `v<version>`.
pub fn default_base_url(version: &str) -> String {
    format!(
        "https://github.com/SirSaltySalmon/imperial-battlecode-proto/releases/download/v{version}"
    )
}

/// The target triple of an archive `ibc-<version>-<target>.tar.gz|.zip` of this version.
fn target_of<'a>(file: &'a str, version: &str) -> Option<&'a str> {
    let rest = file.strip_prefix(&format!("ibc-{version}-"))?;
    let target = rest
        .strip_suffix(".tar.gz")
        .or_else(|| rest.strip_suffix(".zip"))?;
    (!target.is_empty() && !target.contains('/')).then_some(target)
}

/// Scan `dir` for this version's archives.
pub fn build(dir: &Path, version: &str, patch: &PatchId, base_url: &str) -> Result<Distribution> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let base = base_url.trim_end_matches('/');
    let mut artifacts = Vec::new();
    for name in names {
        let Some(target) = target_of(&name, version) else {
            continue;
        };
        let bytes = util::read(&dir.join(&name))?;
        artifacts.push(Artifact {
            target: target.to_string(),
            url: format!("{base}/{name}"),
            sha256: ibc_replay::sha256_hex(&bytes),
            size: bytes.len() as u64,
            file: name,
        });
    }
    if artifacts.is_empty() {
        return Err(format!(
            "{}: no ibc-{version}-<target> archives",
            dir.display()
        ));
    }
    Ok(Distribution {
        version: version.to_string(),
        patch: patch.clone(),
        pip: format!("{PIP_PACKAGE}=={version}"),
        artifacts,
    })
}

pub fn write(dir: &Path, version: &str, patch: &PatchId, base_url: &str) -> Result<()> {
    let dist = build(dir, version, patch, base_url)?;
    let mut text = serde_json::to_string_pretty(&dist).map_err(|e| e.to_string())?;
    text.push('\n');
    util::write(&dir.join(FILE), text.as_bytes())?;
    println!(
        "{} ({} artifact{})",
        dir.join(FILE).display(),
        dist.artifacts.len(),
        if dist.artifacts.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch() -> PatchId {
        PatchId {
            engine: "0.3.1".into(),
            ruleset_hash: "00d2cae8a4f4c61b".into(),
            wasmtime: "36.0.2".into(),
            abi: 1,
        }
    }

    #[test]
    fn archive_names_give_their_target() {
        assert_eq!(
            target_of("ibc-0.3.1-x86_64-unknown-linux-musl.tar.gz", "0.3.1"),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            target_of("ibc-0.3.1-x86_64-pc-windows-msvc.zip", "0.3.1"),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(
            target_of("ibc-0.3.0-x86_64-pc-windows-msvc.zip", "0.3.1"),
            None
        );
        assert_eq!(
            target_of("imperial_battlecode-0.3.1-py3-none-win_amd64.whl", "0.3.1"),
            None
        );
        assert_eq!(target_of("ibc-0.3.1-.zip", "0.3.1"), None);
    }

    #[test]
    fn lists_this_versions_archives_with_url_and_sha256() {
        let dir =
            std::env::temp_dir().join(format!("ibc-xtask-distribution-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, body) in [
            ("ibc-0.3.1-x86_64-pc-windows-msvc.zip", "abc"),
            ("ibc-0.3.1-aarch64-apple-darwin.tar.gz", "b"),
            ("ibc-0.3.0-aarch64-apple-darwin.tar.gz", "old"),
            ("SHA256SUMS", "sums"),
        ] {
            util::write(&dir.join(name), body.as_bytes()).unwrap();
        }
        write(&dir, "0.3.1", &patch(), "https://example.org/v0.3.1/").unwrap();
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let d: Distribution = serde_json::from_str(&text).unwrap();
        assert_eq!(d.version, "0.3.1");
        assert_eq!(d.pip, "imperial-battlecode==0.3.1");
        assert_eq!(d.patch, patch());
        assert_eq!(
            d.artifacts,
            [
                Artifact {
                    target: "aarch64-apple-darwin".into(),
                    file: "ibc-0.3.1-aarch64-apple-darwin.tar.gz".into(),
                    url: "https://example.org/v0.3.1/ibc-0.3.1-aarch64-apple-darwin.tar.gz".into(),
                    sha256: "3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d"
                        .into(),
                    size: 1,
                },
                Artifact {
                    target: "x86_64-pc-windows-msvc".into(),
                    file: "ibc-0.3.1-x86_64-pc-windows-msvc.zip".into(),
                    url: "https://example.org/v0.3.1/ibc-0.3.1-x86_64-pc-windows-msvc.zip".into(),
                    sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                        .into(),
                    size: 3,
                },
            ]
        );
        assert!(build(Path::new("."), "9.9.9", &patch(), "x").is_err());
    }
}
