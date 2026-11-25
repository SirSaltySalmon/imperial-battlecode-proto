//! The replay folder behind `ibc view --replays DIR`: a listing the viewer turns into a menu, and the files.
//!
//! Only `.ibcr` files under the folder are ever read. The listing decodes each replay's header so the menu
//! can say who played, where and how it ended, without the page fetching every file.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// The listing, as JSON.
pub const LIST_ROUTE: &str = "/replays/index.json";
/// Files are served below this prefix, by their path relative to the folder.
pub const FILE_PREFIX: &str = "/replays/";

/// How far below the folder to look, and how many replays to list at most.
const MAX_DEPTH: usize = 4;
const MAX_FILES: usize = 500;

#[derive(Serialize, Debug, PartialEq)]
pub struct Entry {
    /// Path below the folder, with `/` separators.
    pub path: String,
    pub bots: [String; 2],
    pub map: String,
    pub score: [i32; 2],
    /// 0 = team A, 1 = team B, null = draw or unfinished.
    pub winner: Option<i32>,
    pub ticks: u32,
    pub finished: bool,
}

/// Every readable replay under `dir`, sorted by path. Files that do not decode are left out.
pub fn list(dir: &Path) -> Vec<Entry> {
    let mut files = Vec::new();
    collect(dir, 0, &mut files);
    files.sort();
    files.truncate(MAX_FILES);
    files.iter().filter_map(|file| entry(dir, file)).collect()
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for path in read.flatten().map(|e| e.path()) {
        if path.is_dir() {
            if depth < MAX_DEPTH {
                collect(&path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|x| x == "ibcr") {
            out.push(path);
        }
    }
}

fn entry(dir: &Path, file: &Path) -> Option<Entry> {
    let replay = ibc_replay::Replay::decode(&std::fs::read(file).ok()?).ok()?;
    let header = replay.header;
    let relative: Vec<String> =
        file.strip_prefix(dir).ok()?.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let map = header.map_text.lines().find_map(|l| l.strip_prefix("name:")).unwrap_or("").trim().to_string();
    let [a, b] = header.bots;
    Some(Entry {
        path: relative.join("/"),
        bots: [a.name, b.name],
        map,
        score: header.result.score,
        winner: header.result.winner,
        ticks: header.result.ticks,
        finished: header.result.finished,
    })
}

/// The bytes of the replay at `relative` below `dir` (canonical), or `None` if it is not an `.ibcr` file
/// inside that folder.
pub fn read(dir: &Path, relative: &Path) -> Option<Vec<u8>> {
    if relative.extension().is_none_or(|x| x != "ibcr") {
        return None;
    }
    // Resolving links and then checking the prefix keeps a symlink in the folder from leading outside it.
    let real = dir.join(relative).canonicalize().ok()?;
    (real.starts_with(dir) && real.is_file()).then(|| std::fs::read(&real).ok()).flatten()
}
