//! Series, games ("matches" in the API) and the inputs of the ranked matchmaker.
//!
//! A series is N games between two submissions: for ranked and organiser series the ones live for the two
//! teams when it was created, for a scrim whatever the request resolved to. They are pinned on the series
//! and in its jobs, so a later upload or a change of live submission does not affect it, and a ranked
//! series is always rated. Each game is a row pointing at a job; the job holds the result (several games
//! may share one job through the idempotency key). A series is closed once none of its jobs, nor a spot
//! verification of one of them, is outstanding.
//!
//! The archive is private: the public views name teams, never submissions. A caller sees the submission
//! its own team played (`Viewer`); only the organisers see both.

use super::jobs::{self, Enqueued};
use super::{current_patch, live_submission, pool_maps, ratings, SubmissionRow, TeamRow};
use crate::error::{ApiError, ApiResult};
use crate::model::{BotRef, Class, JobSpec, MapRef};
use crate::rating::{Outcome, RatingHook};
use crate::util::{opt_rfc3339, random_hex, rfc3339};
use ibc_rating::{Pairing, TeamState, Winner};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Series kinds and the priority class their games are queued in.
pub fn class_of_kind(kind: &str) -> Option<Class> {
    match kind {
        "ranked" => Some(Class::Ranked),
        "unranked" => Some(Class::Unranked),
        "tournament" => Some(Class::Tournament),
        _ => None,
    }
}

/// Games hosted so far by each map of `pool`, in pool order.
fn map_usage(tx: &Transaction, pool: &[(String, String)]) -> ApiResult<Vec<u64>> {
    let mut st = tx.prepare_cached("SELECT games FROM map_usage WHERE name = ?1")?;
    pool.iter()
        .map(|(name, _)| {
            let n: Option<i64> = st.query_row([name], |r| r.get(0)).optional()?;
            Ok(n.unwrap_or(0).max(0) as u64)
        })
        .collect()
}

/// Games the two teams played against each other on each map of `pool` since `since`, in pool order.
fn pair_map_usage(
    tx: &Transaction,
    pool: &[(String, String)],
    a: &str,
    b: &str,
    since: i64,
) -> ApiResult<Vec<u64>> {
    let mut st = tx.prepare_cached(
        "SELECT g.map_name, COUNT(*) FROM games g JOIN series s ON s.id = g.series_id
         WHERE ((s.team_a = ?1 AND s.team_b = ?2) OR (s.team_a = ?2 AND s.team_b = ?1)) AND g.created_at > ?3
         GROUP BY g.map_name",
    )?;
    let rows = st.query_map(params![a, b, since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let played: BTreeMap<String, i64> = rows.collect::<Result<_, _>>()?;
    Ok(pool
        .iter()
        .map(|(name, _)| played.get(name).copied().unwrap_or(0).max(0) as u64)
        .collect())
}

/// The 64-bit number behind a series id (`s` + 16 hex digits), which seeds its plan.
fn series_number(id: &str) -> u64 {
    u64::from_str_radix(&id[1..], 16).expect("series ids are s + 16 hex digits")
}

/// Which engine slot the series' team A plays in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    /// From a coin flip on the series id, then alternating game by game (every ranked series).
    Alternate,
    /// Team A in engine slot A in every game.
    A,
    /// Team A in engine slot B in every game.
    B,
}

/// Everything a series is created from. The submissions are pinned on the series and its jobs.
pub struct SeriesSpec<'a> {
    pub kind: &'a str,
    pub team_a: &'a TeamRow,
    pub team_b: &'a TeamRow,
    pub sub_a: SubmissionRow,
    pub sub_b: SubmissionRow,
    pub games: i64,
    /// `(name, sha256)` played in turn, cycling; `None`: the pool, least-used maps first.
    pub maps: Option<Vec<(String, String)>>,
    pub sides: Sides,
    /// The team whose unranked quota the games count against.
    pub requested_by: Option<&'a str>,
}

/// Whether a pool map may be played in ranked series and scrims and served by `GET /v1/maps/{name}`: not
/// while a tournament holds it back until its reveal (`tournaments::hidden_maps`).
pub fn map_is_public(tx: &Transaction, name: &str) -> ApiResult<bool> {
    Ok(!super::tournaments::hidden_maps(tx)?.contains(name))
}

