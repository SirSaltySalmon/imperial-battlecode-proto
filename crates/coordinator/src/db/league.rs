//! Series, games ("matches" in the API), ratings and ladder snapshots.
//!
//! A series is N games between the live submissions of two teams at the time it is created. Each game is a
//! row pointing at a job; the job holds the result (several games may share one job through the idempotency
//! key). A series is closed once none of its jobs, nor a spot verification of one of them, is outstanding.

use super::jobs::{self, Enqueued};
use super::{current_patch, live_submission, pool_maps, TeamRow};
use crate::error::{ApiError, ApiResult};
use crate::model::{BotRef, Class, JobSpec, MapRef};
use crate::rating::{Outcome, Rating, RatingHook};
use crate::util::{hash_u64, opt_rfc3339, random_hex, rfc3339};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};

/// Series kinds and the priority class their games are queued in.
pub fn class_of_kind(kind: &str) -> Option<Class> {
    match kind {
        "ranked" => Some(Class::Ranked),
        "unranked" => Some(Class::Unranked),
        "tournament" => Some(Class::Tournament),
        _ => None,
    }
}

/// Games of unranked series `team_id` has requested since `since`.
pub fn games_requested_since(tx: &Transaction, team_id: &str, since: i64) -> ApiResult<i64> {
    Ok(tx.query_row(
        "SELECT COALESCE(SUM(games), 0) FROM series WHERE requested_by = ?1 AND created_at > ?2",
        params![team_id, since],
        |r| r.get(0),
    )?)
}

/// Create a series and queue its games: maps rotate through the pool (each map is played once per side
/// before the next one), seeds derive from the series id and game index, sides alternate.
/// Returns the series id and how many of its games were already in the result cache.
pub fn create_series(
    tx: &Transaction,
    kind: &str,
    team_a: &TeamRow,
    team_b: &TeamRow,
    games: i64,
    requested_by: Option<&str>,
    now: i64,
) -> ApiResult<(String, usize)> {
    let class = class_of_kind(kind)
        .ok_or_else(|| ApiError::bad_request(format!("unknown series kind {kind:?}")))?;
    if team_a.id == team_b.id {
        return Err(ApiError::bad_request("a team cannot play itself"));
    }
    let no_live = |t: &TeamRow| {
        ApiError::conflict(
            "no_live_submission",
            format!("team {:?} has no live submission", t.name),
        )
    };
    let sub_a = live_submission(tx, &team_a.id)?.ok_or_else(|| no_live(team_a))?;
    let sub_b = live_submission(tx, &team_b.id)?.ok_or_else(|| no_live(team_b))?;
    let patch = current_patch(tx)?
        .ok_or_else(|| ApiError::conflict("no_patch", "no current patch is set"))?;
    let pool = pool_maps(tx)?;
    if pool.is_empty() {
        return Err(ApiError::conflict("no_maps", "the map pool is empty"));
    }

    let id = format!("s{}", random_hex(8));
    tx.execute(
        "INSERT INTO series(id, kind, team_a, team_b, sub_a, sub_b, games, state, requested_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?9)",
        params![id, kind, team_a.id, team_b.id, sub_a.id, sub_b.id, games, requested_by, now],
    )?;
    let rotation = hash_u64(&id);
    let mut cache_hits = 0;
    for idx in 0..games {
        let swapped = idx % 2 == 1;
        let (map_name, map_sha) =
            &pool[((rotation % pool.len() as u64) as usize + (idx / 2) as usize) % pool.len()];
        let seed = hash_u64(&format!("{id}\n{idx}"));
        let bot = |s: &super::SubmissionRow| BotRef {
            sha256: s.sha256.clone(),
            fuel_mult: s.fuel_mult,
        };
        let (bot_a, bot_b) = if swapped {
            (bot(&sub_b), bot(&sub_a))
        } else {
            (bot(&sub_a), bot(&sub_b))
        };
        let spec = JobSpec {
            patch: patch.clone(),
            bot_a,
            bot_b,
            map: MapRef {
                sha256: map_sha.clone(),
            },
            seed,
            max_ticks: None,
        }
        .normalize()
        .map_err(ApiError::internal)?;
        let Enqueued { job_id, result, .. } = jobs::enqueue(tx, class, &spec, now)?;
        cache_hits += usize::from(result.is_some());
        tx.execute(
            "INSERT INTO games(id, series_id, idx, job_id, map_name, seed, swapped, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![format!("{id}-{idx}"), id, idx, job_id, map_name, seed.to_string(), swapped, now],
        )?;
    }
    Ok((id, cache_hits))
}

