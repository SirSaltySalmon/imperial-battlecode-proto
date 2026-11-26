//! A participant workspace: a directory with an `ibc.toml` that has an `[ibc]` table (written by `ibc init`).
//! Found by walking up from a path, like cargo finds `Cargo.toml`. (A bot directory may hold its own
//! `ibc.toml` with build settings; that one has no `[ibc]` table and is not a workspace.)

use crate::embedded;
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const CONFIG: &str = "ibc.toml";

/// The workspace containing `start` (a file or directory, relative to the current directory or absolute).
/// An `ibc.toml` that does not parse is reported on stderr and skipped, so the error is not mistaken for
/// "no workspace here".
pub fn find(start: &Path) -> Option<PathBuf> {
    let start = std::path::absolute(start).ok()?;
    start
        .ancestors()
        .find(|dir| match load(dir) {
            Ok(t) => t.contains_key("ibc"),
            Err(e) if dir.join(CONFIG).is_file() => {
                eprintln!("warning: {e:#}");
                false
            }
            Err(_) => false,
        })
        .map(Path::to_path_buf)
}

fn load(root: &Path) -> Result<toml::Table> {
    let path = root.join(CONFIG);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn string<'a>(table: &'a toml::Table, section: &str, key: &str) -> Option<&'a str> {
    table.get(section)?.get(key)?.as_str()
}

/// Point the `[ibc] version` pin of `root/ibc.toml` at `version`, keeping the rest of the file as written.
/// Returns the previous pin when it changed.
pub fn set_pin(root: &Path, version: &str) -> Result<Option<String>> {
    let path = root.join(CONFIG);
    let old = string(&load(root)?, "ibc", "version").map(str::to_string);
    if old.as_deref() == Some(version) {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    let (mut out, mut section, mut done) = (String::new(), String::new(), false);
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if section == "ibc" && !done {
                out.push_str(&format!("version = \"{version}\"\n"));
                done = true;
            }
            // `[ibc]  # comment` names the table `ibc`.
            let header = trimmed.split('#').next().unwrap_or_default().trim();
            section = header
                .trim_matches(|c| c == '[' || c == ']')
                .trim()
                .to_string();
        }
        let is_pin = section == "ibc"
            && trimmed
                .split_once('=')
                .is_some_and(|(k, _)| k.trim() == "version");
        if is_pin && !done {
            out.push_str(&format!(
                "version = \"{version}\"{}",
                if line.ends_with('\n') { "\n" } else { "" }
            ));
            done = true;
        } else {
            out.push_str(line);
        }
    }
    if section == "ibc" && !done {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("version = \"{version}\"\n"));
        done = true;
    }
    if !done {
        bail!(
            "{}: found no `[ibc]` table header to move the pin under; set `version = \"{version}\"` in it by hand",
            path.display()
        );
    }
    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(Some(old.unwrap_or_default()))
}

/// Warnings for building in a workspace whose kit does not match this binary (empty when it matches,
/// or when `bot_dir` is not in a workspace at all).
pub fn staleness(bot_dir: &Path) -> Vec<String> {
    let Some(root) = find(bot_dir) else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    let me = env!("CARGO_PKG_VERSION");
    if let Some(pin) = load(&root)
        .ok()
        .as_ref()
        .and_then(|t| string(t, "ibc", "version"))
    {
        if pin != me {
            warnings.push(format!(
                "this workspace is pinned to ibc {pin} but this is ibc {me}: install {pin}, or run `ibc init` to move the pin"
            ));
        }
    }
    match std::fs::read_to_string(root.join(embedded::MARKER)) {
        Ok(marker) if marker == embedded::marker() => {}
        Ok(marker) => {
            let from = marker.lines().next().unwrap_or("an unknown ibc");
            warnings.push(format!(
                "{} was written by {from} and does not match this ibc {me}: run `ibc init` to refresh the SDK",
                root.join(".ibc").display()
            ));
        }
        Err(_) => warnings.push(format!(
            "{} has no .ibc/ SDK (a fresh clone?): run `ibc init` there before building",
            root.display()
        )),
    }
    warnings
}

/// Print `staleness` warnings for the workspace around `bot_dir`, once per workspace per process (`ibc run`
/// builds two bots, a batch many).
pub fn warn_if_stale(bot_dir: &Path) {
    static WARNED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    let Some(root) = find(bot_dir) else { return };
    if !WARNED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(root)
    {
        return;
    }
    for warning in staleness(bot_dir) {
        eprintln!("warning: {warning}");
    }
}

/// Fill in `ibc run`'s second bot and map from the `[run]` table of the workspace around the current
/// directory when they were not given.
pub fn run_defaults(bot_b: Option<PathBuf>, map: Option<PathBuf>) -> Result<(PathBuf, PathBuf)> {
    if let (Some(b), Some(m)) = (&bot_b, &map) {
        return Ok((b.clone(), m.clone()));
    }
    let Some(root) = find(Path::new(".")) else {
        bail!("give the second bot and --map, or run inside a workspace made by `ibc init` (its ibc.toml has defaults)");
    };
    let config = load(&root)?;
    let default = |key: &str| -> Result<PathBuf> {
        match string(&config, "run", key) {
            Some(p) => Ok(root.join(p)),
            None => bail!(
                "{}: no [run] {key} to default to",
                root.join(CONFIG).display()
            ),
        }
    };
    let bot_b = match bot_b {
        Some(b) => b,
        None => default("opponent")?,
    };
    let map = match map {
        Some(m) => m,
        None => default("map")?,
    };
    Ok((bot_b, map))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_pin_rewrites_only_the_pin() {
        let dir = crate::init::tests::temp_dir("pin");
        let text = "# mine\n[ibc]  # pinned [by hand]\n# comment\nversion = \"0.0.1\"\n\n[run]\nmap = \"maps/x.ibcmap\"\nversion = \"keep\"\n";
        std::fs::write(dir.join(CONFIG), text).unwrap();
        assert_eq!(set_pin(&dir, "9.9.9").unwrap().as_deref(), Some("0.0.1"));
        let after = std::fs::read_to_string(dir.join(CONFIG)).unwrap();
        assert_eq!(after, text.replace("0.0.1", "9.9.9"));
        assert_eq!(set_pin(&dir, "9.9.9").unwrap(), None);
        assert_eq!(
            find(&dir.join("bots").join("x")),
            Some(std::path::absolute(&dir).unwrap())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn set_pin_adds_a_missing_pin() {
        let dir = crate::init::tests::temp_dir("pin-missing");
        std::fs::write(dir.join(CONFIG), "[ibc]\n[run]\nmap = \"m\"\n").unwrap();
        set_pin(&dir, "1.2.3").unwrap();
        let t = load(&dir).unwrap();
        assert_eq!(string(&t, "ibc", "version"), Some("1.2.3"));
        assert_eq!(string(&t, "run", "map"), Some("m"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bot_config_is_not_a_workspace() {
        let dir = crate::init::tests::temp_dir("not-ws");
        std::fs::write(dir.join(CONFIG), "[cpp]\nexceptions = true\n").unwrap();
        // The temp dir's ancestors have no ibc.toml either.
        assert_eq!(find(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