/// The team's live submission, or 409 `no_live_submission`.
pub fn require_live(tx: &Transaction, team: &TeamRow) -> ApiResult<SubmissionRow> {
    live_submission(tx, &team.id)?.ok_or_else(|| {
        ApiError::conflict(
            "no_live_submission",
            format!("team {:?} has no live submission", team.name),
        )
    })
}

/// Create a series between the live submissions of two teams (see `create_series_with`).
pub fn create_series(
    tx: &Transaction,
    kind: &str,
    team_a: &TeamRow,
    team_b: &TeamRow,
    games: i64,
    requested_by: Option<&str>,
    now: i64,
) -> ApiResult<(String, usize)> {
    let spec = SeriesSpec {
        kind,
        team_a,
        team_b,
        sub_a: require_live(tx, team_a)?,
        sub_b: require_live(tx, team_b)?,
        games,
        maps: None,
        sides: Sides::Alternate,
        requested_by,
    };
    create_series_with(tx, &spec, now)
}

/// Create a series and queue its games, planned by `ibc_rating::plan_series`: the maps of the pool the two
/// teams played least against each other in the last 24 h, least-used first among those (unless the spec
/// names maps), engine slots alternating from a coin flip (unless the spec fixes
/// them), seeds from the series id and game index. Returns the series id and how many of its games were
/// already in the result cache.
pub fn create_series_with(
    tx: &Transaction,
    spec: &SeriesSpec,
    now: i64,
) -> ApiResult<(String, usize)> {
    let SeriesSpec {
        kind,
        team_a,
        team_b,
        sub_a,
        sub_b,
        ..
    } = spec;
    let class = class_of_kind(kind)
        .ok_or_else(|| ApiError::bad_request(format!("unknown series kind {kind:?}")))?;
    if team_a.id == team_b.id {
        return Err(ApiError::bad_request("a team cannot play itself"));
    }
    super::moderation::ensure_can_play(tx, [team_a, team_b], spec.requested_by, now)?;
    let games = u32::try_from(spec.games)
        .ok()
        .filter(|g| *g > 0)
        .ok_or_else(|| ApiError::bad_request("a series has at least one game"))?;
    let patch = current_patch(tx)?
        .ok_or_else(|| ApiError::conflict("no_patch", "no current patch is set"))?;
    let (pool, usage, pair_usage) = match &spec.maps {
        Some(maps) => (maps.clone(), vec![0; maps.len()], Vec::new()),
        None => {
            let mut pool = Vec::new();
            for map in pool_maps(tx)? {
                if map_is_public(tx, &map.0)? {
                    pool.push(map);
                }
            }
            let usage = map_usage(tx, &pool)?;
            let pair = pair_map_usage(tx, &pool, &team_a.id, &team_b.id, now - 86_400_000)?;
            (pool, usage, pair)
        }
    };
    if pool.is_empty() {
        return Err(ApiError::conflict("no_maps", "the map pool is empty"));
    }

    let id = format!("s{}", random_hex(8));
    tx.execute(
        "INSERT INTO series(id, kind, team_a, team_b, sub_a, sub_b, games, state, requested_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?9)",
        params![id, kind, team_a.id, team_b.id, sub_a.id, sub_b.id, games, spec.requested_by, now],
    )?;
    // Team A is slot 0 of the pairing, team B slot 1.
    let plan = ibc_rating::plan_series(
        series_number(&id),
        Pairing {
            team_a: 0,
            team_b: 1,
        },
        &usage,
        &pair_usage,
        games,
    );
    let bot = |s: &SubmissionRow| BotRef::submission(s.sha256.clone(), s.language.clone());
    let from_rotation = spec.maps.is_none();
    let mut cache_hits = 0;
    for g in &plan.games {
        let swapped = match spec.sides {
            Sides::Alternate => g.slot_a != 0,
            Sides::A => false,
            Sides::B => true,
        };
        let map = match spec.maps {
            Some(_) => g.index as usize % pool.len(),
            None => g.map as usize,
        };
        let (map_name, map_sha) = &pool[map];
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
            seed: g.seed,
            max_ticks: None,
        }
        .normalize()
        .map_err(ApiError::internal)?;
        let Enqueued { job_id, result, .. } = jobs::enqueue(tx, class, &spec, now)?;
        cache_hits += usize::from(result.is_some());
        tx.execute(
            "INSERT INTO games(id, series_id, idx, job_id, map_name, seed, swapped, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![format!("{id}-{}", g.index), id, g.index, job_id, map_name, g.seed.to_string(), swapped, now],
        )?;
        // Only the rotation's own picks count: maps a team chose for a scrim must not steer which maps
        // later ranked series play.
        if from_rotation {
            tx.execute(
                "INSERT INTO map_usage(name, games) VALUES (?1, 1) ON CONFLICT(name) DO UPDATE SET games = games + 1",
                [map_name],
            )?;
        }
    }
    Ok((id, cache_hits))
}

