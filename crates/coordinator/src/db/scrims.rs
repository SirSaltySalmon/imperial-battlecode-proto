//! What a scrim request resolves to: the requester's own archived submission, the opponent's bot from a
//! series both teams played, pool maps by name, and the unranked quota.

use super::SubmissionRow;
use crate::error::{ApiError, ApiResult};
use rusqlite::{params, OptionalExtension, Transaction};

/// The rolling window of the unranked quota.
pub const QUOTA_WINDOW_MS: i64 = 86_400_000;

fn submission(tx: &Transaction, id: &str) -> ApiResult<Option<SubmissionRow>> {
    Ok(tx
        .query_row(
            "SELECT id, sha256, language FROM submissions WHERE id = ?1",
            [id],
            |r| {
                Ok(SubmissionRow {
                    id: r.get(0)?,
                    sha256: r.get(1)?,
                    language: r.get(2)?,
                })
            },
        )
        .optional()?)
}

/// One of the team's own validated submissions, live or archived.
pub fn own_submission(tx: &Transaction, team_id: &str, id: &str) -> ApiResult<SubmissionRow> {
    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM submissions WHERE id = ?1 AND team_id = ?2",
            params![id, team_id],
            |r| r.get(0),
        )
        .optional()?;
    match status.as_deref() {
        None => Err(ApiError::not_found(format!(
            "no submission {id:?} for this team"
        ))),
        Some("validated") => {
            submission(tx, id)?.ok_or_else(|| ApiError::internal("submission vanished"))
        }
        Some(_) => Err(ApiError::conflict(
            "not_validated",
            format!("submission {id:?} was rejected and cannot play"),
        )),
    }
}

/// The submission `opponent` played in series `series_id` against `me`. The id stays on the server: it is
/// only ever used to pin the new series. Any other series (unknown, or not between these two teams) is a
/// 404 that does not say which.
pub fn past_opponent_submission(
    tx: &Transaction,
    series_id: &str,
    me: &str,
    opponent: &str,
) -> ApiResult<SubmissionRow> {
    let pinned: Option<String> = tx
        .query_row(
            "SELECT CASE WHEN team_a = ?2 THEN sub_b ELSE sub_a END FROM series
             WHERE id = ?1 AND ((team_a = ?2 AND team_b = ?3) OR (team_a = ?3 AND team_b = ?2))",
            params![series_id, me, opponent],
            |r| r.get(0),
        )
        .optional()?;
    let not_found = || {
        ApiError::not_found(format!(
            "no series {series_id:?} between your team and the opponent"
        ))
    };
    let pinned = pinned.ok_or_else(not_found)?;
    submission(tx, &pinned)?.ok_or_else(not_found)
}

/// A map of the pool by name, as `(name, sha256)`. Maps outside the pool (retired, or held back for a
/// tournament) are not served and cannot be played in a scrim.
pub fn pool_map(tx: &Transaction, name: &str) -> ApiResult<(String, String)> {
    let map: Option<(String, String)> = tx
        .query_row(
            "SELECT name, sha256 FROM maps WHERE name = ?1 AND in_pool = 1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match map {
        Some(map) if super::league::map_is_public(tx, name)? => Ok(map),
        _ => Err(ApiError::not_found(format!("no map {name:?} in the pool"))),
    }
}

/// Check that `games` more unranked games fit in the team's quota (`limit` per rolling 24 h). `Err` is a
/// 429 `quota_exceeded` whose `retry_after` is when enough of the team's earlier requests leave the window.
pub fn check_quota(
    tx: &Transaction,
    team_id: &str,
    limit: i64,
    games: i64,
    now: i64,
) -> ApiResult<()> {
    let requests: Vec<(i64, i64)> = {
        let mut st = tx.prepare_cached(
            "SELECT created_at, games FROM series WHERE requested_by = ?1 AND created_at > ?2
             ORDER BY created_at",
        )?;
        let rows = st.query_map(params![team_id, now - QUOTA_WINDOW_MS], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let used: i64 = requests.iter().map(|(_, g)| g).sum();
    if used + games <= limit {
        return Ok(());
    }
    let message =
        format!("{used} of {limit} unranked games used in the last 24 h; {games} more do not fit");
    // The oldest requests leave the window first; wait for the one after which the new request fits.
    let mut left = used;
    for (created_at, g) in &requests {
        left -= g;
        if left + games <= limit {
            let wait_ms = created_at + QUOTA_WINDOW_MS - now;
            return Err(ApiError::rate_limited(
                "quota_exceeded",
                message,
                (wait_ms.max(0) as u64).div_ceil(1000),
            ));
        }
    }
    Err(ApiError::bad_request(format!(
        "{games} games are more than the daily quota of {limit}"
    )))
}
