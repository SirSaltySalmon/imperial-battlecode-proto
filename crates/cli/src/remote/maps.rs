//! `ibc maps pull`: copy the platform's map pool into a local directory. Files are only ever created or
//! replaced under the pool's own names; a custom map with another name is never touched.

use super::client::{encode, CliError, CliResult, EXIT_ERROR};
use super::{s, safe_name, Opts};
use serde_json::{json, Value};
use std::path::Path;

fn file_name(name: &str) -> String {
    if name.ends_with(".ibcmap") {
        name.to_string()
    } else {
        format!("{name}.ibcmap")
    }
}

pub fn pull(opts: &Opts, dir: &Path, all: bool) -> CliResult<()> {
    let client = opts.client()?;
    let list = client.get_json("maps")?;
    let maps = list
        .get("maps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    std::fs::create_dir_all(dir).map_err(|e| CliError::other(format!("{}: {e}", dir.display())))?;
    let mut rows = Vec::new();
    for m in maps
        .iter()
        .filter(|m| all || m.get("in_pool").and_then(Value::as_bool) != Some(false))
    {
        let name = m.get("name").and_then(Value::as_str).unwrap_or("");
        let row = match pull_one(&client, dir, name, m.get("sha256").and_then(Value::as_str)) {
            Ok(row) => row,
            // No map can be fetched from a server without the endpoint: say so once.
            Err(e) if e.code == "endpoint_missing" => return Err(e),
            Err(e) => {
                json!({"name": name, "status": "failed", "error": e.code, "message": e.message})
            }
        };
        rows.push(row);
    }
    let failed = rows.iter().filter(|r| r["status"] == "failed").count();
    let out = json!({"dir": dir.display().to_string(), "maps": rows});
    opts.emit(&out, |v| {
        for r in v["maps"].as_array().into_iter().flatten() {
            match r["status"].as_str() {
                Some("failed") => println!("failed    {}: {}", s(r, "name"), s(r, "message")),
                Some(status) => println!("{status:<9} {}", s(r, "path")),
                None => {}
            }
        }
        if v["maps"].as_array().is_some_and(Vec::is_empty) {
            println!("the server's map pool is empty");
        }
    });
    if failed > 0 {
        // Under --json the per-map report on stdout is the answer and the exit code says some failed.
        if opts.json {
            std::process::exit(EXIT_ERROR);
        }
        return Err(CliError::new(
            EXIT_ERROR,
            "maps_failed",
            format!("{failed} map(s) could not be pulled"),
        ));
    }
    Ok(())
}

fn pull_one(
    client: &super::client::Client,
    dir: &Path,
    name: &str,
    sha256: Option<&str>,
) -> CliResult<Value> {
    if !safe_name(name) {
        return Err(CliError::new(
            EXIT_ERROR,
            "bad_name",
            format!("refusing the map name {name:?}"),
        ));
    }
    let text = client.get_bytes(&format!("maps/{}", encode(name)))?;
    let got = ibc_replay::sha256_hex(&text);
    if let Some(want) = sha256.filter(|w| *w != got) {
        return Err(CliError::new(
            EXIT_ERROR,
            "hash_mismatch",
            format!("sha256 {got}, the list says {want}"),
        ));
    }
    let path = dir.join(file_name(name));
    let status = match std::fs::read(&path) {
        Ok(old) if old == text => "unchanged",
        Ok(_) => "updated",
        Err(_) => "new",
    };
    if status != "unchanged" {
        std::fs::write(&path, &text)
            .map_err(|e| CliError::other(format!("{}: {e}", path.display())))?;
    }
    Ok(json!({"name": name, "path": path.display().to_string(), "sha256": got, "status": status}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_could_escape_the_directory_are_refused() {
        assert!(safe_name("crossfire"));
        assert!(safe_name("v2.arena_b-1"));
        for bad in ["", "..", ".hidden", "a/b", "a\\b", "c:x", "x y"] {
            assert!(!safe_name(bad), "{bad:?}");
        }
        assert_eq!(file_name("crossfire"), "crossfire.ibcmap");
        assert_eq!(file_name("crossfire.ibcmap"), "crossfire.ibcmap");
    }
}