// ---- closing series ----

/// Set on a closed series that a quarantine reopened; it is rated again when its re-runs finish.
pub const REOPENED_NOTE: &str =
    "reopened: a result was invalidated by a quarantine; its games are re-run";

/// The outcome of a finished match by engine slot: the result's `outcome`, or for a worker that does not
/// send it, the `outcome` of its `summary.json`. `None` for `"unfinished"` (or anything unknown): such a
/// game has no result and is not rated (it is not a draw).
pub fn slot_winner(result: &Value) -> Option<Winner> {
    result
        .get("outcome")
        .or_else(|| result.get("summary")?.get("outcome"))
        .and_then(Value::as_str)
        .and_then(Winner::from_job_outcome)
}

/// The game result from team A's point of view (`swapped`: team A played in engine slot B).
fn outcome_of(result: &Value, swapped: bool) -> Option<Outcome> {
    Some(match (slot_winner(result)?, swapped) {
        (Winner::Draw, _) => Outcome::Draw,
        (Winner::SlotA, false) | (Winner::SlotB, true) => Outcome::AWins,
        (Winner::SlotA, true) | (Winner::SlotB, false) => Outcome::BWins,
    })
}

/// Close every running series with nothing outstanding and rate the ranked ones; a rated series writes a
/// ladder snapshot. Returns the number of series closed.
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
    super::tournaments::on_series_closed(tx, &ready, now)?;
    if rated_any {
        ratings::write_snapshot(tx, hook, now, snapshots_kept)?;
    }
    Ok(ready.len())
}

