//! `ibc submit` and `ibc submissions`.

use super::client::{encode, query, Body, CliError, CliResult};
use super::{s, Opts};
use crate::build;
use serde_json::{json, Value};
use std::path::Path;

/// The module to upload: built from a bot directory, or a prebuilt `.wasm`.
struct Module {
    bytes: Vec<u8>,
    sha256: String,
    language: String,
    path: String,
}

fn module(bot: &Path, language: Option<String>) -> CliResult<Module> {
    if bot.is_file() {
        let bytes =
            std::fs::read(bot).map_err(|e| CliError::other(format!("{}: {e}", bot.display())))?;
        ibc_runner::check_wasm(&bytes).map_err(|e| {
            CliError::other(format!("{} is not a usable bot: {e:#}", bot.display()))
        })?;
        let sha256 = ibc_replay::sha256_hex(&bytes);
        let language = language.unwrap_or_else(|| "rust".to_string());
        return Ok(Module {
            bytes,
            sha256,
            language,
            path: bot.display().to_string(),
        });
    }
    if language.is_some() {
        return Err(CliError::usage(
            "--language is only for a prebuilt .wasm; a bot directory's language is detected",
        ));
    }
    eprintln!("building {} (release)...", bot.display());
    let built = build::build_bot(bot, true)
        .map_err(|e| CliError::new(1, "build_failed", format!("{e:#}")))?;
    let bytes = std::fs::read(&built.wasm_path)
        .map_err(|e| CliError::other(format!("{}: {e}", built.wasm_path.display())))?;
    Ok(Module {
        bytes,
        sha256: built.sha256,
        language: built.language.name().to_string(),
        path: built.wasm_path.display().to_string(),
    })
}

pub fn submit(
    opts: &Opts,
    bot: &Path,
    live: bool,
    note: Option<String>,
    language: Option<String>,
) -> CliResult<()> {
    let client = opts.client()?;
    client.require_token()?;
    let m = module(bot, language)?;
    let q = query(&[
        ("language", Some(m.language.clone())),
        ("live", Some(live.to_string())),
        ("note", note),
    ]);
    let resp = client.send(
        "POST",
        &format!("submissions{q}"),
        Body::Bytes(&m.bytes, "application/wasm"),
    )?;
    let submission = super::client::json_of(resp)?;
    if submission
        .get("sha256")
        .and_then(Value::as_str)
        .is_some_and(|h| h != m.sha256)
    {
        eprintln!(
            "warning: the server stored sha256 {} but the local module is {}",
            s(&submission, "sha256"),
            m.sha256
        );
    }
    let out = json!({
        "module": {"path": m.path, "sha256": m.sha256, "language": m.language, "bytes": m.bytes.len()},
        "submission": submission,
    });
    opts.emit(&out, |v| {
        let sub = &v["submission"];
        println!(
            "submitted {} ({} bytes, sha256 {}): {}",
            s(sub, "submission_id"),
            m.bytes.len(),
            m.sha256,
            s(sub, "status")
        );
        if sub["live"].as_bool() == Some(true) {
            println!("it is now your live submission");
        } else {
            println!(
                "not live; make it live with `ibc submissions --live {}`",
                s(sub, "submission_id")
            );
        }
    });
    Ok(())
}

pub fn submissions(opts: &Opts, live: Option<String>) -> CliResult<()> {
    let client = opts.client()?;
    client.require_token()?;
    if let Some(id) = live {
        let out = client.post_json(&format!("submissions/{}/live", encode(&id)), &json!({}))?;
        opts.emit(&out, |v| match v["changed"].as_bool() {
            Some(false) => println!("{id} was already live"),
            _ => println!("{id} is now live; series created from now on use it"),
        });
        return Ok(());
    }
    let out = client.get_json("submissions")?;
    opts.emit(&out, |v| {
        let rows = v["submissions"].as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            println!("no submissions yet: `ibc submit BOT_DIR --live`");
        }
        for r in &rows {
            let mark = if r["live"].as_bool() == Some(true) {
                "*"
            } else {
                " "
            };
            let hash: String = s(r, "sha256").chars().take(12).collect();
            println!(
                "{mark} {:<24} {:<20} {:<9} {:<5} {hash} {}{}",
                s(r, "submission_id"),
                s(r, "created_at"),
                s(r, "status"),
                s(r, "language"),
                r.get("note")
                    .and_then(Value::as_str)
                    .map(|n| format!("\"{n}\" "))
                    .unwrap_or_default(),
                r.get("reason")
                    .and_then(Value::as_str)
                    .map(|x| format!("rejected: {x}"))
                    .unwrap_or_default(),
            );
        }
        if !rows.is_empty() {
            println!("(* = live)");
        }
    });
    Ok(())
}
