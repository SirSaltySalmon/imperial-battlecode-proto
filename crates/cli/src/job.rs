//! One match as a job: inputs are files, outputs are files in a directory. Shared by `run` and `run-batch`.

use anyhow::{anyhow, Context, Result};
use ibc_replay::Summary;
use ibc_runner::{BotSpec, MatchConfig};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct BotInput {
    pub name: String,
    pub wasm: PathBuf,
    /// The language whose `[[language]]` row of the ruleset applies; `None`: what the module says
    /// (`ibc_runner::language_of`).
    pub language: Option<String>,
    /// Replaces the row's fuel multiplier.
    pub fuel_override: Option<f64>,
}

impl BotInput {
    /// The runner's input: the module read from disk, its language resolved.
    pub fn spec(&self) -> Result<BotSpec> {
        let mut spec = BotSpec::new(self.name.clone(), read(&self.wasm)?);
        if let Some(l) = &self.language {
            spec.language = l.clone();
        }
        spec.fuel_override = self.fuel_override;
        Ok(spec)
    }
}

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub bot_a: BotInput,
    pub bot_b: BotInput,
    pub map: PathBuf,
    pub seed: u64,
    pub out_dir: PathBuf,
    pub max_ticks: Option<u32>,
    pub frames: bool,
}

pub struct JobResult {
    pub summary: Summary,
    pub replay_path: PathBuf,
    pub summary_path: PathBuf,
}

/// A bot argument is a `.wasm` file or a bot directory (which is built first).
/// Returns the module path and its language.
pub fn resolve_bot(arg: &Path) -> Result<BotInput> {
    if arg.is_dir() {
        let built = crate::build::build_bot(arg, true)?;
        let name = std::fs::canonicalize(arg)
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "bot".into());
        let language = Some(built.language.name().to_string());
        return Ok(BotInput { name, wasm: built.wasm_path, language, fuel_override: None });
    }
    wasm_bot(arg)
}

/// A bot argument that must already be a `.wasm` file.
pub fn wasm_bot(path: &Path) -> Result<BotInput> {
    if !path.is_file() {
        return Err(anyhow!("{}: no such bot (expected a bot directory or a .wasm file)", path.display()));
    }
    // `<bot>/.ibc/bot.wasm` is named after its bot directory, anything else after the file.
    let in_ibc_dir = path.parent().and_then(|p| p.file_name()).is_some_and(|n| n == ".ibc");
    let named = if in_ibc_dir { path.parent().and_then(|p| p.parent()).unwrap_or(path) } else { path };
    let name = named.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "bot".into());
    Ok(BotInput { name, wasm: path.to_path_buf(), language: None, fuel_override: None })
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Run the match and write `replay.ibcr`, `summary.json`, `team-a.log`, `team-b.log` (and `frames.jsonl`).
pub fn execute(job: &JobSpec) -> Result<JobResult> {
    let map_text = String::from_utf8(read(&job.map)?).map_err(|_| anyhow!("{}: map is not UTF-8", job.map.display()))?;
    let out = ibc_runner::run_match(MatchConfig {
        rules: ibc_engine::Ruleset::builtin(),
        map_text,
        seed: job.seed,
        bots: [job.bot_a.spec()?, job.bot_b.spec()?],
        log_limit: ibc_runner::DEFAULT_LOG_LIMIT,
        max_ticks: job.max_ticks,
        frames: job.frames,
    })?;
    let (replay_path, summary_path) = write_output(&job.out_dir, &out)?;
    Ok(JobResult { summary: out.summary, replay_path, summary_path })
}

/// Write `replay.ibcr`, `summary.json`, `team-a.log`, `team-b.log` (and `frames.jsonl`) of a finished run.
/// Returns the replay and summary paths.
pub fn write_output(out_dir: &Path, out: &ibc_runner::MatchOutput) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let replay_path = out_dir.join("replay.ibcr");
    let summary_path = out_dir.join("summary.json");
    write(&replay_path, &out.replay_bytes)?;
    write(&summary_path, out.summary.to_json().as_bytes())?;
    for (team, file) in ["team-a.log", "team-b.log"].iter().enumerate() {
        let mut log = Vec::new();
        for (unit, _) in out.units.iter().enumerate().filter(|(_, s)| s.team == team as i32) {
            log.extend_from_slice(format!("=== unit {unit} ===\n").as_bytes());
            log.extend_from_slice(&out.logs[unit]);
            if !log.ends_with(b"\n") {
                log.push(b'\n');
            }
        }
        write(&out_dir.join(file), &log)?;
    }
    if let Some(frames) = &out.frames {
        let mut text = String::with_capacity(frames.iter().map(|f| f.len() + 1).sum());
        for f in frames {
            text.push_str(f);
            text.push('\n');
        }
        write(&out_dir.join("frames.jsonl"), text.as_bytes())?;
    }
    Ok((replay_path, summary_path))
}

/// "A 7-3", "draw 6-6", "unfinished 1-0"
pub fn result_line(s: &Summary) -> String {
    let who = match s.outcome.as_str() {
        "a" => format!("A ({}) wins", s.teams[0].name),
        "b" => format!("B ({}) wins", s.teams[1].name),
        other => other.to_string(),
    };
    let faults = if s.faults.is_empty() { String::new() } else { format!(" faults={}", s.faults.len()) };
    format!("{who} {}-{} in {} ticks{faults}", s.score[0], s.score[1], s.ticks)
}
