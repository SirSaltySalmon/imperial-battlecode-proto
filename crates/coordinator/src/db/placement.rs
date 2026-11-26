//! Placement: which teams are still placing their live submission, and how many placement series each
//! has started lately. A team is placing while its live submission has finished fewer ranked series than
//! `placement_series` (whoever scheduled them: the placement task or a scheduler round). A series that
//! could not be run counts too, so a submission whose games all fail is not placed forever.

use crate::error::ApiResult;
use rusqlite::{params, Transaction};
use std::collections::{BTreeMap, BTreeSet};

/// Teams whose live submission has finished fewer than `series` ranked series. Empty if `series` <= 0.
pub fn placing_teams(tx: &Transaction, series: i64) -> ApiResult<BTreeSet<String>> {
    if series <= 0 {
        return Ok(BTreeSet::new());
    }
    let mut st = tx.prepare_cached(
        "SELECT s.team_id FROM submissions s WHERE s.live = 1 AND
           (SELECT COUNT(*) FROM series x WHERE x.kind = 'ranked' AND x.state != 'running'
              AND ((x.team_a = s.team_id AND x.sub_a = s.id) OR (x.team_b = s.team_id AND x.sub_b = s.id))) < ?1",
    )?;
    let rows = st.query_map([series], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Placement series started for each team since `since`.
pub fn started_since(tx: &Transaction, since: i64) -> ApiResult<BTreeMap<String, i64>> {
    let mut st = tx.prepare_cached(
        "SELECT placement_for, COUNT(*) FROM series
         WHERE placement_for IS NOT NULL AND created_at > ?1 GROUP BY placement_for",
    )?;
    let rows = st.query_map([since], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Mark a series as a placement series of `team_id`.
pub fn mark(tx: &Transaction, series_id: &str, team_id: &str) -> ApiResult<()> {
    tx.execute(
        "UPDATE series SET placement_for = ?2 WHERE id = ?1",
        params![series_id, team_id],
    )?;
    Ok(())
}