/// Close one series. Every game that finished counts; a game that could not be run (dead-lettered) or
/// ended unfinished is left out, and a series where no game finished is void. A ranked series is rated
/// once, from its maps won, lost and drawn (`ibc_rating::rate_series`: each map's update from the
/// pre-series ratings, summed), whatever is live now.
/// Returns true if ratings changed.
fn finalize_one(tx: &Transaction, hook: &dyn RatingHook, id: &str, now: i64) -> ApiResult<bool> {
    let (kind, team_a, team_b, prev_note): (String, String, String, Option<String>) = tx
        .query_row(
            "SELECT kind, team_a, team_b, note FROM series WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    let games: Vec<(i64, bool, String, Option<String>)> = {
        let mut st = tx.prepare_cached(
            "SELECT g.idx, g.swapped, j.state, j.result FROM games g JOIN jobs j ON j.id = g.job_id
             WHERE g.series_id = ?1 ORDER BY g.idx",
        )?;
        let rows = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let close = |state: &str, rated: bool, wld: (i64, i64, i64), note: Option<&str>| {
        tx.execute(
            "UPDATE series SET state = ?2, rated = ?3, wins_a = ?4, wins_b = ?5, draws = ?6, note = ?7, finished_at = ?8 WHERE id = ?1",
            params![id, state, rated, wld.0, wld.1, wld.2, note, now],
        )
    };
    let mut not_run = 0;
    let mut finished: Vec<(i64, Option<Outcome>)> = Vec::new();
    for (idx, swapped, state, result) in &games {
        if state != "done" {
            not_run += 1;
            continue;
        }
        let result: Value = result
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        finished.push((*idx, outcome_of(&result, *swapped)));
    }
    if finished.is_empty() {
        close(
            "void",
            false,
            (0, 0, 0),
            Some("no game could be run (see the dead-letter list)"),
        )?;
        return Ok(false);
    }
    let count = |o: Outcome| finished.iter().filter(|(_, x)| *x == Some(o)).count() as i64;
    let wld = (
        count(Outcome::AWins),
        count(Outcome::BWins),
        count(Outcome::Draw),
    );
    let unfinished = finished.iter().filter(|(_, o)| o.is_none()).count();
    let mut notes = Vec::new();
    if prev_note.as_deref() == Some(REOPENED_NOTE) {
        notes.push("re-rated after a quarantine invalidated a result".to_string());
    }
    if not_run > 0 {
        notes.push(format!(
            "{not_run} game(s) could not be run (see the dead-letter list)"
        ));
    }
    if unfinished > 0 {
        notes.push(format!("{unfinished} game(s) ended unfinished"));
    }
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    let outcomes: Vec<Outcome> = finished.iter().filter_map(|(_, o)| *o).collect();
    let rated = kind == "ranked" && !outcomes.is_empty();
    if rated {
        let series = ratings::RatedSeries {
            series_id: id,
            team_a: &team_a,
            team_b: &team_b,
            outcomes: &outcomes,
        };
        ratings::rate_series(tx, hook, &series, now)?;
    }
    close("done", rated, wld, note.as_deref())?;
    Ok(rated)
}

// ---- matchmaking inputs ----

/// The ranked matchmaker's view of the league: every team with a live submission (sorted by id; the
/// matchmaker's `TeamId` is the index), with its rating, rated games and ranked opponents of the last 24 h,
/// its ranked series created since `cap_since` (the cap window) and when it last played. Teams that still
/// have a ranked series running are marked not live, so the ranked backlog cannot grow faster than the
/// workers clear it.
pub fn matchmaking_pool(
    tx: &Transaction,
    hook: &dyn RatingHook,
    now: i64,
    cap_since: i64,
) -> ApiResult<Vec<(TeamRow, TeamState)>> {
    let pairs = |sql: &str, since: i64| -> ApiResult<Vec<(String, String)>> {
        let mut st = tx.prepare(sql)?;
        let rows = st.query_map([since], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    };
    let teams: Vec<TeamRow> = {
        let mut st = tx.prepare(
            "SELECT t.id, t.name FROM teams t JOIN submissions s ON s.team_id = t.id AND s.live = 1 ORDER BY t.id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(TeamRow {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let blocked = super::moderation::blocked_teams(tx, now)?;
    let teams: Vec<TeamRow> = teams
        .into_iter()
        .filter(|t| !blocked.contains(&t.id))
        .collect();
    let index: BTreeMap<&str, u64> = teams
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id.as_str(), i as u64))
        .collect();
    let day = now - 86_400_000;
    // `created_at > 0`: every running ranked series.
    let busy: BTreeSet<String> = pairs(
        "SELECT team_a, team_b FROM series WHERE state = 'running' AND kind = 'ranked' AND created_at > ?1",
        0,
    )?
    .into_iter()
    .flat_map(|(a, b)| [a, b])
    .collect();
    let mut recent_games: BTreeMap<String, u32> = BTreeMap::new();
    {
        let mut st = tx.prepare_cached(
            "SELECT kind, team_a, team_b, outcome FROM rating_events
             WHERE kind IN ('game', 'series') AND created_at > ?1",
        )?;
        let rows = st.query_map([day], |r| {
            let (kind, outcome): (String, Option<String>) = (r.get(0)?, r.get(3)?);
            let games = ratings::event_games(&kind, outcome.as_deref());
            Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?, games))
        })?;
        for row in rows {
            let (a, b, games) = row?;
            *recent_games.entry(a).or_default() += games;
            *recent_games.entry(b).or_default() += games;
        }
    }
    let mut recent_series: BTreeMap<String, u32> = BTreeMap::new();
    for (a, b) in pairs(
        "SELECT team_a, team_b FROM series WHERE kind = 'ranked' AND created_at > ?1",
        cap_since,
    )? {
        *recent_series.entry(a).or_default() += 1;
        *recent_series.entry(b).or_default() += 1;
    }
    let mut opponents: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (a, b) in pairs(
        "SELECT team_a, team_b FROM series WHERE kind = 'ranked' AND created_at > ?1 ORDER BY created_at, id",
        day,
    )? {
        if let (Some(&ia), Some(&ib)) = (index.get(a.as_str()), index.get(b.as_str())) {
            opponents.entry(ia).or_default().push(ib);
            opponents.entry(ib).or_default().push(ia);
        }
    }
    let mut out = Vec::with_capacity(teams.len());
    for (i, team) in teams.into_iter().enumerate() {
        let standing = ratings::load(tx, hook, &team.id)?;
        let state = TeamState {
            id: i as u64,
            rating: standing.rating,
            recent_games: recent_games.get(&team.id).copied().unwrap_or(0),
            last_played: standing.last_played.map(|ms| (ms.max(0) / 1000) as u64),
            recent_opponents: opponents.remove(&(i as u64)).unwrap_or_default(),
            recent_series: recent_series.get(&team.id).copied().unwrap_or(0),
            live: !busy.contains(&team.id),
        };
        out.push((team, state));
    }
    Ok(out)
}

