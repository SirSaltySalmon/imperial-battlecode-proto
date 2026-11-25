//! Small helpers shared by the tasks: paths, running commands, reading and writing files.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

/// The repository root (the directory that holds the workspace `Cargo.toml`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in <root>/xtask")
        .to_path_buf()
}

/// Where cargo puts build output for this workspace.
pub fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) if !dir.is_empty() => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                dir
            } else {
                root().join(dir)
            }
        }
        _ => root().join("target"),
    }
}

/// A `cargo` command that runs in the repository root.
pub fn cargo() -> Command {
    let mut cmd =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    cmd.current_dir(root());
    cmd
}

fn describe(cmd: &Command) -> String {
    let mut s = cmd.get_program().to_string_lossy().into_owned();
    for a in cmd.get_args() {
        s.push(' ');
        s.push_str(&a.to_string_lossy());
    }
    s
}

/// Run a command with inherited output; an error names the command.
pub fn run(cmd: &mut Command) -> Result<()> {
    eprintln!("$ {}", describe(cmd));
    let status = cmd
        .status()
        .map_err(|e| format!("could not start `{}`: {e}", describe(cmd)))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{}` failed ({status})", describe(cmd)))
    }
}

/// Run a command and return its stdout; stderr is inherited.
pub fn capture(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("could not start `{}`: {e}", describe(cmd)))?;
    if !out.status.success() {
        return Err(format!("`{}` failed ({})", describe(cmd), out.status));
    }
    String::from_utf8(out.stdout).map_err(|_| format!("`{}` printed invalid UTF-8", describe(cmd)))
}

/// True if `program --version` can be started and succeeds.
pub fn has_program(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write a file, creating its directory. Text is written with LF line ends as given.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// `name` or `name.exe`, depending on the platform the file is for.
pub fn exe_name(name: &str, windows: bool) -> String {
    if windows {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// A path relative to the repository root, with forward slashes, for messages and committed files.
pub fn rel(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Flags of a task: `--name value` pairs, bare `--switch`es and positional words.
pub struct Args {
    words: Vec<String>,
}

impl Args {
    pub fn new(words: &[String]) -> Args {
        Args {
            words: words.to_vec(),
        }
    }

    /// Remove `--name` and report whether it was there.
    pub fn switch(&mut self, name: &str) -> bool {
        let before = self.words.len();
        self.words.retain(|w| w != name);
        self.words.len() != before
    }

    /// Remove every `--name value` and return the values.
    pub fn values(&mut self, name: &str) -> Result<Vec<String>> {
        let mut found = Vec::new();
        while let Some(i) = self.words.iter().position(|w| w == name) {
            if i + 1 >= self.words.len() {
                return Err(format!("{name} needs a value"));
            }
            found.push(self.words.remove(i + 1));
            self.words.remove(i);
        }
        Ok(found)
    }

    /// Remove `--name value`; the last one wins.
    pub fn value(&mut self, name: &str) -> Result<Option<String>> {
        Ok(self.values(name)?.pop())
    }

    /// What is left: positional words. Unknown flags are an error.
    pub fn rest(self) -> Result<Vec<String>> {
        match self.words.iter().find(|w| w.starts_with("--")) {
            Some(flag) => Err(format!("unknown flag {flag}")),
            None => Ok(self.words),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Args {
        Args::new(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn flags_are_taken_out_and_the_rest_is_positional() {
        let mut a = args("verify --native --exclude a extra --exclude b");
        assert!(a.switch("--native"));
        assert!(!a.switch("--wasm"));
        assert_eq!(a.values("--exclude").unwrap(), ["a", "b"]);
        assert_eq!(a.rest().unwrap(), ["verify", "extra"]);
    }

    #[test]
    fn unknown_flags_and_missing_values_are_errors() {
        assert!(args("--bogus").rest().is_err());
        assert!(args("--out").value("--out").is_err());
    }
}