// ---- closing series, ratings ----

fn load_rating(tx: &Transaction, hook: &dyn RatingHook, team_id: &str) -> ApiResult<Rating> {
    Ok(tx
        .query_row(
            "SELECT mu, sigma, games FROM ratings WHERE team_id = ?1",
            [team_id],
            |r| {
                Ok(Rating {
                    mu: r.get(0)?,
                    sigma: r.get(1)?,
                    games: r.get(2)?,
                })
            },
        )
        .optional()?
        .unwrap_or_else(|| hook.initial()))
}

fn save_rating(
    tx: &Transaction,
    team_id: &str,
    r: &Rating,
    wld: (i64, i64, i64),
    now: i64,
) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO ratings(team_id, mu, sigma, games, wins, losses, draws, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(team_id) DO UPDATE SET mu = excluded.mu, sigma = excluded.sigma, games = excluded.games,
           wins = wins + excluded.wins, losses = losses + excluded.losses, draws = draws + excluded.draws,
           updated_at = excluded.updated_at",
        params![team_id, r.mu, r.sigma, r.games, wld.0, wld.1, wld.2, now],
    )?;
    Ok(())
}

/// The game result from team A's point of view (`swapped`: team A played as bot B).
fn outcome_of(result: &Value, swapped: bool) -> Outcome {
    match (result.get("winner").and_then(Value::as_i64), swapped) {
        (Some(0), false) | (Some(1), true) => Outcome::WinA,
        (Some(1), false) | (Some(0), true) => Outcome::WinB,
        _ => Outcome::Draw,
    }
}