/// Mean wall-clock seconds of the matches finished since `since` that reported `wall_ms`. A worker runs one
/// match per runner process at a time, so this is core-seconds per game. `None` if there are none.
pub fn mean_game_seconds(tx: &Transaction, since: i64) -> ApiResult<Option<f64>> {
    let mean: Option<f64> = tx.query_row(
        "SELECT AVG(json_extract(result, '$.wall_ms')) FROM jobs
         WHERE finished_at > ?1 AND state = 'done' AND json_extract(result, '$.wall_ms') IS NOT NULL",
        [since],
        |r| r.get(0),
    )?;
    Ok(mean.map(|ms| ms / 1000.0))
}

/// Jobs of any class waiting for a worker.
pub fn queued_jobs(tx: &Transaction) -> ApiResult<i64> {
    Ok(tx.query_row(
        "SELECT COUNT(*) FROM jobs WHERE state = 'queued'",
        [],
        |r| r.get(0),
    )?)
}

/// Remember the seed of a ranked batch, so its pairings can be replayed.
pub fn record_batch(
    tx: &Transaction,
    seed: u64,
    budget: u32,
    created: usize,
    now: i64,
) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO ranked_batches(seed, budget, created, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![seed.to_string(), budget, created as i64, now],
    )?;
    Ok(())
}

// ---- views ----

const MATCH_SQL: &str = "
SELECT g.id, g.series_id, g.idx, g.map_name, g.seed, g.swapped, s.kind, s.team_a, ta.name, s.team_b, tb.name,
       j.state, j.result, j.replay_sha256, j.verified, g.created_at, j.finished_at, j.id, j.error
FROM games g JOIN series s ON s.id = g.series_id JOIN teams ta ON ta.id = s.team_a JOIN teams tb ON tb.id = s.team_b
JOIN jobs j ON j.id = g.job_id";

/// Who is looking at the archive. Submission ids and module hashes are shown only for the viewer's own team
/// (the organisers see every team): another team's versions, and how many it has fielded, stay private.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Viewer {
    Public,
    Team(String),
    Admin,
}

impl Viewer {
    /// May see what `team_id` submitted.
    pub fn sees(&self, team_id: &str) -> bool {
        match self {
            Viewer::Public => false,
            Viewer::Team(own) => own == team_id,
            Viewer::Admin => true,
        }
    }
}

/// `summary.json` names each side's module (`teams[i].wasm_sha256`, and `name`, a hash prefix since workers
/// never name bots): drop the hash for every side the viewer may not see, and name every side by its team.
/// `slot_teams` is the team (`{team_id, name}`) in each slot.
fn redact_summary(mut summary: Value, slot_teams: [&Value; 2], viewer: &Viewer) -> Value {
    for (i, team) in slot_teams.iter().enumerate() {
        if let Some(side) = summary
            .get_mut("teams")
            .and_then(|t| t.get_mut(i))
            .and_then(Value::as_object_mut)
        {
            if !viewer.sees(team["team_id"].as_str().unwrap_or_default()) {
                side.remove("wasm_sha256");
            }
            side.insert("name".into(), team["name"].clone());
        }
    }
    summary
}

/// One game as the API shows it. `bot_a` / `bot_b` are the sides of the match (`score` is in that order);
/// `summary` (the runner's `summary.json`, redacted for `viewer`) is only included in the detail view, and
/// the job behind the game only for the organisers.
fn match_view(r: &rusqlite::Row, detail: Option<&Viewer>) -> rusqlite::Result<Value> {
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
        "outcome": if done { json!(slot_winner(&result).map(|w| match w {
            Winner::SlotA => "a", Winner::SlotB => "b", Winner::Draw => "draw" }).unwrap_or("unfinished")) } else { Value::Null },
        "score": if done { result.get("score").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "ticks": if done { result.get("ticks").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "replay_sha256": r.get::<_, Option<String>>(13)?,
        "replay_url": if done { json!(format!("/v1/matches/{id}/replay")) } else { Value::Null },
        "verified": r.get::<_, bool>(14)?,
        "created_at": rfc3339(r.get(15)?), "finished_at": opt_rfc3339(r.get(16)?),
    });
    if let Some(viewer) = detail {
        if *viewer == Viewer::Admin {
            v["job_id"] = json!(r.get::<_, String>(17)?);
            v["error"] = json!(r.get::<_, Option<String>>(18)?);
        }
        v["summary"] = match result.get("summary") {
            Some(summary) if done => {
                let slots = [&v["bot_a"], &v["bot_b"]];
                redact_summary(summary.clone(), slots, viewer)
            }
            _ => Value::Null,
        };
    }
    Ok(v)
}

