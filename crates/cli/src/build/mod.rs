//! `ibc build`: turn a bot directory into `<bot_dir>/.ibc/bot.wasm`.
//!
//! One module per language. `detect` picks the language from the files in the directory and `build_bot`
//! dispatches; to add a language, add a variant to `Language`, a detection rule, and a module with a
//! `build(dir, release) -> Result<PathBuf>` function that returns the freshly built wasm.

mod cpp;
mod rust;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust,
    Cpp,
}

impl Language {
    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Cpp => "cpp",
        }
    }

    /// Default fuel multiplier for bots written in this language.
    pub fn fuel_multiplier(self) -> f64 {
        match self {
            Language::Rust | Language::Cpp => 1.0,
        }
    }
}

pub struct BuiltBot {
    pub language: Language,
    /// `<bot_dir>/.ibc/bot.wasm`
    pub wasm_path: PathBuf,
    pub sha256: String,
}

pub fn detect(bot_dir: &Path) -> Result<Language> {
    if !bot_dir.is_dir() {
        bail!("{} is not a directory", bot_dir.display());
    }
    if bot_dir.join("Cargo.toml").is_file() {
        return Ok(Language::Rust);
    }
    if cpp::is_cpp_bot(bot_dir) {
        return Ok(Language::Cpp);
    }
    bail!("{}: cannot tell the bot's language (expected a Cargo.toml for a Rust bot or src/*.cpp for a C++ bot)", bot_dir.display())
}

/// Build the bot in `bot_dir` and install the module at `<bot_dir>/.ibc/bot.wasm`.
pub fn build_bot(bot_dir: &Path, release: bool) -> Result<BuiltBot> {
    let language = detect(bot_dir)?;
    // ---- language dispatch ----
    let artifact = match language {
        Language::Rust => rust::build(bot_dir, release)?,
        Language::Cpp => cpp::build(bot_dir, release)?,
    };
    let wasm = std::fs::read(&artifact).with_context(|| format!("reading {}", artifact.display()))?;
    ibc_runner::check_wasm(&wasm).with_context(|| format!("{} is not a usable bot", artifact.display()))?;
    let out_dir = bot_dir.join(".ibc");
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let wasm_path = out_dir.join("bot.wasm");
    // Leave an identical file alone so its mtime stays meaningful.
    if std::fs::read(&wasm_path).ok().as_deref() != Some(&wasm[..]) {
        std::fs::write(&wasm_path, &wasm).with_context(|| format!("writing {}", wasm_path.display()))?;
    }
    Ok(BuiltBot { language, wasm_path, sha256: ibc_replay::sha256_hex(&wasm) })
}
