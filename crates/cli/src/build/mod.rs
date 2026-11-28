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

/// Name of the custom section that declares a module's language; the coordinator checks an upload's
/// `?language=` against it, and the runner picks the bot's `[[language]]` row of the ruleset by it.
use ibc_replay::LANGUAGE_SECTION;

/// `wasm` with a custom section `ibc.language` holding the language's name appended. Custom sections may
/// come anywhere and never change what the module does.
fn with_language_section(mut wasm: Vec<u8>, language: Language) -> Vec<u8> {
    fn leb(out: &mut Vec<u8>, mut v: usize) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            out.push(if v == 0 { byte } else { byte | 0x80 });
            if v == 0 {
                break;
            }
        }
    }
    let mut body = Vec::new();
    leb(&mut body, LANGUAGE_SECTION.len());
    body.extend_from_slice(LANGUAGE_SECTION.as_bytes());
    body.extend_from_slice(language.name().as_bytes());
    wasm.push(0);
    leb(&mut wasm, body.len());
    wasm.extend_from_slice(&body);
    wasm
}

/// Build the bot in `bot_dir` and install the module at `<bot_dir>/.ibc/bot.wasm`.
pub fn build_bot(bot_dir: &Path, release: bool) -> Result<BuiltBot> {
    let language = detect(bot_dir)?;
    crate::workspace::warn_if_stale(bot_dir);
    // ---- language dispatch ----
    let artifact = match language {
        Language::Rust => rust::build(bot_dir, release)?,
        Language::Cpp => cpp::build(bot_dir, release)?,
    };
    let wasm = std::fs::read(&artifact).with_context(|| format!("reading {}", artifact.display()))?;
    let wasm = with_language_section(wasm, language);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_section_is_a_well_formed_custom_section() {
        let empty = b"\0asm\x01\0\0\0".to_vec();
        let out = with_language_section(empty.clone(), Language::Cpp);
        let mut expect = empty;
        expect.extend_from_slice(&[0, 16, 12]);
        expect.extend_from_slice(b"ibc.languagecpp");
        assert_eq!(out, expect);
    }
}
