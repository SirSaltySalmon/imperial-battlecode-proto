//! `ibc ladder`, `ibc matches`, `ibc scrim`, `ibc replay download`.

use super::client::{encode, query, CliError, CliResult, Client};
use super::{auth, missing, s, safe_name, t, Opts};
use serde_json::{json, Value};
use std::path::PathBuf;

/// `{generated_at, teams:[{rank, team_id, team_name, rating, provisional, games, ...}]}`, whether the server
/// answers with a bare array or with `{teams}`; rows keep every field the server sent.
pub fn normalize_ladder(raw: Value, top: Option<usize>) -> Value {
    let generated_at = raw.get("generated_at").cloned().unwrap_or(Value::Null);
    let rows = match raw {
        Value::Array(rows) => rows,
        Value::Object(mut o) => match o.remove("teams") {
            Some(Value::Array(rows)) => rows,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let rows: Vec<Value> = rows
        .into_iter()
        .take(top.unwrap_or(usize::MAX))
        .map(|mut r| {
            if let Some(o) = r.as_object_mut().filter(|o| !o.contains_key("team_name")) {
                let name = o.get("name").cloned().unwrap_or(Value::Null);
                o.insert("team_name".to_string(), name);
            }
            r
        })
        .collect();
    json!({"generated_at": generated_at, "teams": rows})
}

pub fn ladder(opts: &Opts, top: Option<usize>) -> CliResult<()> {
    let out = normalize_ladder(opts.client()?.get_json("ladder")?, top);
    opts.emit(&out, |v| {
        let rows = v["teams"].as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            println!("the ladder is empty");
        }
        for r in &rows {
            let prov = if r["provisional"].as_bool() == Some(true) {
                " (provisional)"
            } else {
                ""
            };
            println!(
                "{:>4}  {:<28} {:>6}  {:>4} games{prov}",
                t(r, "rank"),
                s(r, "team_name"),
                t(r, "rating"),
                t(r, "games")
            );
        }
    });
    Ok(())
}

pub struct MatchArgs {
    pub mine: bool,
    pub team: Option<String>,
    pub series: Option<String>,
    pub kind: Option<String>,
    pub limit: u32,
    pub before: Option<String>,
}

pub fn matches(opts: &Opts, a: MatchArgs) -> CliResult<()> {
    let client = opts.client()?;
    let team = if a.mine {
        Some(auth::my_team_id(&client)?)
    } else {
        a.team
    };
    let q = query(&[
        ("team", team),
        ("kind", a.kind.clone()),
        ("series", a.series),
        ("before", a.before),
        ("limit", Some(a.limit.to_string())),
    ]);
    let mut out = client.get_json(&format!("matches{q}"))?;
    // A server that does not filter by kind yet still gets the right rows (the page may come out shorter).
    if let (Some(kind), Some(rows)) = (
        &a.kind,
        out.get_mut("matches").and_then(Value::as_array_mut),
    ) {
        rows.retain(|m| {
            m.get("kind")
                .and_then(Value::as_str)
                .is_none_or(|k| k == kind)
        });
    }
    opts.emit(&out, |v| {
        let rows = v["matches"].as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            println!("no matches");
        }
        for m in &rows {
            let result = match m["outcome"].as_str() {
                Some(o) => format!("{o} {}", t(m, "score")),
                None => s(m, "state").to_string(),
            };
            println!(
                "{:<24} {:<10} {:<14} {} vs {}  {result}",
                s(m, "match_id"),
                s(m, "kind"),
                s(m, "map"),
                s(&m["bot_a"], "name"),
                s(&m["bot_b"], "name"),
            );
        }
        if let Some(next) = v["next"].as_str() {
            println!("more: --before {next}");
        }
    });
    Ok(())
}

pub struct ScrimArgs {
    pub opponent: String,
    pub games: u32,
    pub maps: Vec<String>,
    pub me: Option<String>,
    pub rematch: Option<String>,
}

