//! `ibc admin`: the moderation terminal. A thin client of the coordinator's admin API
//! (`docs/api/moderation.md`, `docs/MODERATION.md`) that prints compact tables, or the raw JSON with `--json`.

use anyhow::{anyhow, bail, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Args)]
pub struct AdminArgs {
    /// Coordinator URL, e.g. https://battlecode.example.org (default: $IBC_COORDINATOR)
    #[arg(long, global = true)]
    url: Option<String>,
    /// Organiser token (default: $IBC_ADMIN_TOKEN)
    #[arg(long, global = true)]
    token: Option<String>,
    /// Print the coordinator's JSON instead of a table
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: AdminCommand,
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Load at a glance: queue, workers, throughput, cost per game, dead letters, switches, limits
    Overview,
    /// Computed flags: upload bursts, duplicate modules, failing submissions, quarantined workers
    Flags,
    /// The audit log of organiser actions, newest first
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// Only entries older than this sequence number (paging)
        #[arg(long)]
        before: Option<i64>,
    },
    /// Teams with their live submission and any suspension or ban
    Teams,
    /// Suspend a team: no matches, no uploads. --until 7d | 12h | 2026-12-01 | 2026-12-01T18:00:00Z
    Suspend {
        /// Team id or name
        team: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        until: Option<String>,
    },
    /// Ban a team until it is unbanned
    Ban {
        team: String,
        #[arg(long)]
        reason: String,
    },
    /// Lift a suspension or ban
    Unban {
        team: String,
        /// Recorded in the audit log
        #[arg(long)]
        reason: Option<String>,
    },
    /// Workers: cores, liveness, trust, quarantine, leased jobs
    Workers,
    /// Give a dead-lettered job another round of attempts
    Requeue { job: String },
    /// The dead-letter list
    Dead,
}

struct Client {
    agent: ureq::Agent,
    base: String,
    token: String,
}

impl Client {
    fn new(args: &AdminArgs) -> Result<Client> {
        let from = |flag: &Option<String>, env: &str| {
            flag.clone()
                .or_else(|| std::env::var(env).ok())
                .filter(|v| !v.is_empty())
        };
        let base = from(&args.url, "IBC_COORDINATOR")
            .ok_or_else(|| anyhow!("no coordinator URL: pass --url or set IBC_COORDINATOR"))?;
        let token = from(&args.token, "IBC_ADMIN_TOKEN")
            .ok_or_else(|| anyhow!("no admin token: pass --token or set IBC_ADMIN_TOKEN"))?;
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .build();
        Ok(Client {
            agent,
            base: base.trim_end_matches('/').to_string(),
            token,
        })
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let req = self
            .agent
            .request(method, &format!("{}{path}", self.base))
            .set("Authorization", &format!("Bearer {}", self.token));
        let res = match body {
            Some(b) => req.send_json(b),
            None => req.call(),
        };
        match res {
            Ok(r) => Ok(r.into_json().unwrap_or(Value::Null)),
            Err(ureq::Error::Status(code, r)) => {
                let retry = r.header("Retry-After").map(str::to_string);
                let body: Value = r.into_json().unwrap_or(Value::Null);
                let mut msg = format!(
                    "{method} {path}: HTTP {code} {}: {}",
                    body["error"].as_str().unwrap_or("error"),
                    body["message"].as_str().unwrap_or("(no message)")
                );
                if let Some(secs) = retry {
                    msg.push_str(&format!(" (retry after {secs} s)"));
                }
                bail!(msg)
            }
            Err(e) => bail!("{method} {path}: {e}"),
        }
    }
}

/// Print rows under a header, columns padded to their widest cell.
fn table(header: &[&str], rows: &[Vec<String>]) {
    let mut width: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for row in rows {
        for (w, cell) in width.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let padded: Vec<String> = cells
            .iter()
            .zip(&width)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        println!("{}", padded.join("  ").trim_end());
    };
    line(header.to_vec());
    for row in rows {
        line(row.iter().map(String::as_str).collect());
    }
    if rows.is_empty() {
        println!("(none)");
    }
}

