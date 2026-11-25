//! All SQL of the coordinator lives in this module tree (`db`, `db::jobs`, `db::league`, `db::ratings`),
//! behind plain functions that take a transaction. One SQLite connection in WAL mode, serialised by a mutex:
//! every operation is a short transaction, so "lease" can never hand the same job to two workers.
//! Moving to Postgres means rewriting these four files and nothing else.

pub mod jobs;
pub mod league;
pub mod patches;
pub mod ratings;

use crate::error::{ApiError, ApiResult};
use crate::model::Patch;
use crate::util::{opt_rfc3339, random_hex, rfc3339};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Mutex;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS teams(
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, token_hash TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS submissions(
  id TEXT PRIMARY KEY, team_id TEXT NOT NULL REFERENCES teams(id), sha256 TEXT NOT NULL, size INTEGER NOT NULL,
  language TEXT NOT NULL, fuel_mult REAL NOT NULL, status TEXT NOT NULL, reason TEXT,
  live INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
CREATE UNIQUE INDEX IF NOT EXISTS submissions_live ON submissions(team_id) WHERE live = 1;
CREATE INDEX IF NOT EXISTS submissions_team ON submissions(team_id, created_at);
CREATE TABLE IF NOT EXISTS maps(
  name TEXT PRIMARY KEY, sha256 TEXT NOT NULL, in_pool INTEGER NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS patches(
  id INTEGER PRIMARY KEY, engine TEXT NOT NULL, ruleset_hash TEXT NOT NULL, wasmtime TEXT NOT NULL,
  current INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, runner_blobs TEXT,
  UNIQUE(engine, ruleset_hash, wasmtime));
CREATE TABLE IF NOT EXISTS workers(
  id TEXT PRIMARY KEY, name TEXT NOT NULL, labels TEXT NOT NULL, cores INTEGER NOT NULL,
  token_hash TEXT NOT NULL UNIQUE, trusted INTEGER NOT NULL DEFAULT 0, quarantined INTEGER NOT NULL DEFAULT 0,
  quarantine_reason TEXT, created_at INTEGER NOT NULL, last_seen INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS workers_seen ON workers(last_seen);
CREATE TABLE IF NOT EXISTS jobs(
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, class INTEGER NOT NULL, key TEXT NOT NULL UNIQUE,
  payload TEXT NOT NULL, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
  lease_token TEXT, lease_expires_at INTEGER, worker_id TEXT,
  require_label TEXT, verifies TEXT, avoid_worker TEXT,
  result TEXT, replay_sha256 TEXT, verified INTEGER NOT NULL DEFAULT 0, error TEXT,
  timeouts INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL, queued_at INTEGER NOT NULL, finished_at INTEGER);
CREATE INDEX IF NOT EXISTS jobs_queue ON jobs(state, class, created_at);
CREATE INDEX IF NOT EXISTS jobs_worker ON jobs(worker_id, state);
CREATE INDEX IF NOT EXISTS jobs_verifies ON jobs(verifies);
CREATE INDEX IF NOT EXISTS jobs_finished ON jobs(finished_at);
CREATE TABLE IF NOT EXISTS series(
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, team_a TEXT NOT NULL, team_b TEXT NOT NULL,
  sub_a TEXT NOT NULL, sub_b TEXT NOT NULL, games INTEGER NOT NULL, state TEXT NOT NULL,
  rated INTEGER NOT NULL DEFAULT 0, wins_a INTEGER NOT NULL DEFAULT 0, wins_b INTEGER NOT NULL DEFAULT 0,
  draws INTEGER NOT NULL DEFAULT 0, note TEXT, requested_by TEXT,
  created_at INTEGER NOT NULL, finished_at INTEGER);
CREATE INDEX IF NOT EXISTS series_state ON series(state);
CREATE INDEX IF NOT EXISTS series_requested ON series(requested_by, created_at);
CREATE TABLE IF NOT EXISTS games(
  id TEXT PRIMARY KEY, series_id TEXT NOT NULL REFERENCES series(id), idx INTEGER NOT NULL,
  job_id TEXT NOT NULL, map_name TEXT NOT NULL, seed TEXT NOT NULL, swapped INTEGER NOT NULL,
  created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS games_job ON games(job_id);
CREATE INDEX IF NOT EXISTS games_series ON games(series_id, idx);
CREATE INDEX IF NOT EXISTS games_created ON games(created_at);
CREATE TABLE IF NOT EXISTS map_usage(name TEXT PRIMARY KEY, games INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ratings(
  team_id TEXT PRIMARY KEY, mu REAL NOT NULL, sigma REAL NOT NULL, games INTEGER NOT NULL,
  wins INTEGER NOT NULL, losses INTEGER NOT NULL, draws INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  last_played INTEGER);
CREATE TABLE IF NOT EXISTS rating_events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, team_a TEXT NOT NULL, team_b TEXT,
  series_id TEXT, game_idx INTEGER, outcome TEXT, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS rating_events_series ON rating_events(series_id);
CREATE INDEX IF NOT EXISTS rating_events_created ON rating_events(created_at);
CREATE TABLE IF NOT EXISTS ranked_batches(
  id INTEGER PRIMARY KEY AUTOINCREMENT, seed TEXT NOT NULL, budget INTEGER NOT NULL, created INTEGER NOT NULL,
  created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS ladder_snapshots(
  id INTEGER PRIMARY KEY AUTOINCREMENT, created_at INTEGER NOT NULL, body TEXT NOT NULL);
-- patch lifecycle (db/patches.rs)
CREATE TABLE IF NOT EXISTS patch_meta(
  patch_id INTEGER PRIMARY KEY REFERENCES patches(id), abi TEXT, superseded_at INTEGER);
CREATE TABLE IF NOT EXISTS submission_checks(
  submission_id TEXT NOT NULL REFERENCES submissions(id), patch_id INTEGER NOT NULL REFERENCES patches(id),
  ok INTEGER NOT NULL, reason TEXT, checked_at INTEGER NOT NULL, PRIMARY KEY(submission_id, patch_id));
CREATE TABLE IF NOT EXISTS patch_misses(
  worker_id TEXT NOT NULL, engine TEXT NOT NULL, ruleset_hash TEXT NOT NULL, wasmtime TEXT NOT NULL,
  until INTEGER NOT NULL, PRIMARY KEY(worker_id, engine, ruleset_hash, wasmtime));
";

/// Columns added after the first release: `CREATE TABLE IF NOT EXISTS` does not add them to an existing data
/// directory, so they are added here when missing.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    ("jobs", "timeouts", "INTEGER NOT NULL DEFAULT 0"),
    ("patches", "runner_blobs", "TEXT"),
    ("ratings", "last_played", "INTEGER"),
];

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    for (table, column, decl) in ADDED_COLUMNS {
        let present: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |r| r.get(0),
        )?;
        if present == 0 {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
        }
    }
    // `map_usage` is a running count of games per map; fill it once for a data directory that predates it.
    conn.execute_batch(
        "INSERT INTO map_usage(name, games) SELECT map_name, COUNT(*) FROM games
           WHERE NOT EXISTS (SELECT 1 FROM map_usage) GROUP BY map_name",
    )
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Db> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Db::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Db> {
        Db::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Db> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    /// Run `f` in one write transaction: committed if it returns `Ok`, rolled back otherwise.
    pub fn tx<T>(&self, f: impl FnOnce(&Transaction) -> ApiResult<T>) -> ApiResult<T> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

pub fn setting(tx: &Transaction, key: &str) -> ApiResult<Option<String>> {
    Ok(tx
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

pub fn set_setting(tx: &Transaction, key: &str, value: &str) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation)
}

// ---- teams ----

#[derive(Clone, Debug)]
pub struct TeamRow {
    pub id: String,
    pub name: String,
}

pub fn create_team(tx: &Transaction, name: &str, token_hash: &str, now: i64) -> ApiResult<TeamRow> {
    let id = format!("t{}", random_hex(6));
    match tx.execute(
        "INSERT INTO teams(id, name, token_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, name, token_hash, now],
    ) {
        Ok(_) => Ok(TeamRow {
            id,
            name: name.to_string(),
        }),
        Err(e) if is_unique_violation(&e) => Err(ApiError::conflict(
            "team_exists",
            format!("a team named {name:?} already exists"),
        )),
        Err(e) => Err(e.into()),
    }
}

pub fn team_by_token(tx: &Transaction, token_hash: &str) -> ApiResult<Option<TeamRow>> {
    Ok(tx
        .query_row(
            "SELECT id, name FROM teams WHERE token_hash = ?1",
            [token_hash],
            |r| {
                Ok(TeamRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            },
        )
        .optional()?)
}

/// A team by id or by name.
pub fn find_team(tx: &Transaction, id_or_name: &str) -> ApiResult<TeamRow> {
    tx.query_row(
        "SELECT id, name FROM teams WHERE id = ?1 OR name = ?1 ORDER BY id = ?1 DESC LIMIT 1",
        [id_or_name],
        |r| {
            Ok(TeamRow {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| ApiError::not_found(format!("no team {id_or_name:?}")))
}

pub fn list_teams(tx: &Transaction) -> ApiResult<Vec<Value>> {
    let mut st = tx.prepare(
        "SELECT t.id, t.name, t.created_at, s.id, s.sha256 FROM teams t
         LEFT JOIN submissions s ON s.team_id = t.id AND s.live = 1 ORDER BY t.name",
    )?;
    let rows = st.query_map([], |r| {
        Ok(json!({"team_id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "created_at": rfc3339(r.get(2)?),
                  "live_submission": r.get::<_, Option<String>>(3)?, "live_sha256": r.get::<_, Option<String>>(4)?}))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- submissions ----

#[derive(Clone, Debug)]
pub struct SubmissionRow {
    pub id: String,
    pub sha256: String,
    pub fuel_mult: f64,
}

pub struct NewSubmission<'a> {
    pub team_id: &'a str,
    pub sha256: &'a str,
    pub size: usize,
    pub language: &'a str,
    pub fuel_mult: f64,
    /// `None`: validated. `Some(reason)`: rejected (never live).
    pub rejected: Option<&'a str>,
    /// Make a validated module the team's live submission at once (the default of the upload endpoint).
    pub make_live: bool,
}

/// What a change of live submission did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveChange {
    /// The live submission is what it was.
    Unchanged,
    /// The team had no live submission before.
    First,
    /// Another submission was live before: the team's rating uncertainty is widened.
    Replaced,
}

/// Store a submission. Returns its id and what happened to the team's live submission.
pub fn insert_submission(
    tx: &Transaction,
    s: &NewSubmission,
    now: i64,
) -> ApiResult<(String, LiveChange)> {
    let validated = s.rejected.is_none();
    let current = live_submission(tx, s.team_id)?;
    // Re-uploading the module that is already live changes nothing.
    if let Some(current) = current
        .as_ref()
        .filter(|c| validated && c.sha256 == s.sha256 && c.fuel_mult == s.fuel_mult)
    {
        return Ok((current.id.clone(), LiveChange::Unchanged));
    }
    let id = format!("sub{}", random_hex(6));
    tx.execute(
        "INSERT INTO submissions(id, team_id, sha256, size, language, fuel_mult, status, reason, live, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
        params![id, s.team_id, s.sha256, s.size as i64, s.language, s.fuel_mult, if validated { "validated" } else { "rejected" }, s.rejected, now],
    )?;
    let change = if validated && s.make_live {
        set_live(tx, s.team_id, &id)?
    } else {
        LiveChange::Unchanged
    };
    Ok((id, change))
}

/// Make `submission_id` (a validated submission of `team_id`) the team's live submission. Series that
/// already exist keep the submissions they were created with; only series created from now on use it.
pub fn set_live(tx: &Transaction, team_id: &str, submission_id: &str) -> ApiResult<LiveChange> {
    let row: Option<(String, bool)> = tx
        .query_row(
            "SELECT status, live FROM submissions WHERE id = ?1 AND team_id = ?2",
            params![submission_id, team_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((status, live)) = row else {
        return Err(ApiError::not_found(format!(
            "no submission {submission_id:?} for this team"
        )));
    };
    if status != "validated" {
        return Err(ApiError::conflict(
            "not_validated",
            format!("submission {submission_id:?} was rejected and cannot be live"),
        ));
    }
    if live {
        return Ok(LiveChange::Unchanged);
    }
    let replaced = tx.execute(
        "UPDATE submissions SET live = 0 WHERE team_id = ?1 AND live = 1",
        [team_id],
    )?;
    tx.execute(
        "UPDATE submissions SET live = 1 WHERE id = ?1",
        [submission_id],
    )?;
    Ok(if replaced > 0 {
        LiveChange::Replaced
    } else {
        LiveChange::First
    })
}

/// The team a submission belongs to.
pub fn submission_team(tx: &Transaction, submission_id: &str) -> ApiResult<TeamRow> {
    tx.query_row(
        "SELECT t.id, t.name FROM submissions s JOIN teams t ON t.id = s.team_id WHERE s.id = ?1",
        [submission_id],
        |r| {
            Ok(TeamRow {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| ApiError::not_found(format!("no submission {submission_id:?}")))
}

pub fn live_submission(tx: &Transaction, team_id: &str) -> ApiResult<Option<SubmissionRow>> {
    Ok(tx
        .query_row(
            "SELECT id, sha256, fuel_mult FROM submissions WHERE team_id = ?1 AND live = 1",
            [team_id],
            |r| {
                Ok(SubmissionRow {
                    id: r.get(0)?,
                    sha256: r.get(1)?,
                    fuel_mult: r.get(2)?,
                })
            },
        )
        .optional()?)
}

pub fn list_submissions(tx: &Transaction, team_id: &str) -> ApiResult<Vec<Value>> {
    let mut st = tx.prepare(
        "SELECT id, sha256, size, language, fuel_mult, status, reason, live, created_at FROM submissions
         WHERE team_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 100",
    )?;
    let rows = st.query_map([team_id], |r| {
        Ok(json!({"submission_id": r.get::<_, String>(0)?, "sha256": r.get::<_, String>(1)?, "size": r.get::<_, i64>(2)?,
                  "language": r.get::<_, String>(3)?, "fuel_mult": r.get::<_, f64>(4)?, "status": r.get::<_, String>(5)?,
                  "reason": r.get::<_, Option<String>>(6)?, "live": r.get::<_, bool>(7)?, "created_at": rfc3339(r.get(8)?)}))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- maps ----

pub fn upsert_map(
    tx: &Transaction,
    name: &str,
    sha256: &str,
    in_pool: bool,
    now: i64,
) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO maps(name, sha256, in_pool, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET sha256 = excluded.sha256, in_pool = excluded.in_pool",
        params![name, sha256, in_pool, now],
    )?;
    Ok(())
}

/// The map pool as `(name, sha256)`, sorted by name so rotation is stable.
pub fn pool_maps(tx: &Transaction) -> ApiResult<Vec<(String, String)>> {
    let mut st = tx.prepare("SELECT name, sha256 FROM maps WHERE in_pool = 1 ORDER BY name")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn list_maps(tx: &Transaction) -> ApiResult<Vec<Value>> {
    let mut st = tx.prepare("SELECT name, sha256, in_pool FROM maps ORDER BY name")?;
    let rows = st.query_map([], |r| {
        Ok(json!({"name": r.get::<_, String>(0)?, "sha256": r.get::<_, String>(1)?, "in_pool": r.get::<_, bool>(2)?}))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- patches ----

/// Make `patch` the current one; returns its row id. Jobs of other patches stay pinned to their patch:
/// what happens to them is `crate::patch::switch`'s decision (docs/JOBS.md, "Patch boundary").
///
/// `runner_blobs` (`"<os>-<arch>"` -> SHA-256 of an `ibc` binary of this patch), when given, replaces the
/// runner blobs stored for the patch; they are sent with its jobs (docs/JOBS.md, `runner_blobs`).
pub fn set_current_patch(
    tx: &Transaction,
    patch: &Patch,
    runner_blobs: Option<&Value>,
    now: i64,
) -> ApiResult<i64> {
    tx.execute("UPDATE patches SET current = 0 WHERE current = 1", [])?;
    let blobs = runner_blobs.map(Value::to_string);
    tx.execute(
        "INSERT INTO patches(engine, ruleset_hash, wasmtime, current, created_at, runner_blobs) VALUES (?1, ?2, ?3, 1, ?4, ?5)
         ON CONFLICT(engine, ruleset_hash, wasmtime) DO UPDATE SET current = 1,
           runner_blobs = COALESCE(excluded.runner_blobs, runner_blobs)",
        params![patch.engine, patch.ruleset_hash, patch.wasmtime, now, blobs],
    )?;
    Ok(tx.query_row("SELECT id FROM patches WHERE current = 1", [], |r| r.get(0))?)
}

/// The runner blobs of every patch that has some, keyed by patch.
pub fn runner_blobs(tx: &Transaction) -> ApiResult<Vec<(Patch, Value)>> {
    let mut st = tx.prepare_cached(
        "SELECT engine, ruleset_hash, wasmtime, runner_blobs FROM patches WHERE runner_blobs IS NOT NULL",
    )?;
    let rows = st.query_map([], |r| {
        let patch = Patch {
            engine: r.get(0)?,
            ruleset_hash: r.get(1)?,
            wasmtime: r.get(2)?,
        };
        Ok((patch, r.get::<_, String>(3)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (patch, text) = row?;
        let blobs: Value = serde_json::from_str(&text).map_err(ApiError::internal)?;
        if blobs.as_object().is_some_and(|m| !m.is_empty()) {
            out.push((patch, blobs));
        }
    }
    Ok(out)
}

pub fn current_patch(tx: &Transaction) -> ApiResult<Option<Patch>> {
    Ok(tx
        .query_row(
            "SELECT engine, ruleset_hash, wasmtime FROM patches WHERE current = 1",
            [],
            |r| {
                Ok(Patch {
                    engine: r.get(0)?,
                    ruleset_hash: r.get(1)?,
                    wasmtime: r.get(2)?,
                })
            },
        )
        .optional()?)
}

// ---- workers ----

#[derive(Clone, Debug)]
pub struct WorkerRow {
    pub id: String,
    /// Effective labels: what the worker declared, with `trusted` present exactly when the flag is set.
    pub labels: Vec<String>,
    pub trusted: bool,
    pub quarantined: bool,
}

pub const TRUSTED_LABEL: &str = "trusted";

fn effective_labels(declared: &str, trusted: bool) -> Vec<String> {
    let mut labels: Vec<String> = serde_json::from_str(declared).unwrap_or_default();
    labels.retain(|l| l != TRUSTED_LABEL);
    if trusted {
        labels.push(TRUSTED_LABEL.to_string());
    }
    labels
}

pub fn register_worker(
    tx: &Transaction,
    name: &str,
    labels: &[String],
    cores: i64,
    token_hash: &str,
    trusted: bool,
    now: i64,
) -> ApiResult<String> {
    let id = format!("w{}", random_hex(8));
    let labels = serde_json::to_string(labels).map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO workers(id, name, labels, cores, token_hash, trusted, created_at, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![id, name, labels, cores, token_hash, trusted, now],
    )?;
    Ok(id)
}

/// Authenticate a worker and record that it is alive.
pub fn worker_by_token(
    tx: &Transaction,
    token_hash: &str,
    now: i64,
) -> ApiResult<Option<WorkerRow>> {
    let row = tx
        .query_row(
            "SELECT id, labels, trusted, quarantined FROM workers WHERE token_hash = ?1",
            [token_hash],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((id, labels, trusted, quarantined)) = row else {
        return Ok(None);
    };
    tx.execute(
        "UPDATE workers SET last_seen = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    Ok(Some(WorkerRow {
        id,
        labels: effective_labels(&labels, trusted),
        trusted,
        quarantined,
    }))
}

pub fn list_workers(tx: &Transaction, now: i64, live_ms: i64) -> ApiResult<Vec<Value>> {
    let mut st = tx.prepare(
        "SELECT w.id, w.name, w.labels, w.cores, w.trusted, w.quarantined, w.quarantine_reason, w.created_at, w.last_seen,
                (SELECT COUNT(*) FROM jobs j WHERE j.worker_id = w.id AND j.state = 'leased')
         FROM workers w ORDER BY w.last_seen DESC LIMIT 1000",
    )?;
    let rows = st.query_map([], |r| {
        let trusted: bool = r.get(4)?;
        let last_seen: i64 = r.get(8)?;
        Ok(json!({"worker_id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?,
                  "labels": effective_labels(&r.get::<_, String>(2)?, trusted), "cores": r.get::<_, i64>(3)?,
                  "trusted": trusted, "quarantined": r.get::<_, bool>(5)?, "quarantine_reason": r.get::<_, Option<String>>(6)?,
                  "created_at": rfc3339(r.get(7)?), "last_seen": opt_rfc3339(Some(last_seen)),
                  "live": now - last_seen <= live_ms, "leased_jobs": r.get::<_, i64>(9)?}))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Cores of the workers heard from since `since` that are not quarantined.
pub fn live_cores(tx: &Transaction, since: i64) -> ApiResult<i64> {
    Ok(tx.query_row(
        "SELECT COALESCE(SUM(cores), 0) FROM workers WHERE last_seen >= ?1 AND quarantined = 0",
        [since],
        |r| r.get(0),
    )?)
}

fn worker_exists(tx: &Transaction, id: &str) -> ApiResult<()> {
    let n: i64 = tx.query_row("SELECT COUNT(*) FROM workers WHERE id = ?1", [id], |r| {
        r.get(0)
    })?;
    if n == 0 {
        return Err(ApiError::not_found(format!("no worker {id:?}")));
    }
    Ok(())
}

/// Mark a worker trusted (its future results are final; past ones stay provisional) or not.
pub fn set_worker_trusted(tx: &Transaction, id: &str, trusted: bool) -> ApiResult<()> {
    worker_exists(tx, id)?;
    tx.execute(
        "UPDATE workers SET trusted = ?2 WHERE id = ?1",
        params![id, trusted],
    )?;
    Ok(())
}

/// Lift a quarantine (setting one goes through `jobs::quarantine`, which also invalidates results).
pub fn release_worker(tx: &Transaction, id: &str) -> ApiResult<()> {
    worker_exists(tx, id)?;
    tx.execute(
        "UPDATE workers SET quarantined = 0, quarantine_reason = NULL WHERE id = ?1",
        [id],
    )?;
    Ok(())
}
