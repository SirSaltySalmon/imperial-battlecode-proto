//! SQL of moderation (`docs/MODERATION.md`): team sanctions, the admin audit log, the upload window,
//! submission retention, the overview and the computed flags.

use super::TeamRow;
use crate::error::{ApiError, ApiResult};
use crate::model::Class;
use crate::util::{opt_rfc3339, rfc3339};
use axum::http::StatusCode;
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

// ---- sanctions ----

/// A team's sanction. `banned`: until lifted. `suspended`: until `until` (or lifted; `None` = indefinite).
#[derive(Clone, Debug)]
pub struct Sanction {
    pub kind: String,
    pub reason: String,
    pub until: Option<i64>,
    pub created_at: i64,
}

impl Sanction {
    fn active(&self, now: i64) -> bool {
        self.kind == "banned" || self.until.is_none_or(|u| u > now)
    }
}

/// The team's sanction if it is in force at `now`.
pub fn active_sanction(tx: &Transaction, team_id: &str, now: i64) -> ApiResult<Option<Sanction>> {
    let s = tx
        .query_row(
            "SELECT kind, reason, until, created_at FROM team_sanctions WHERE team_id = ?1",
            [team_id],
            |r| {
                Ok(Sanction {
                    kind: r.get(0)?,
                    reason: r.get(1)?,
                    until: r.get(2)?,
                    created_at: r.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(s.filter(|s| s.active(now)))
}

/// Ids of the teams under a sanction in force at `now`.
pub fn blocked_teams(tx: &Transaction, now: i64) -> ApiResult<BTreeSet<String>> {
    let mut st = tx.prepare_cached(
        "SELECT team_id FROM team_sanctions WHERE kind = 'banned' OR until IS NULL OR until > ?1",
    )?;
    let rows = st.query_map([now], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// 403 `team_suspended`, with the sanction in `kind`, `reason` and `until`.
pub fn sanction_error(team_name: &str, s: &Sanction) -> ApiError {
    let until = match (s.kind.as_str(), s.until) {
        ("suspended", Some(u)) => format!(" until {}", rfc3339(u)),
        _ => " until an organiser lifts it".to_string(),
    };
    let mut details = Map::new();
    details.insert("kind".into(), json!(s.kind));
    details.insert("reason".into(), json!(s.reason));
    details.insert("until".into(), json!(opt_rfc3339(s.until)));
    ApiError {
        details: Some(details),
        ..ApiError::new(
            StatusCode::FORBIDDEN,
            "team_suspended",
            format!("team {team_name:?} is {}{until}: {}", s.kind, s.reason),
        )
    }
}

/// Refuse a match (ranked, scrim or tournament series) or a write for a team under a sanction.
pub fn ensure_active(tx: &Transaction, team: &TeamRow, now: i64) -> ApiResult<()> {
    match active_sanction(tx, &team.id, now)? {
        Some(s) => Err(sanction_error(&team.name, &s)),
        None => Ok(()),
    }
}

/// Refuse a series for both teams of it. The team that asked for it (`requested_by`) sees its own sanction;
/// a sanctioned opponent is only "unavailable", so another team's moderation details stay private.
pub fn ensure_can_play(
    tx: &Transaction,
    teams: [&TeamRow; 2],
    requested_by: Option<&str>,
    now: i64,
) -> ApiResult<()> {
    for team in teams {
        let Some(s) = active_sanction(tx, &team.id, now)? else {
            continue;
        };
        if requested_by.is_some_and(|r| r != team.id) {
            return Err(ApiError::conflict(
                "opponent_unavailable",
                format!("team {:?} cannot play at the moment", team.name),
            ));
        }
        return Err(sanction_error(&team.name, &s));
    }
    Ok(())
}

/// Suspend (`until` `None` = indefinitely) or ban a team, replacing any sanction it had.
pub fn set_sanction(
    tx: &Transaction,
    team_id: &str,
    kind: &str,
    reason: &str,
    until: Option<i64>,
    now: i64,
) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO team_sanctions(team_id, kind, reason, until, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(team_id) DO UPDATE SET kind = excluded.kind, reason = excluded.reason,
           until = excluded.until, created_at = excluded.created_at",
        params![team_id, kind, reason, until, now],
    )?;
    Ok(())
}

/// Lift a team's sanction. Returns whether it had one.
pub fn lift_sanction(tx: &Transaction, team_id: &str) -> ApiResult<bool> {
    Ok(tx.execute("DELETE FROM team_sanctions WHERE team_id = ?1", [team_id])? > 0)
}

/// Every sanction on record (expired suspensions included, with `active: false`).
pub fn list_sanctions(tx: &Transaction, now: i64) -> ApiResult<Vec<Value>> {
    let mut st = tx.prepare(
        "SELECT s.team_id, t.name, s.kind, s.reason, s.until, s.created_at FROM team_sanctions s
         JOIN teams t ON t.id = s.team_id ORDER BY s.created_at DESC",
    )?;
    let rows = st.query_map([], |r| {
        let s = Sanction {
            kind: r.get(2)?,
            reason: r.get(3)?,
            until: r.get(4)?,
            created_at: r.get(5)?,
        };
        Ok(json!({"team_id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "kind": s.kind,
                  "reason": s.reason, "until": opt_rfc3339(s.until), "since": rfc3339(s.created_at),
                  "active": s.active(now)}))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- audit ----

/// Append one entry to the audit log (the table refuses updates and deletes).
pub fn audit(
    tx: &Transaction,
    actor: &str,
    action: &str,
    status: u16,
    payload: &Value,
    now: i64,
) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO admin_audit(actor, action, status, payload, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![actor, action, status, payload.to_string(), now],
    )?;
    Ok(())
}

/// Audit entries, newest first, with `seq` below `before` when given.
pub fn audit_log(tx: &Transaction, before: Option<i64>, limit: i64) -> ApiResult<Vec<Value>> {
    let mut st = tx.prepare(
        "SELECT seq, actor, action, status, payload, created_at FROM admin_audit
         WHERE ?1 IS NULL OR seq < ?1 ORDER BY seq DESC LIMIT ?2",
    )?;
    let rows = st.query_map(params![before, limit], |r| {
        let payload: Option<String> = r.get(4)?;
        let payload = payload
            .and_then(|p| serde_json::from_str::<Value>(&p).ok())
            .unwrap_or(Value::Null);
        Ok(json!({"seq": r.get::<_, i64>(0)?, "actor": r.get::<_, String>(1)?, "action": r.get::<_, String>(2)?,
                  "status": r.get::<_, i64>(3)?, "payload": payload, "at": rfc3339(r.get(5)?)}))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- uploads and retention ----

/// The upload limit: at most `per_hour` submissions (rejected ones included) per rolling hour. Counted from
/// the submissions table, so it survives restarts. `per_hour <= 0` never limits.
pub fn check_upload(tx: &Transaction, team_id: &str, per_hour: i64, now: i64) -> ApiResult<()> {
    if per_hour <= 0 {
        return Ok(());
    }
    let since = now - 3_600_000;
    let mut st = tx.prepare_cached(
        "SELECT created_at FROM submissions WHERE team_id = ?1 AND created_at > ?2
         ORDER BY created_at DESC LIMIT 1 OFFSET ?3",
    )?;
    // The upload that has to leave the window before another one fits.
    let blocking: Option<i64> = st
        .query_row(params![team_id, since, per_hour - 1], |r| r.get(0))
        .optional()?;
    match blocking {
        None => Ok(()),
        Some(at) => Err(ApiError::rate_limited(
            "rate_limited",
            format!("upload limit: {per_hour} uploads per team per hour; build and test locally with `ibc run` between uploads"),
            ((at + 3_600_000 - now) as u64).div_ceil(1000),
        )),
    }
}

/// Tables and columns that pin a submission against retention. A unit adding a table that refers to
/// submissions (tournament entries, ...) adds it here; tables that do not exist are skipped.
const PINS: &[(&str, &str)] = &[("series", "sub_a"), ("series", "sub_b")];

fn table_exists(tx: &Transaction, table: &str) -> ApiResult<bool> {
    Ok(tx.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// Delete the team's submissions beyond the newest `kept`, except live ones, ones a series (or another
/// pinning table) used, and ones from the last hour (the upload limit counts them). Returns the hashes of
/// validated modules nothing refers to any more, whose blobs can be deleted. A rejected upload never stored
/// a blob, so its hash (which could be any blob's, a replay's say) is never returned.
pub fn prune_submissions(
    tx: &Transaction,
    team_id: &str,
    kept: i64,
    now: i64,
) -> ApiResult<Vec<String>> {
    if kept <= 0 {
        return Ok(Vec::new());
    }
    let old: Vec<(String, String, bool)> = {
        let mut st = tx.prepare(
            "SELECT id, sha256, status = 'validated' FROM submissions WHERE team_id = ?1 AND live = 0
             AND created_at <= ?3
             AND id NOT IN (SELECT id FROM submissions WHERE team_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2)",
        )?;
        let rows = st.query_map(params![team_id, kept, now - 3_600_000], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    if old.is_empty() {
        return Ok(Vec::new());
    }
    let mut pinned = BTreeSet::new();
    for (table, column) in PINS {
        if !table_exists(tx, table)? {
            continue;
        }
        let mut st = tx.prepare(&format!(
            "SELECT 1 FROM {table} WHERE {column} = ?1 LIMIT 1"
        ))?;
        for (id, _, _) in &old {
            if st.exists([id])? {
                pinned.insert(id.clone());
            }
        }
    }
    let mut hashes = BTreeSet::new();
    for (id, sha, validated) in old.iter().filter(|(id, _, _)| !pinned.contains(id)) {
        tx.execute("DELETE FROM submissions WHERE id = ?1", [id])?;
        if *validated {
            hashes.insert(sha.clone());
        }
    }
    let mut orphans = Vec::new();
    for sha in hashes {
        // Still a submission (of any team), a map, a runner binary, or the bot of a job that may run again.
        let used: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM submissions WHERE sha256 = ?1)
                 OR EXISTS(SELECT 1 FROM maps WHERE sha256 = ?1)
                 OR EXISTS(SELECT 1 FROM patches WHERE instr(runner_blobs, ?1) > 0)
                 OR EXISTS(SELECT 1 FROM jobs WHERE state IN ('queued', 'leased', 'dead') AND instr(payload, ?1) > 0)",
            [&sha],
            |r| r.get(0),
        )?;
        if !used {
            orphans.push(sha);
        }
    }
    Ok(orphans)
}

// ---- overview ----

fn count(tx: &Transaction, sql: &str) -> ApiResult<i64> {
    Ok(tx.query_row(sql, [], |r| r.get(0))?)
}

fn count_since(tx: &Transaction, sql: &str, since: i64) -> ApiResult<i64> {
    Ok(tx.query_row(sql, [since], |r| r.get(0))?)
}

/// Loads at a glance: queue by class and state, workers, throughput, cost per game, dead letters, switches.
pub fn overview(tx: &Transaction, now: i64, worker_live_ms: i64) -> ApiResult<Value> {
    let mut queue = Vec::new();
    {
        let mut st = tx.prepare(
            "SELECT class, state, COUNT(*), MIN(queued_at) FROM jobs WHERE state IN ('queued', 'leased')
             GROUP BY class, state ORDER BY class, state",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (class, state, jobs, oldest) = row?;
            queue.push(
                json!({"class": Class::from_i64(class).name(), "state": state, "jobs": jobs,
                              "oldest_age_secs": (now - oldest).max(0) / 1000}),
            );
        }
    }
    let (live, cores): (i64, i64) = tx.query_row(
        "SELECT COUNT(*), COALESCE(SUM(cores), 0) FROM workers WHERE last_seen >= ?1 AND quarantined = 0",
        [now - worker_live_ms],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let done_since = "SELECT COUNT(*) FROM jobs WHERE state = 'done' AND finished_at > ?1";
    let hour = super::league::mean_game_seconds(tx, now - 3_600_000)?;
    let mean = match hour {
        Some(m) => Some(m),
        None => super::league::mean_game_seconds(tx, now - 86_400_000)?,
    };
    let mut pause = Map::new();
    {
        let mut st = tx.prepare("SELECT key, value FROM settings WHERE key LIKE '%pause%'")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (k, v) = row?;
            pause.insert(k, json!(v));
        }
    }
    let blocked = blocked_teams(tx, now)?.len();
    Ok(json!({
        "queue": queue,
        "workers": {
            "live": live, "live_cores": cores,
            "quarantined": count(tx, "SELECT COUNT(*) FROM workers WHERE quarantined = 1")?,
            "total": count(tx, "SELECT COUNT(*) FROM workers")?,
        },
        "games": {
            "last_hour": count_since(tx, done_since, now - 3_600_000)?,
            "last_day": count_since(tx, done_since, now - 86_400_000)?,
        },
        // One match runs on one core (`ibc run-batch --jobs 1` per slot), so wall seconds are core-seconds.
        "mean_core_seconds_per_game": mean,
        "dead_letter": count(tx, "SELECT COUNT(*) FROM jobs WHERE state = 'dead'")?,
        "teams": {
            "total": count(tx, "SELECT COUNT(*) FROM teams")?,
            "with_live_submission": count(tx, "SELECT COUNT(*) FROM submissions WHERE live = 1")?,
            "sanctioned": blocked,
        },
        "uploads_last_hour": count_since(tx, "SELECT COUNT(*) FROM submissions WHERE created_at > ?1", now - 3_600_000)?,
        "pause": pause,
    }))
}

// ---- flags ----

/// One computed flag. `subject` identifies it across calls so `first_seen` can be remembered.
struct Flag {
    kind: &'static str,
    subject: String,
    team: Option<String>,
    worker: Option<String>,
    detail: String,
    /// When the data says the condition started, if it says.
    since: Option<i64>,
}

fn team_flag(
    kind: &'static str,
    team: &str,
    key: &str,
    detail: String,
    since: Option<i64>,
) -> Flag {
    Flag {
        kind,
        subject: format!("{team}:{key}"),
        team: Some(team.to_string()),
        worker: None,
        detail,
        since,
    }
}

fn upload_bursts(tx: &Transaction, over: i64, now: i64) -> ApiResult<Vec<Flag>> {
    let mut st = tx.prepare(
        "SELECT team_id, COUNT(*), MIN(created_at) FROM submissions WHERE created_at > ?1
         GROUP BY team_id HAVING COUNT(*) > ?2 ORDER BY team_id",
    )?;
    let rows = st.query_map(params![now - 3_600_000, over], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (team, n, first) = row?;
        let detail = format!("{n} uploads in the last hour (flagged above {over})");
        out.push(team_flag("upload_burst", &team, "", detail, Some(first)));
    }
    Ok(out)
}

/// The same validated module uploaded by more than one team.
fn duplicate_modules(tx: &Transaction, names: &BTreeMap<String, String>) -> ApiResult<Vec<Flag>> {
    let mut st = tx.prepare(
        "SELECT sha256, team_id, MIN(created_at) FROM submissions WHERE status = 'validated' AND sha256 IN
           (SELECT sha256 FROM submissions WHERE status = 'validated' GROUP BY sha256 HAVING COUNT(DISTINCT team_id) > 1)
         GROUP BY sha256, team_id ORDER BY sha256, MIN(created_at)",
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut by_sha: BTreeMap<String, Vec<(String, i64)>> = BTreeMap::new();
    for row in rows {
        let (sha, team, at) = row?;
        by_sha.entry(sha).or_default().push((team, at));
    }
    let mut out = Vec::new();
    for (sha, teams) in by_sha {
        // Rows are in upload order: the duplicate appeared when the second team uploaded it.
        let since = teams.get(1).map(|t| t.1);
        for (team, _) in &teams {
            let others: Vec<&str> = teams
                .iter()
                .map(|t| t.0.as_str())
                .filter(|t| t != team)
                .map(|t| names.get(t).map_or(t, String::as_str))
                .collect();
            let detail = format!(
                "module {} also uploaded by {}",
                &sha[..12.min(sha.len())],
                others.join(", ")
            );
            out.push(team_flag("duplicate_wasm", team, &sha, detail, since));
        }
    }
    Ok(out)
}

/// Teams whose newest upload was rejected (they keep playing an older module, or none), and teams whose
/// live submission has dead-lettered games in the last day.
fn failing_submissions(tx: &Transaction, now: i64) -> ApiResult<Vec<Flag>> {
    let mut out = Vec::new();
    let mut st = tx.prepare(
        "SELECT s.team_id, s.id, s.reason, s.created_at FROM submissions s
         WHERE s.status = 'rejected' AND s.rowid = (SELECT MAX(rowid) FROM submissions WHERE team_id = s.team_id)
         ORDER BY s.team_id",
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (team, id, reason, at) = row?;
        let detail = format!(
            "newest upload {id} was rejected: {}",
            reason.unwrap_or_default()
        );
        out.push(team_flag("rejected_upload", &team, &id, detail, Some(at)));
    }
    let mut st = tx.prepare(
        "SELECT x.team_id, x.id, COUNT(*), MIN(j.finished_at) FROM submissions x
         JOIN series s ON (s.team_a = x.team_id AND s.sub_a = x.id) OR (s.team_b = x.team_id AND s.sub_b = x.id)
         JOIN games g ON g.series_id = s.id JOIN jobs j ON j.id = g.job_id
         WHERE x.live = 1 AND j.state = 'dead' AND j.finished_at > ?1 GROUP BY x.team_id, x.id ORDER BY x.team_id",
    )?;
    let rows = st.query_map([now - 86_400_000], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (team, id, n, first) = row?;
        let detail = format!("{n} games of live submission {id} dead-lettered in the last 24 h");
        out.push(team_flag(
            "failing_live_submission",
            &team,
            &id,
            detail,
            Some(first),
        ));
    }
    Ok(out)
}

fn quarantined_workers(tx: &Transaction) -> ApiResult<Vec<Flag>> {
    let mut st = tx.prepare(
        "SELECT id, name, quarantine_reason FROM workers WHERE quarantined = 1 ORDER BY id",
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, reason) = row?;
        out.push(Flag {
            kind: "worker_quarantined",
            subject: id.clone(),
            team: None,
            worker: Some(id),
            detail: format!("{name}: {}", reason.unwrap_or_default()),
            since: None,
        });
    }
    Ok(out)
}

/// Every flag that holds now. `first_seen` is when the data says it started, or else when it was first
/// computed; flags that no longer hold are forgotten.
pub fn flags(tx: &Transaction, burst_over: i64, now: i64) -> ApiResult<Vec<Value>> {
    let names: BTreeMap<String, String> = {
        let mut st = tx.prepare("SELECT id, name FROM teams")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut all = upload_bursts(tx, burst_over, now)?;
    all.extend(duplicate_modules(tx, &names)?);
    all.extend(failing_submissions(tx, now)?);
    all.extend(quarantined_workers(tx)?);
    let current: BTreeSet<(String, String)> = all
        .iter()
        .map(|f| (f.kind.to_string(), f.subject.clone()))
        .collect();
    let stored: Vec<(String, String)> = {
        let mut st = tx.prepare("SELECT kind, subject FROM flags_seen")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    for (kind, subject) in stored.iter().filter(|k| !current.contains(*k)) {
        tx.execute(
            "DELETE FROM flags_seen WHERE kind = ?1 AND subject = ?2",
            params![kind, subject],
        )?;
    }
    let mut out = Vec::new();
    for f in all {
        tx.execute(
            "INSERT OR IGNORE INTO flags_seen(kind, subject, first_seen) VALUES (?1, ?2, ?3)",
            params![f.kind, f.subject, f.since.unwrap_or(now)],
        )?;
        let first_seen: i64 = tx.query_row(
            "SELECT first_seen FROM flags_seen WHERE kind = ?1 AND subject = ?2",
            params![f.kind, f.subject],
            |r| r.get(0),
        )?;
        let team_name = f.team.as_ref().and_then(|t| names.get(t));
        out.push(
            json!({"kind": f.kind, "team": f.team, "team_name": team_name, "worker": f.worker,
                        "detail": f.detail, "first_seen": rfc3339(first_seen)}),
        );
    }
    Ok(out)
}
