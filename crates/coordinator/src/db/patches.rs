//! SQL of the patch lifecycle (`crate::patch`): per-patch metadata (the ABI a patch offers bots, when it
//! stopped being current), the series a patch switch re-plans or keeps, the drain of superseded patches,
//! and the validation of submissions per patch.
//!
//! A job is pinned to the patch in its payload. A patch is *superseded* once another one became current;
//! its jobs keep running on workers that still have its runner, until `expire_drained` dead-letters what
//! is still queued after the drain window.

use crate::error::{ApiError, ApiResult};
use crate::model::Patch;
use crate::wasmcheck::Abi;
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::{BTreeMap, BTreeSet};

/// `jobs` row is on the patch `p` (a row of `patches` under that alias).
const JOB_ON_P: &str = "p.engine = json_extract(jobs.payload, '$.patch.engine')
    AND p.ruleset_hash = json_extract(jobs.payload, '$.patch.ruleset_hash')
    AND p.wasmtime = json_extract(jobs.payload, '$.patch.wasmtime')";

/// `jobs` row is not on the patch `?1, ?2, ?3`.
const JOB_OFF_PATCH: &str = "(json_extract(jobs.payload, '$.patch.engine') != ?1
    OR json_extract(jobs.payload, '$.patch.ruleset_hash') != ?2
    OR json_extract(jobs.payload, '$.patch.wasmtime') != ?3)";

pub fn current_id(tx: &Transaction) -> ApiResult<Option<i64>> {
    Ok(tx
        .query_row("SELECT id FROM patches WHERE current = 1", [], |r| r.get(0))
        .optional()?)
}

/// Record when a patch stopped being current (`None`: it is current again).
pub fn set_superseded(tx: &Transaction, patch_id: i64, at: Option<i64>) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO patch_meta(patch_id, superseded_at) VALUES (?1, ?2)
         ON CONFLICT(patch_id) DO UPDATE SET superseded_at = excluded.superseded_at",
        params![patch_id, at],
    )?;
    Ok(())
}

/// Store the ABI a patch offers bots. Returns true if it differs from what was stored.
pub fn set_abi(tx: &Transaction, patch_id: i64, abi: &Abi) -> ApiResult<bool> {
    let text = serde_json::to_string(abi).map_err(ApiError::internal)?;
    let old: Option<Option<String>> = tx
        .query_row(
            "SELECT abi FROM patch_meta WHERE patch_id = ?1",
            [patch_id],
            |r| r.get(0),
        )
        .optional()?;
    tx.execute(
        "INSERT INTO patch_meta(patch_id, abi) VALUES (?1, ?2)
         ON CONFLICT(patch_id) DO UPDATE SET abi = excluded.abi",
        params![patch_id, text],
    )?;
    let old: Option<Abi> = old.flatten().and_then(|t| serde_json::from_str(&t).ok());
    Ok(old.unwrap_or_default() != *abi)
}

/// Forget every validation on a patch: they are redone (live submissions at once, the others when made
/// live), and a failure starts its grace period again.
pub fn forget_checks(tx: &Transaction, patch_id: i64) -> ApiResult<()> {
    tx.execute(
        "DELETE FROM submission_checks WHERE patch_id = ?1",
        [patch_id],
    )?;
    Ok(())
}

/// The worker gave back a job of this patch because it does not have its runner: do not offer it jobs of
/// the patch again before `until`.
pub fn record_miss(tx: &Transaction, worker_id: &str, patch: &Patch, until: i64) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO patch_misses(worker_id, engine, ruleset_hash, wasmtime, until) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(worker_id, engine, ruleset_hash, wasmtime) DO UPDATE SET until = excluded.until",
        params![worker_id, patch.engine, patch.ruleset_hash, patch.wasmtime, until],
    )?;
    Ok(())
}

