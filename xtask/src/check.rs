//! `cargo xtask check [fmt] [clippy] [test] [--exclude <package>]... [--strict]`
//!
//! What CI runs on every PR. With no step named, all three run in order.
//!
//! `cargo fmt --check` and `clippy -D warnings` are enforced on the packages in `ENFORCED` only: the rest
//! of the tree predates this check and is neither rustfmt-clean nor warning-free, and cleaning it up
//! belongs in one dedicated commit, not in a feature PR. Until then the rest of the workspace is still
//! linted (clippy's deny-by-default lints fail the build) and its backlog is printed as warnings.
//! After that commit, pass `--strict` in CI: it enforces both on the whole workspace.

use crate::util::{self, Args, Result};
use std::process::Command;

const ENFORCED: &[&str] = &["xtask"];

fn enforced_packages(cmd: &mut Command) {
    for p in ENFORCED {
        cmd.args(["-p", p]);
    }
}

fn fmt(strict: bool) -> Result<()> {
    if strict {
        return util::run(util::cargo().args(["fmt", "--all", "--", "--check"]));
    }
    let mut enforced = util::cargo();
    enforced.arg("fmt");
    enforced_packages(&mut enforced);
    util::run(enforced.args(["--", "--check"]))?;
    // Advisory: count the files (without printing the diffs) that are not formatted elsewhere.
    let out = util::cargo()
        .args(["fmt", "--all", "--", "--check", "-l"])
        .output()
        .map_err(|e| format!("could not start cargo fmt: {e}"))?;
    let unformatted = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    if unformatted > 0 {
        eprintln!("warning: {unformatted} file(s) outside {ENFORCED:?} are not rustfmt-clean (enforced only with --strict)");
    }
    Ok(())
}

fn with_excludes(cmd: &mut Command, excludes: &[String]) {
    for e in excludes {
        cmd.args(["--exclude", e]);
    }
}

fn clippy(excludes: &[String], strict: bool) -> Result<()> {
    let mut all = util::cargo();
    all.args(["clippy", "--workspace", "--all-targets"]);
    with_excludes(&mut all, excludes);
    if strict {
        return util::run(all.args(["--", "-D", "warnings"]));
    }
    // Without --strict the workspace keeps its warnings; the `ENFORCED` packages deny theirs through the
    // `[lints]` table of their own manifest, because `-D warnings` here would reach every workspace member.
    util::run(&mut all)
}

fn test(excludes: &[String]) -> Result<()> {
    let mut cmd = util::cargo();
    cmd.args(["test", "--workspace"]);
    with_excludes(&mut cmd, excludes);
    util::run(&mut cmd)
}

pub fn main(args: &[String]) -> Result<()> {
    let mut args = Args::new(args);
    let strict = args.switch("--strict");
    let excludes = args.values("--exclude")?;
    let mut steps = args.rest()?;
    if steps.is_empty() {
        steps = vec!["fmt".to_string(), "clippy".to_string(), "test".to_string()];
    }
    for step in &steps {
        match step.as_str() {
            "fmt" => fmt(strict)?,
            "clippy" => clippy(&excludes, strict)?,
            "test" => test(&excludes)?,
            other => {
                return Err(format!(
                    "check: unknown step {other:?} (steps: fmt, clippy, test)"
                ))
            }
        }
    }
    println!("check OK: {}", steps.join(", "));
    Ok(())
}
