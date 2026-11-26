//! `ibc replay redact` and `ibc replay records`: the team-private part of a replay.

use anyhow::{bail, Context, Result};
use ibc_replay::Keep;
use std::path::Path;

/// `A`, `B` or `none` (case-insensitive).
pub fn parse_keep(s: &str) -> Result<Keep, String> {
    match s.to_ascii_lowercase().as_str() {
        "a" => Ok(Keep::Team(0)),
        "b" => Ok(Keep::Team(1)),
        "none" => Ok(Keep::None),
        _ => Err(format!("{s:?}: use A, B or none")),
    }
}

/// Write a copy of `input` without the private data (memory writes, logs, fuel, debug drawings) of the teams
/// that are not kept. The copy still verifies.
pub fn redact(input: &Path, output: &Path, keep: Keep) -> Result<()> {
    let replay = crate::load_replay(input)?;
    if let Keep::Team(t) = keep {
        if replay.header.redacted[t] {
            bail!("{}: team {} is already redacted in this replay", input.display(), ["A", "B"][t]);
        }
    }
    let view = replay.view(keep);
    let bytes = view.encode();
    std::fs::write(output, &bytes).with_context(|| format!("writing {}", output.display()))?;
    let hidden: Vec<&str> = (0..2).filter(|&t| view.header.redacted[t]).map(|t| ["A", "B"][t]).collect();
    let whose = if hidden.is_empty() { "no team".to_string() } else { format!("team {}", hidden.join(" and ")) };
    println!("{}: {} bytes, private data of {whose} removed", output.display(), bytes.len());
    Ok(())
}

/// Print the private records of the teams the replay still has, as one JSON object.
pub fn records(file: &Path) -> Result<()> {
    let replay = crate::load_replay(file)?;
    println!("{}", ibc_replay::records_json(&replay, true));
    Ok(())
}