/// The current patch's id and the ABI it offers bots (this build's if it was registered without one).
pub fn current_abi(tx: &Transaction) -> ApiResult<(Option<i64>, Abi)> {
    let row: Option<(i64, Option<String>)> = tx
        .query_row(
            "SELECT p.id, m.abi FROM patches p LEFT JOIN patch_meta m ON m.patch_id = p.id WHERE p.current = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((id, abi)) = row else {
        return Ok((None, Abi::default()));
    };
    let abi = match abi {
        Some(text) => serde_json::from_str(&text).map_err(ApiError::internal)?,
        None => Abi::default(),
    };
    Ok((Some(id), abi))
}

// ---- the patch boundary ----

/// Running series to re-plan on `patch`: not a tournament, nothing played or running yet (no game whose job
/// is leased or done), and at least one queued game on another patch.
pub fn replannable_series(tx: &Transaction, patch: &Patch) -> ApiResult<Vec<String>> {
    let mut st = tx.prepare(&format!(
        "SELECT s.id FROM series s WHERE s.state = 'running' AND s.kind != 'tournament'
           AND NOT EXISTS (SELECT 1 FROM games g JOIN jobs ON jobs.id = g.job_id
                           WHERE g.series_id = s.id AND jobs.state IN ('leased', 'done'))
           AND EXISTS (SELECT 1 FROM games g JOIN jobs ON jobs.id = g.job_id
                       WHERE g.series_id = s.id AND jobs.state = 'queued' AND {JOB_OFF_PATCH})
         ORDER BY s.created_at, s.id"
    ))?;
    let rows = st.query_map(
        params![patch.engine, patch.ruleset_hash, patch.wasmtime],
        |r| r.get(0),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The distinct queued jobs of a series that are not on `patch`, in game order.
pub fn queued_off_patch(
    tx: &Transaction,
    series_id: &str,
    patch: &Patch,
) -> ApiResult<Vec<String>> {
    let mut st = tx.prepare(&format!(
        "SELECT jobs.id FROM games g JOIN jobs ON jobs.id = g.job_id
         WHERE g.series_id = ?4 AND jobs.state = 'queued' AND {JOB_OFF_PATCH}
         GROUP BY jobs.id ORDER BY MIN(g.idx)"
    ))?;
    let rows = st.query_map(
        params![patch.engine, patch.ruleset_hash, patch.wasmtime, series_id],
        |r| r.get(0),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Running series that still have a queued or leased game on another patch than `patch`.
pub fn series_off_patch(tx: &Transaction, patch: &Patch) -> ApiResult<i64> {
    Ok(tx.query_row(
        &format!(
            "SELECT COUNT(DISTINCT g.series_id) FROM games g JOIN jobs ON jobs.id = g.job_id
             JOIN series s ON s.id = g.series_id
             WHERE s.state = 'running' AND jobs.state IN ('queued', 'leased') AND {JOB_OFF_PATCH}"
        ),
        params![patch.engine, patch.ruleset_hash, patch.wasmtime],
        |r| r.get(0),
    )?)
}

/// Dead-letter the queued jobs of every patch superseded at or before `cutoff`, except tournament jobs
/// (a tournament plays out on its patch). Returns how many jobs that was.
pub fn expire_drained(tx: &Transaction, cutoff: i64, now: i64) -> ApiResult<usize> {
    Ok(tx.execute(
        &format!(
            "UPDATE jobs SET state = 'dead', finished_at = ?2, lease_token = NULL, lease_expires_at = NULL,
                    error = 'patch superseded: not run within the drain window'
             WHERE state = 'queued' AND class != 0
               AND EXISTS (SELECT 1 FROM patches p JOIN patch_meta m ON m.patch_id = p.id
                           WHERE p.current = 0 AND m.superseded_at <= ?1 AND {JOB_ON_P})"
        ),
        params![cutoff, now],
    )?)
}

/// Queued and leased jobs of superseded patches: what is still draining.
pub fn draining_jobs(tx: &Transaction) -> ApiResult<i64> {
    Ok(tx.query_row(
        &format!(
            "SELECT COUNT(*) FROM jobs WHERE state IN ('queued', 'leased')
               AND EXISTS (SELECT 1 FROM patches p JOIN patch_meta m ON m.patch_id = p.id
                           WHERE p.current = 0 AND m.superseded_at IS NOT NULL AND {JOB_ON_P})"
        ),
        [],
        |r| r.get(0),
    )?)
}

// ---- submission validation per patch ----

/// A submission's validation on one patch.
#[derive(Clone, Debug)]
pub struct Check {
    /// `None`: valid. `Some(reason)`: invalid.
    pub failure: Option<String>,
    pub checked_at: i64,
}

/// The team id, submission id and module hash of every live submission.
pub fn live_submissions(tx: &Transaction) -> ApiResult<Vec<(String, String, String)>> {
    let mut st =
        tx.prepare("SELECT team_id, id, sha256 FROM submissions WHERE live = 1 ORDER BY team_id")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The module hash of a validated submission that is not live (of `team_id`, when given): one that making
/// live would change something.
pub fn validated_module(
    tx: &Transaction,
    submission_id: &str,
    team_id: Option<&str>,
) -> ApiResult<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT sha256 FROM submissions WHERE id = ?1 AND status = 'validated' AND live = 0
               AND (?2 IS NULL OR team_id = ?2)",
            params![submission_id, team_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Store a validation result. A submission that keeps failing keeps the time it first failed (when its
/// grace period started); any change of verdict restarts the clock.
pub fn record_check(
    tx: &Transaction,
    submission_id: &str,
    patch_id: i64,
    failure: Option<&str>,
    now: i64,
) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO submission_checks(submission_id, patch_id, ok, reason, checked_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(submission_id, patch_id) DO UPDATE SET reason = excluded.reason,
           checked_at = CASE WHEN ok = excluded.ok THEN checked_at ELSE excluded.checked_at END,
           ok = excluded.ok",
        params![submission_id, patch_id, failure.is_none(), failure, now],
    )?;
    Ok(())
}

fn check_row(r: &rusqlite::Row, at: usize) -> rusqlite::Result<Check> {
    let ok: bool = r.get(at)?;
    let reason: Option<String> = r.get(at + 1)?;
    Ok(Check {
        failure: if ok {
            None
        } else {
            Some(reason.unwrap_or_default())
        },
        checked_at: r.get(at + 2)?,
    })
}

/// A submission's validation on the current patch, if it was checked on it.
pub fn current_check(tx: &Transaction, submission_id: &str) -> ApiResult<Option<Check>> {
    Ok(tx
        .query_row(
            "SELECT c.ok, c.reason, c.checked_at FROM submission_checks c
             JOIN patches p ON p.id = c.patch_id AND p.current = 1 WHERE c.submission_id = ?1",
            [submission_id],
            |r| check_row(r, 0),
        )
        .optional()?)
}

/// Validation on the current patch of every checked submission of a team, by submission id.
pub fn team_checks(tx: &Transaction, team_id: &str) -> ApiResult<BTreeMap<String, Check>> {
    let mut st = tx.prepare(
        "SELECT s.id, c.ok, c.reason, c.checked_at FROM submissions s
         JOIN submission_checks c ON c.submission_id = s.id
         JOIN patches p ON p.id = c.patch_id AND p.current = 1 WHERE s.team_id = ?1",
    )?;
    let rows = st.query_map([team_id], |r| Ok((r.get(0)?, check_row(r, 1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Teams whose live submission fails on the current patch: `(team_id, failed since)`.
pub fn failing_teams(tx: &Transaction) -> ApiResult<Vec<(String, i64)>> {
    let mut st = tx.prepare(
        "SELECT s.team_id, c.checked_at FROM submissions s
         JOIN submission_checks c ON c.submission_id = s.id
         JOIN patches p ON p.id = c.patch_id AND p.current = 1
         WHERE s.live = 1 AND c.ok = 0 ORDER BY s.team_id",
    )?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Teams whose live submission has failed on the current patch since `cutoff` or earlier: past their grace
/// period, they are left out of ranked pairing.
pub fn excluded_teams(tx: &Transaction, cutoff: i64) -> ApiResult<BTreeSet<String>> {
    Ok(failing_teams(tx)?
        .into_iter()
        .filter(|(_, since)| *since <= cutoff)
        .map(|(team, _)| team)
        .collect())
}