/// A JSON value as a table cell: strings bare, null as `-`.
fn cell(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn print_overview(o: &Value) {
    let queue: Vec<String> = o["queue"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|q| {
            format!(
                "{} {} {} (oldest {} s)",
                cell(&q["class"]),
                cell(&q["state"]),
                q["jobs"],
                q["oldest_age_secs"]
            )
        })
        .collect();
    let w = &o["workers"];
    let g = &o["games"];
    let t = &o["teams"];
    let l = &o["limits"];
    let pause: Vec<String> = o["pause"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k}={}", cell(v)))
        .collect();
    let rows = [
        ("queue", if queue.is_empty() { "empty".to_string() } else { queue.join(", ") }),
        ("workers", format!("{} live, {} cores ({} quarantined, {} known)", w["live"], w["live_cores"], w["quarantined"], w["total"])),
        ("games", format!("{} last hour, {} last day; {} core-s per game", g["last_hour"], g["last_day"], cell(&o["mean_core_seconds_per_game"]))),
        ("dead letter", cell(&o["dead_letter"])),
        ("teams", format!("{} ({} with a live submission, {} sanctioned)", t["total"], t["with_live_submission"], t["sanctioned"])),
        ("uploads", format!("{} in the last hour", o["uploads_last_hour"])),
        ("pause", pause.join(", ")),
        ("limits", format!("{} uploads/team/h, {} req/token/min, {} req/IP/min, {} submissions kept, {} B max module",
                            l["uploads_per_hour"], l["api_per_minute"], l["public_per_minute"], l["submissions_kept"], l["max_wasm_bytes"])),
    ];
    for (k, v) in rows {
        println!("{k:<12} {v}");
    }
}

fn print_teams(teams: &Value, sanctions: &Value) {
    let status = |id: &Value| -> String {
        sanctions["sanctions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["team_id"] == *id && s["active"] == true)
            .map(|s| match s["until"].as_str() {
                Some(u) => format!("{} until {u}: {}", cell(&s["kind"]), cell(&s["reason"])),
                None => format!("{}: {}", cell(&s["kind"]), cell(&s["reason"])),
            })
            .unwrap_or_else(|| "ok".into())
    };
    let rows: Vec<Vec<String>> = teams["teams"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let live = t["live_sha256"]
                .as_str()
                .map(|s| short(s, 13))
                .unwrap_or_else(|| "-".into());
            vec![
                cell(&t["name"]),
                cell(&t["team_id"]),
                live,
                status(&t["team_id"]),
            ]
        })
        .collect();
    table(&["NAME", "ID", "LIVE", "STATUS"], &rows);
}

fn rows_of(v: &Value, key: &str, f: impl Fn(&Value) -> Vec<String>) -> Vec<Vec<String>> {
    v[key].as_array().into_iter().flatten().map(f).collect()
}

fn print(command: &AdminCommand, out: &Value, extra: Option<&Value>) {
    match command {
        AdminCommand::Overview => print_overview(out),
        AdminCommand::Flags => table(
            &["KIND", "TEAM/WORKER", "FIRST SEEN", "DETAIL"],
            &rows_of(out, "flags", |f| {
                let who = f["team_name"]
                    .as_str()
                    .or(f["worker"].as_str())
                    .unwrap_or("-");
                vec![
                    cell(&f["kind"]),
                    who.to_string(),
                    cell(&f["first_seen"]),
                    cell(&f["detail"]),
                ]
            }),
        ),
        AdminCommand::Audit { .. } => table(
            &["SEQ", "AT", "ACTOR", "STATUS", "ACTION", "PAYLOAD"],
            &rows_of(out, "entries", |e| {
                vec![
                    cell(&e["seq"]),
                    cell(&e["at"]),
                    cell(&e["actor"]),
                    cell(&e["status"]),
                    cell(&e["action"]),
                    short(&cell(&e["payload"]), 60),
                ]
            }),
        ),
        AdminCommand::Teams => print_teams(out, extra.unwrap_or(&Value::Null)),
        AdminCommand::Workers => table(
            &[
                "ID",
                "NAME",
                "CORES",
                "LIVE",
                "TRUSTED",
                "QUARANTINED",
                "LEASED",
                "LAST SEEN",
            ],
            &rows_of(out, "workers", |w| {
                let q = match w["quarantined"].as_bool() {
                    Some(true) => format!("yes: {}", cell(&w["quarantine_reason"])),
                    _ => "no".into(),
                };
                vec![
                    cell(&w["worker_id"]),
                    cell(&w["name"]),
                    cell(&w["cores"]),
                    cell(&w["live"]),
                    cell(&w["trusted"]),
                    q,
                    cell(&w["leased_jobs"]),
                    cell(&w["last_seen"]),
                ]
            }),
        ),
        AdminCommand::Dead => table(
            &["JOB", "CLASS", "ATTEMPTS", "FINISHED", "ERROR"],
            &rows_of(out, "jobs", |j| {
                vec![
                    cell(&j["job_id"]),
                    cell(&j["class"]),
                    cell(&j["attempts"]),
                    cell(&j["finished_at"]),
                    short(&cell(&j["error"]), 80),
                ]
            }),
        ),
        AdminCommand::Suspend { .. } | AdminCommand::Ban { .. } => println!(
            "{} {} ({}){}: {}",
            cell(&out["kind"]),
            cell(&out["name"]),
            cell(&out["team_id"]),
            out["until"]
                .as_str()
                .map(|u| format!(" until {u}"))
                .unwrap_or_default(),
            cell(&out["reason"])
        ),
        AdminCommand::Unban { .. } => match out["lifted"].as_bool() {
            Some(true) => println!("lifted the sanction of {}", cell(&out["name"])),
            _ => println!("{} had no sanction", cell(&out["name"])),
        },
        AdminCommand::Requeue { job } => println!("requeued {job}"),
    }
}

/// Percent-encode a path segment (team names and job ids are plain, but say what we mean).
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn run(args: AdminArgs) -> Result<()> {
    let c = Client::new(&args)?;
    let (out, extra) = match &args.command {
        AdminCommand::Overview => (c.call("GET", "/v1/admin/overview", None)?, None),
        AdminCommand::Flags => (c.call("GET", "/v1/admin/flags", None)?, None),
        AdminCommand::Audit { limit, before } => {
            let mut p = format!("/v1/admin/audit?limit={limit}");
            if let Some(b) = before {
                p.push_str(&format!("&before={b}"));
            }
            (c.call("GET", &p, None)?, None)
        }
        AdminCommand::Teams => {
            let sanctions = c.call("GET", "/v1/admin/sanctions", None)?;
            (c.call("GET", "/v1/admin/teams", None)?, Some(sanctions))
        }
        AdminCommand::Suspend {
            team,
            reason,
            until,
        } => {
            let body = json!({"reason": reason, "until": until});
            (
                c.call(
                    "POST",
                    &format!("/v1/admin/teams/{}/suspend", enc(team)),
                    Some(body),
                )?,
                None,
            )
        }
        AdminCommand::Ban { team, reason } => {
            let body = json!({"reason": reason});
            (
                c.call(
                    "POST",
                    &format!("/v1/admin/teams/{}/ban", enc(team)),
                    Some(body),
                )?,
                None,
            )
        }
        AdminCommand::Unban { team, reason } => {
            let body = json!({"reason": reason});
            (
                c.call(
                    "POST",
                    &format!("/v1/admin/teams/{}/unban", enc(team)),
                    Some(body),
                )?,
                None,
            )
        }
        AdminCommand::Workers => (c.call("GET", "/v1/admin/workers", None)?, None),
        AdminCommand::Requeue { job } => (
            c.call(
                "POST",
                &format!("/v1/admin/jobs/{}/requeue", enc(job)),
                Some(json!({})),
            )?,
            None,
        ),
        AdminCommand::Dead => (c.call("GET", "/v1/admin/dead", None)?, None),
    };
    if args.json {
        let mut out = out;
        if let Some(extra) = extra.clone() {
            out = json!({"teams": out["teams"], "sanctions": extra["sanctions"]});
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print(&args.command, &out, extra.as_ref());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_cell() {
        assert_eq!(short("abcdef", 4), "abc…");
        assert_eq!(short("abc", 4), "abc");
        assert_eq!(cell(&Value::Null), "-");
        assert_eq!(cell(&json!("x")), "x");
        assert_eq!(cell(&json!(3)), "3");
    }
}