pub fn scrim(opts: &Opts, a: ScrimArgs) -> CliResult<()> {
    let client = opts.client()?;
    client.require_token()?;
    let mut body = json!({"opponent_team_id": a.opponent, "games": a.games});
    if !a.maps.is_empty() {
        body["maps"] = json!(a.maps);
    }
    if let Some(me) = &a.me {
        body["my_submission_id"] = json!(me);
    }
    if let Some(series) = &a.rematch {
        body["against_series_id"] = json!(series);
    }
    let out = client.post_json("scrims", &body)?;
    let id = out
        .get("series_id")
        .and_then(Value::as_str)
        .ok_or_else(|| missing("series_id"))?
        .to_string();
    opts.emit(&out, |v| {
        println!(
            "series {id}: {} vs {}, {} game(s), {}",
            s(&v["team_a"], "name"),
            s(&v["team_b"], "name"),
            t(v, "games"),
            s(v, "state")
        );
        println!("follow it with `ibc matches --series {id}`");
    });
    Ok(())
}

/// Fetches a replay. With no `--view` and a stored token, asks for the team view first (your own team's
/// memory and logs kept) and falls back to the public view when the server refuses it, which it does for a
/// match your team did not play.
fn fetch_replay(
    client: &Client,
    match_id: &str,
    view: Option<String>,
) -> CliResult<(Vec<u8>, String)> {
    let get = |v: &str| {
        let q = query(&[("view", Some(v.to_string()))]);
        client.get_bytes(&format!("matches/{}/replay{q}", encode(match_id)))
    };
    match view {
        Some(v) => Ok((get(&v)?, v)),
        None if client.token.is_some() => match get("team") {
            Ok(bytes) => Ok((bytes, "team".into())),
            Err(e) if matches!(e.status, Some(401 | 403)) => Ok((get("public")?, "public".into())),
            Err(e) => Err(e),
        },
        None => Ok((get("public")?, "public".into())),
    }
}

pub fn replay_download(
    opts: &Opts,
    match_id: &str,
    out: Option<PathBuf>,
    view: Option<String>,
) -> CliResult<()> {
    if out.is_none() && !safe_name(match_id) {
        return Err(CliError::usage(format!(
            "{match_id:?} cannot be a file name; pass -o FILE"
        )));
    }
    let client = opts.client()?;
    let (bytes, view) = fetch_replay(&client, match_id, view)?;
    let path = out.unwrap_or_else(|| {
        PathBuf::from("out")
            .join("replays")
            .join(format!("{match_id}.ibcr"))
    });
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| CliError::other(format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(&path, &bytes)
        .map_err(|e| CliError::other(format!("{}: {e}", path.display())))?;
    let mut info = json!({
        "match_id": match_id, "path": path.display().to_string(), "bytes": bytes.len(),
        "sha256": ibc_replay::sha256_hex(&bytes), "view": view,
    });
    // A replay this `ibc` cannot read is still saved; say why so the user can upgrade.
    match ibc_replay::Replay::decode(&bytes) {
        Ok(r) => {
            info["engine_version"] = json!(r.header.engine_version);
            info["result"] = serde_json::to_value(&r.header.result).unwrap_or(Value::Null);
        }
        Err(e) => info["decode_error"] = json!(e.to_string()),
    }
    opts.emit(&info, |v| {
        println!(
            "saved {} ({} bytes, sha256 {})",
            s(v, "path"),
            t(v, "bytes"),
            s(v, "sha256")
        );
        match v.get("decode_error").and_then(Value::as_str) {
            Some(e) => println!("warning: this ibc cannot read it: {e}"),
            None => println!(
                "check it with `ibc replay verify {}`, watch it with `ibc view {}`",
                s(v, "path"),
                s(v, "path")
            ),
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_rows_get_a_team_name_and_odd_rows_pass_through() {
        let v = normalize_ladder(
            json!({"teams": [{"rank": 1, "name": "alpha"}, "odd", {"team_name": "beta"}]}),
            None,
        );
        assert_eq!(
            v["teams"],
            json!([{"rank": 1, "name": "alpha", "team_name": "alpha"}, "odd", {"team_name": "beta"}])
        );
        let v = normalize_ladder(json!([{"team_name": "a"}, {"team_name": "b"}]), Some(1));
        assert_eq!(
            v,
            json!({"generated_at": null, "teams": [{"team_name": "a"}]})
        );
    }
}
