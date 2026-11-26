//! `ibc new NAME [--lang rust|cpp] [--template starter|example]`: create `bots/NAME` in the workspace
//! around the current directory from a template embedded in this binary.
//!
//! `starter` is a small bot to grow from (`templates/`); `example` is a complete reference bot (`rusher`
//! for Rust, `cpp-starter` for C++), taken from the repo's own bots so it never drifts from them.

use crate::{embedded, workspace};
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Lang {
    Rust,
    Cpp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Template {
    /// A small bot to grow from: buys, walks to a site, shoots, plants and defuses
    Starter,
    /// A complete reference bot (rusher for Rust, cpp-starter for C++)
    Example,
}

#[derive(clap::Args)]
pub struct NewArgs {
    /// Bot name: lowercase letters, digits, `-` and `_`, starting with a letter
    name: String,
    #[arg(long, value_enum, default_value_t = Lang::Rust)]
    lang: Lang,
    #[arg(long, value_enum, default_value_t = Template::Starter)]
    template: Template,
}

/// Crate names (with `-` read as `_`) that clash with the SDK crates or the Rust standard library.
const RESERVED: &[&str] = &[
    "ibc",
    "ibc_abi",
    "std",
    "core",
    "alloc",
    "test",
    "proc_macro",
];

/// Rust keywords, strict and reserved: a crate named after one cannot be named in `ibc::bot!(name::Bot)`.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "try", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield",
];

pub fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let ok = name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        bail!("bot name {name:?}: use up to 64 lowercase letters, digits, `-` and `_`, starting with a letter");
    }
    let krate = name.replace('-', "_");
    if RESERVED.contains(&krate.as_str()) || KEYWORDS.contains(&krate.as_str()) {
        bail!("bot name {name:?} is taken by the SDK, the Rust standard library or a Rust keyword; pick another");
    }
    if is_windows_device(name) {
        bail!("bot name {name:?} is a reserved device name on Windows; pick another");
    }
    Ok(())
}

/// `con`, `nul`, `com1` and friends cannot be directory names on Windows.
fn is_windows_device(name: &str) -> bool {
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit() && n != "0")
    };
    ["con", "prn", "aux", "nul"].contains(&name) || numbered("com") || numbered("lpt")
}

/// The opponent and map in the C++ example's "Build and run" line; `ibc run` takes both from ibc.toml.
const EXAMPLE_CPP_OPPONENT: &str = " bots/rusher --map maps/tunnels.ibcmap";

fn fill(text: &str, vars: &[(&str, &str)]) -> String {
    vars.iter().fold(text.to_string(), |t, (k, v)| {
        t.replace(&format!("{{{{{k}}}}}"), v)
    })
}

/// The files of a new bot called `name`, as (path relative to the bot directory, content).
pub fn render(name: &str, lang: Lang, template: Template) -> Vec<(String, String)> {
    let krate = name.replace('-', "_");
    let tpl = |path: &str| embedded::file(&format!("crates/cli/templates/{path}"));
    match lang {
        Lang::Rust => {
            let (lib, ty) = match template {
                Template::Starter => (tpl("rust/starter.rs"), "Player"),
                Template::Example => (embedded::file("bots/rusher/src/lib.rs"), "Rusher"),
            };
            let vars = [("name", name), ("crate", krate.as_str()), ("type", ty)];
            vec![
                ("Cargo.toml".into(), fill(tpl("rust/Cargo.toml"), &vars)),
                ("src/lib.rs".into(), fill(lib, &vars)),
                ("src/main.rs".into(), fill(tpl("rust/main.rs"), &vars)),
            ]
        }
        Lang::Cpp => {
            let main = match template {
                Template::Starter => fill(
                    tpl("cpp/starter.cpp"),
                    &[("name", name), ("type", "Player")],
                ),
                // Its header names its own directory and a repo opponent the workspace does not have.
                Template::Example => embedded::file("bots/cpp-starter/src/main.cpp")
                    .replace(EXAMPLE_CPP_OPPONENT, "")
                    .replace("cpp-starter", name),
            };
            vec![
                ("src/main.cpp".into(), main),
                (
                    "compile_flags.txt".into(),
                    tpl("cpp/compile_flags.txt").to_string(),
                ),
            ]
        }
    }
}

/// Create `root/bots/name`. Fails if it exists.
pub fn create(root: &Path, name: &str, lang: Lang, template: Template) -> Result<PathBuf> {
    validate_name(name)?;
    let dir = root.join("bots").join(name);
    if dir.exists() {
        bail!("{} already exists", dir.display());
    }
    for (rel, content) in render(name, lang, template) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap())
            .with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(dir)
}

pub fn run(args: NewArgs) -> Result<()> {
    let Some(root) = workspace::find(Path::new(".")) else {
        bail!("not inside an ibc workspace: run `ibc init` first (it writes the SDK the bot builds against)");
    };
    let dir = create(&root, &args.name, args.lang, args.template)?;
    // As a path from the current directory when it is below it (the usual case: run from the root).
    let cwd = std::path::absolute(".")?;
    let shown = dir.strip_prefix(&cwd).unwrap_or(&dir).display();
    println!("created {shown}");
    println!("build it:  ibc build {shown}");
    println!("play it:   ibc run {shown}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for good in ["mybot", "my-bot", "b2_x", "console", "com0", "com12"] {
            validate_name(good).unwrap();
        }
        for bad in [
            "",
            "2bot",
            "MyBot",
            "my bot",
            "../x",
            "ibc",
            "ibc-abi",
            "std",
            "ref",
            "self",
            "con",
            "com1",
            &"a".repeat(65),
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn every_template_renders_completely() {
        for lang in [Lang::Rust, Lang::Cpp] {
            for template in [Template::Starter, Template::Example] {
                let files = render("my-bot", lang, template);
                for (path, content) in &files {
                    assert!(
                        !content.contains("{{"),
                        "{lang:?}/{template:?} {path} has an unfilled placeholder"
                    );
                }
                let names: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
                match lang {
                    Lang::Rust => {
                        assert_eq!(names, ["Cargo.toml", "src/lib.rs", "src/main.rs"]);
                        let manifest: toml::Table = toml::from_str(&files[0].1).unwrap();
                        assert_eq!(manifest["package"]["name"].as_str(), Some("my-bot"));
                        let dep = manifest["dependencies"]["ibc"]["path"].as_str();
                        assert_eq!(dep, Some("../../.ibc/sdk/rust"));
                        assert!(files[2].1.contains("ibc::bot!(my_bot::"));
                    }
                    Lang::Cpp => {
                        assert!(names.contains(&"src/main.cpp"));
                        assert!(files[0].1.contains("IBC_BOT("));
                        assert!(!files[0].1.contains("bots/rusher"));
                        assert!(!files[0].1.contains("cpp-starter"));
                    }
                }
            }
        }
    }
}