#[derive(Debug, Default)]
pub struct MatchFilter {
    pub team: Option<String>,
    pub series: Option<String>,
    /// `ranked`, `unranked` or `tournament`.
    pub kind: Option<String>,
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
           AND (?5 IS NULL OR s.kind = ?5)
           AND {}
         ORDER BY g.created_at DESC, g.series_id DESC, g.idx DESC LIMIT ?4",
        super::tournaments::VISIBLE_GAME
    );
    let mut st = tx.prepare_cached(&sql)?;
    let rows = st.query_map(params![f.team, f.series, f.before, f.limit, f.kind], |r| {
        match_view(r, None)
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get_match(tx: &Transaction, id: &str, viewer: &Viewer) -> ApiResult<Option<Value>> {
    Ok(tx
        .query_row(
            &format!(
                "{MATCH_SQL} WHERE g.id = ?1 AND {}",
                super::tournaments::VISIBLE_GAME
            ),
            [id],
            |r| match_view(r, Some(viewer)),
        )
        .optional()?)
}

/// The replay of a finished match and the teams that played its sides A and B.
pub struct MatchReplay {
    pub sha256: String,
    pub team_ids: [String; 2],
    pub team_names: [String; 2],
}

pub fn match_replay(tx: &Transaction, id: &str) -> ApiResult<Option<MatchReplay>> {
    Ok(tx
        .query_row(
            &format!(
                "SELECT j.replay_sha256, g.swapped, s.team_a, ta.name, s.team_b, tb.name
                 FROM games g JOIN jobs j ON j.id = g.job_id JOIN series s ON s.id = g.series_id
                 JOIN teams ta ON ta.id = s.team_a JOIN teams tb ON tb.id = s.team_b
                 WHERE g.id = ?1 AND j.state = 'done' AND {}",
                super::tournaments::VISIBLE_GAME
            ),
            [id],
            |r| {
                let a: (String, String) = (r.get(2)?, r.get(3)?);
                let b: (String, String) = (r.get(4)?, r.get(5)?);
                let (a, b) = if r.get::<_, bool>(1)? { (b, a) } else { (a, b) };
                Ok(r.get::<_, Option<String>>(0)?.map(|sha256| MatchReplay {
                    sha256,
                    team_ids: [a.0, b.0],
                    team_names: [a.1, b.1],
                }))
            },
        )
        .optional()?
        .flatten())
}

/// A series and its games. Each side shows the submission it played (pinned at creation) only to a viewer
/// who may see that team.
pub fn series_view(tx: &Transaction, id: &str, viewer: &Viewer) -> ApiResult<Option<Value>> {
    let head = tx
        .query_row(
            &format!("SELECT s.id, s.kind, s.team_a, ta.name, s.team_b, tb.name, s.sub_a, s.sub_b, s.games, s.state, s.rated,
                    s.wins_a, s.wins_b, s.draws, s.note, s.created_at, s.finished_at, sa.sha256, sb.sha256
             FROM series s JOIN teams ta ON ta.id = s.team_a JOIN teams tb ON tb.id = s.team_b
             JOIN submissions sa ON sa.id = s.sub_a JOIN submissions sb ON sb.id = s.sub_b WHERE s.id = ?1 AND {}",
            super::tournaments::VISIBLE_SERIES),
            [id],
            |r| {
                let side = |team: usize, name: usize, sub: usize, sha: usize| -> rusqlite::Result<Value> {
                    let team_id: String = r.get(team)?;
                    let mut v = json!({"team_id": team_id, "name": r.get::<_, String>(name)?});
                    if viewer.sees(&team_id) {
                        v["submission_id"] = json!(r.get::<_, String>(sub)?);
                        v["sha256"] = json!(r.get::<_, String>(sha)?);
                    }
                    Ok(v)
                };
                Ok(json!({
                    "series_id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?,
                    "team_a": side(2, 3, 6, 17)?, "team_b": side(4, 5, 7, 18)?,
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