/// Close every running series with nothing outstanding. A series with a dead game is void. A ranked series
/// is rated game by game unless one of the two submissions has been superseded since it was created; a
/// rated series writes a ladder snapshot. Returns the number of series closed.
pub fn finalize_ready(
    tx: &Transaction,
    hook: &dyn RatingHook,
    now: i64,
    snapshots_kept: i64,
) -> ApiResult<usize> {
    let ready: Vec<String> = {
        let mut st = tx.prepare_cached(
            "SELECT s.id FROM series s WHERE s.state = 'running'
               AND NOT EXISTS (SELECT 1 FROM games g JOIN jobs j ON j.id = g.job_id
                               WHERE g.series_id = s.id AND j.state IN ('queued', 'leased'))
               AND NOT EXISTS (SELECT 1 FROM games g JOIN jobs v ON v.verifies = g.job_id
                               WHERE g.series_id = s.id AND v.state IN ('queued', 'leased'))
             ORDER BY s.created_at, s.id",
        )?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut rated_any = false;
    for id in &ready {
        rated_any |= finalize_one(tx, hook, id, now)?;
    }
    if rated_any {
        write_snapshot(tx, hook, now, snapshots_kept)?;
    }
    Ok(ready.len())
}

/// Returns true if ratings changed.
fn finalize_one(tx: &Transaction, hook: &dyn RatingHook, id: &str, now: i64) -> ApiResult<bool> {
    let (kind, team_a, team_b, sub_a, sub_b): (String, String, String, String, String) = tx
        .query_row(
            "SELECT kind, team_a, team_b, sub_a, sub_b FROM series WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
    let games: Vec<(bool, String, Option<String>)> = {
        let mut st = tx.prepare_cached(
            "SELECT g.swapped, j.state, j.result FROM games g JOIN jobs j ON j.id = g.job_id WHERE g.series_id = ?1 ORDER BY g.idx",
        )?;
        let rows = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let close = |state: &str, rated: bool, wld: (i64, i64, i64), note: Option<&str>| {
        tx.execute(
            "UPDATE series SET state = ?2, rated = ?3, wins_a = ?4, wins_b = ?5, draws = ?6, note = ?7, finished_at = ?8 WHERE id = ?1",
            params![id, state, rated, wld.0, wld.1, wld.2, note, now],
        )
    };
    if games.iter().any(|(_, state, _)| state != "done") {
        close(
            "void",
            false,
            (0, 0, 0),
            Some("a game could not be run (see the dead-letter list)"),
        )?;
        return Ok(false);
    }
    let outcomes: Vec<Outcome> = games
        .iter()
        .map(|(swapped, _, result)| {
            let result: Value = result
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            outcome_of(&result, *swapped)
        })
        .collect();
    let count = |o: Outcome| outcomes.iter().filter(|x| **x == o).count() as i64;
    let wld = (
        count(Outcome::WinA),
        count(Outcome::WinB),
        count(Outcome::Draw),
    );

    if kind != "ranked" {
        close("done", false, wld, None)?;
        return Ok(false);
    }
    let still_live = |team: &str, sub: &str| -> ApiResult<bool> {
        Ok(live_submission(tx, team)?.is_some_and(|s| s.id == sub))
    };
    if !(still_live(&team_a, &sub_a)? && still_live(&team_b, &sub_b)?) {
        close(
            "done",
            false,
            wld,
            Some("not rated: a submission was superseded before the series finished"),
        )?;
        return Ok(false);
    }
    let (mut ra, mut rb) = (
        load_rating(tx, hook, &team_a)?,
        load_rating(tx, hook, &team_b)?,
    );
    for outcome in &outcomes {
        (ra, rb) = hook.update(&ra, &rb, *outcome);
    }
    save_rating(tx, &team_a, &ra, wld, now)?;
    save_rating(tx, &team_b, &rb, (wld.1, wld.0, wld.2), now)?;
    close("done", true, wld, None)?;
    Ok(true)
}

// ---- ladder ----

struct LadderRow {
    team_id: String,
    name: String,
    submission: String,
    rating: Rating,
    wld: (i64, i64, i64),
}

/// Teams with a live submission and their ratings, best first.
fn ladder_rows(tx: &Transaction, hook: &dyn RatingHook) -> ApiResult<Vec<LadderRow>> {
    let initial = hook.initial();
    let mut st = tx.prepare(
        "SELECT t.id, t.name, s.id, r.mu, r.sigma, r.games, r.wins, r.losses, r.draws
         FROM teams t JOIN submissions s ON s.team_id = t.id AND s.live = 1 LEFT JOIN ratings r ON r.team_id = t.id",
    )?;
    let rows = st.query_map([], |r| {
        let rating = match r.get::<_, Option<f64>>(3)? {
            Some(mu) => Rating {
                mu,
                sigma: r.get(4)?,
                games: r.get(5)?,
            },
            None => initial,
        };
        let n = |i: usize| r.get::<_, Option<i64>>(i).map(|v| v.unwrap_or(0));
        Ok(LadderRow {
            team_id: r.get(0)?,
            name: r.get(1)?,
            submission: r.get(2)?,
            rating,
            wld: (n(6)?, n(7)?, n(8)?),
        })
    })?;
    let mut rows: Vec<LadderRow> = rows.collect::<Result<_, _>>()?;
    rows.sort_by(|a, b| {
        hook.display(&b.rating)
            .total_cmp(&hook.display(&a.rating))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(rows)
}

pub fn write_snapshot(
    tx: &Transaction,
    hook: &dyn RatingHook,
    now: i64,
    kept: i64,
) -> ApiResult<()> {
    let teams: Vec<Value> = ladder_rows(tx, hook)?
        .iter()
        .enumerate()
        .map(|(i, row)| {
            json!({"rank": i + 1, "team_id": row.team_id, "name": row.name, "rating": hook.display(&row.rating),
                   "mu": row.rating.mu, "sigma": row.rating.sigma, "games": row.rating.games,
                   "wins": row.wld.0, "losses": row.wld.1, "draws": row.wld.2, "submission_id": row.submission})
        })
        .collect();
    let body = json!({"generated_at": rfc3339(now), "system": hook.name(), "teams": teams});
    tx.execute(
        "INSERT INTO ladder_snapshots(created_at, body) VALUES (?1, ?2)",
        params![now, body.to_string()],
    )?;
    tx.execute(
        "DELETE FROM ladder_snapshots WHERE id <= (SELECT MAX(id) FROM ladder_snapshots) - ?1",
        [kept.max(1)],
    )?;
    Ok(())
}

/// The body of the latest snapshot (already JSON).
pub fn latest_snapshot(tx: &Transaction) -> ApiResult<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT body FROM ladder_snapshots ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

/// Pairs for one scheduler round: live teams sorted by rating, neighbours paired, the pairing offset
/// alternating with `round` so a team meets both of its neighbours. Teams that still have a ranked series
/// running are left out, which also bounds the ranked backlog when workers are slow.
pub fn auto_pairings(
    tx: &Transaction,
    hook: &dyn RatingHook,
    budget: usize,
    round: u64,
) -> ApiResult<Vec<(TeamRow, TeamRow)>> {
    let busy: Vec<String> = {
        let mut st = tx.prepare(
            "SELECT team_a FROM series WHERE state = 'running' AND kind = 'ranked'
             UNION SELECT team_b FROM series WHERE state = 'running' AND kind = 'ranked'",
        )?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let free: Vec<TeamRow> = ladder_rows(tx, hook)?
        .into_iter()
        .filter(|row| !busy.contains(&row.team_id))
        .map(|row| TeamRow {
            id: row.team_id,
            name: row.name,
        })
        .collect();
    let offset = if free.len() >= 3 {
        (round % 2) as usize
    } else {
        0
    };
    Ok(free[offset.min(free.len())..]
        .chunks_exact(2)
        .take(budget)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect())
}

// ---- views ----

const MATCH_SQL: &str = "
SELECT g.id, g.series_id, g.idx, g.map_name, g.seed, g.swapped, s.kind, s.team_a, ta.name, s.team_b, tb.name,
       j.state, j.result, j.replay_sha256, j.verified, g.created_at, j.finished_at, j.id, j.error
FROM games g JOIN series s ON s.id = g.series_id JOIN teams ta ON ta.id = s.team_a JOIN teams tb ON tb.id = s.team_b
JOIN jobs j ON j.id = g.job_id";

/// One game as the public API shows it. `bot_a` / `bot_b` are the sides of the match (`score` is in that
/// order); `summary` (the runner's `summary.json`) is only included in the detail view.
fn match_view(r: &rusqlite::Row, detail: bool) -> rusqlite::Result<Value> {
    let swapped: bool = r.get(5)?;
    let team = |id: usize, name: usize| -> rusqlite::Result<Value> {
        Ok(json!({"team_id": r.get::<_, String>(id)?, "name": r.get::<_, String>(name)?}))
    };
    let (team_a, team_b) = (team(7, 8)?, team(9, 10)?);
    let (bot_a, bot_b) = if swapped {
        (team_b, team_a)
    } else {
        (team_a, team_b)
    };
    let state: String = r.get(11)?;
    let result: Value = r
        .get::<_, Option<String>>(12)?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let done = state == "done";
    let winner_team = match result.get("winner").and_then(Value::as_i64) {
        Some(0) if done => bot_a["team_id"].clone(),
        Some(1) if done => bot_b["team_id"].clone(),
        _ => Value::Null,
    };
    let id: String = r.get(0)?;
    let mut v = json!({
        "match_id": id, "series_id": r.get::<_, String>(1)?, "index": r.get::<_, i64>(2)?,
        "kind": r.get::<_, String>(6)?, "map": r.get::<_, String>(3)?,
        "seed": r.get::<_, String>(4)?.parse::<u64>().unwrap_or(0),
        "bot_a": bot_a, "bot_b": bot_b, "state": state,
        "winner": if done { result.get("winner").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "winner_team": winner_team,
        "score": if done { result.get("score").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "ticks": if done { result.get("ticks").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "replay_sha256": r.get::<_, Option<String>>(13)?,
        "replay_url": if done { json!(format!("/v1/matches/{id}/replay")) } else { Value::Null },
        "verified": r.get::<_, bool>(14)?,
        "created_at": rfc3339(r.get(15)?), "finished_at": opt_rfc3339(r.get(16)?),
    });
    if detail {
        v["job_id"] = json!(r.get::<_, String>(17)?);
        v["error"] = json!(r.get::<_, Option<String>>(18)?);
        v["summary"] = if done {
            result.get("summary").cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        };
    }
    Ok(v)
}

#[derive(Debug, Default)]
pub struct MatchFilter {
    pub team: Option<String>,
    pub series: Option<String>,
    /// Only matches listed after this match id: the cursor for paging backwards in time.
    pub before: Option<String>,
    pub limit: i64,
}

/// Most recent first.
pub fn list_matches(tx: &Transaction, f: &MatchFilter) -> ApiResult<Vec<Value>> {
    let sql = format!(
        "{MATCH_SQL} WHERE (?1 IS NULL OR s.team_a = ?1 OR s.team_b = ?1) AND (?2 IS NULL OR g.series_id = ?2)
           AND (?3 IS NULL OR (g.created_at, g.series_id, g.idx) <
                               (SELECT c.created_at, c.series_id, c.idx FROM games c WHERE c.id = ?3))
         ORDER BY g.created_at DESC, g.series_id DESC, g.idx DESC LIMIT ?4"
    );
    let mut st = tx.prepare_cached(&sql)?;
    let rows = st.query_map(params![f.team, f.series, f.before, f.limit], |r| {
        match_view(r, false)
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get_match(tx: &Transaction, id: &str) -> ApiResult<Option<Value>> {
    Ok(tx
        .query_row(&format!("{MATCH_SQL} WHERE g.id = ?1"), [id], |r| {
            match_view(r, true)
        })
        .optional()?)
}

/// The replay hash of a finished match.
pub fn match_replay(tx: &Transaction, id: &str) -> ApiResult<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT j.replay_sha256 FROM games g JOIN jobs j ON j.id = g.job_id WHERE g.id = ?1 AND j.state = 'done'",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

pub fn series_view(tx: &Transaction, id: &str) -> ApiResult<Option<Value>> {
    let head = tx
        .query_row(
            "SELECT s.id, s.kind, s.team_a, ta.name, s.team_b, tb.name, s.sub_a, s.sub_b, s.games, s.state, s.rated,
                    s.wins_a, s.wins_b, s.draws, s.note, s.created_at, s.finished_at
             FROM series s JOIN teams ta ON ta.id = s.team_a JOIN teams tb ON tb.id = s.team_b WHERE s.id = ?1",
            [id],
            |r| {
                Ok(json!({
                    "series_id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?,
                    "team_a": {"team_id": r.get::<_, String>(2)?, "name": r.get::<_, String>(3)?, "submission_id": r.get::<_, String>(6)?},
                    "team_b": {"team_id": r.get::<_, String>(4)?, "name": r.get::<_, String>(5)?, "submission_id": r.get::<_, String>(7)?},
                    "games": r.get::<_, i64>(8)?, "state": r.get::<_, String>(9)?, "rated": r.get::<_, bool>(10)?,
                    "wins_a": r.get::<_, i64>(11)?, "wins_b": r.get::<_, i64>(12)?, "draws": r.get::<_, i64>(13)?,
                    "note": r.get::<_, Option<String>>(14)?,
                    "created_at": rfc3339(r.get(15)?), "finished_at": opt_rfc3339(r.get(16)?),
                }))
            },
        )
        .optional()?;
    let Some(mut head) = head else {
        return Ok(None);
    };
    let mut matches = list_matches(
        tx,
        &MatchFilter {
            series: Some(id.to_string()),
            limit: 1000,
            ..Default::default()
        },
    )?;
    matches.reverse();
    head["matches"] = Value::Array(matches);
    Ok(Some(head))
}
