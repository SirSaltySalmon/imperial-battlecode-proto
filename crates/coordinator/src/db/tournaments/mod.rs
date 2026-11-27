//! Tournaments: a draft (name, format, field, hidden maps), a lock that snapshots each entrant's live
//! submission and ladder seed and pauses ranked matchmaking, a single or double elimination bracket
//! (`ibc_rating::bracket`) whose pairings are `tournament` series playing every tournament map once, and a
//! reveal that makes the maps public and adds them to the ranked pool.
//!
//! The bracket's shape is a pure function of (format, number of entrants), so only the result of each
//! bracket match is stored (`tournament_matches`); the seats are recomputed from the results. A pairing is
//! decided when its series closes (`on_series_closed`, called by `league::finalize_ready`): more maps won,
//! then more rounds won, then the higher seed (flagged as `decided_by = "seed"`). The first decision stands:
//! a series a quarantine reopens later does not change the bracket. Views live in `view`.

use super::jobs::{self, Enqueued};
use super::league::slot_winner;
use super::{current_patch, find_team, ratings, set_setting, setting};
use crate::error::{ApiError, ApiResult};
use crate::model::{BotRef, Class, JobSpec, MapRef};
use crate::rating::{Outcome, RatingHook};
use crate::util::random_hex;
use ibc_rating::bracket::{self, BracketMatch, Format, MapResult, MatchState, Seat, Side};
use ibc_rating::rng::mix64;
use ibc_rating::Winner;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

mod view;
pub use view::{hidden_maps, list, view};

/// Settings key: ranked matchmaking is paused until this time (ms since the epoch).
const RANKED_PAUSED_UNTIL: &str = "ranked_paused_until";

/// The error a job gets when a patch change dead-letters it (`db::set_current_patch`).
const PATCH_SUPERSEDED: &str = "patch superseded";

/// SQL condition, over a games row `g`, that the game is not part of a tournament whose maps are hidden.
pub const VISIBLE_GAME: &str = "NOT EXISTS (SELECT 1 FROM tournament_series ts JOIN tournaments t
    ON t.id = ts.tournament_id WHERE ts.series_id = g.series_id AND t.revealed_at IS NULL)";

/// SQL condition, over a series row `s`, that the series is not part of a tournament whose maps are hidden.
pub const VISIBLE_SERIES: &str = "NOT EXISTS (SELECT 1 FROM tournament_series ts JOIN tournaments t
    ON t.id = ts.tournament_id WHERE ts.series_id = s.id AND t.revealed_at IS NULL)";

/// Who may enter, resolved to a team list at lock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Field {
    /// Every team with a live submission.
    All,
    /// The top `n` of the ladder.
    Top { n: u32 },
    /// These teams (ids or names on input, ids once stored).
    List { team_ids: Vec<String> },
}

pub struct NewTournament {
    pub name: String,
    pub format: Format,
    pub field: Field,
    pub maps: Vec<String>,
}

fn format_name(f: Format) -> &'static str {
    match f {
        Format::Single => "single",
        Format::Double => "double",
    }
}

pub fn parse_format(s: &str) -> Option<Format> {
    match s {
        "single" => Some(Format::Single),
        "double" => Some(Format::Double),
        _ => None,
    }
}

struct Tournament {
    id: String,
    name: String,
    format: Format,
    field: Field,
    maps: Vec<String>,
    /// SHA-256 of each map, fixed at lock.
    map_shas: Option<Vec<String>>,
    status: String,
    excluded: Value,
    winner: Option<String>,
    times: [Option<i64>; 5],
}

fn from_json<T: serde::de::DeserializeOwned>(text: &str) -> ApiResult<T> {
    serde_json::from_str(text).map_err(ApiError::internal)
}

fn load(tx: &Transaction, id: &str) -> ApiResult<Tournament> {
    // JSON columns are read as text here and parsed below, where a parse error can be an `ApiError`.
    let mut text: [Option<String>; 5] = Default::default();
    let mut t = tx
        .query_row(
            "SELECT id, name, status, winner, created_at, locked_at, started_at, finished_at, revealed_at,
                    format, field, maps, map_shas, excluded
             FROM tournaments WHERE id = ?1",
            [id],
            |r| {
                for (i, slot) in text.iter_mut().enumerate() {
                    *slot = r.get(9 + i)?;
                }
                Ok(Tournament {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    format: Format::Single,
                    field: Field::All,
                    maps: Vec::new(),
                    map_shas: None,
                    status: r.get(2)?,
                    excluded: json!([]),
                    winner: r.get(3)?,
                    times: [r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?],
                })
            },
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found(format!("no tournament {id:?}")))?;
    let [format, field, maps, shas, excluded] = text;
    t.format = format
        .as_deref()
        .and_then(parse_format)
        .ok_or_else(|| ApiError::internal("bad tournament format"))?;
    t.field = from_json(field.as_deref().unwrap_or("null"))?;
    t.maps = from_json(maps.as_deref().unwrap_or("null"))?;
    t.map_shas = shas.as_deref().map(from_json).transpose()?;
    if let Some(excluded) = excluded {
        t.excluded = from_json(&excluded)?;
    }
    Ok(t)
}

fn wrong_status(t: &Tournament, wanted: &str) -> ApiError {
    ApiError::conflict(
        "wrong_status",
        format!("tournament {:?} is {}, not {wanted}", t.id, t.status),
    )
}

// ---- organiser actions ----

/// Store a draft. Every map must exist (upload hidden maps with `?in_pool=false`), the map count must be
/// odd, and a team list is resolved to ids now.
pub fn create(tx: &Transaction, req: NewTournament, now: i64) -> ApiResult<String> {
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::bad_request("name must be 1..=80 characters"));
    }
    if req.maps.len().is_multiple_of(2) {
        return Err(ApiError::bad_request(
            "a tournament needs an odd number of maps (at least one), so a pairing cannot split them evenly",
        ));
    }
    let mut seen = BTreeSet::new();
    for map in &req.maps {
        if !seen.insert(map) {
            return Err(ApiError::bad_request(format!(
                "map {map:?} is listed twice"
            )));
        }
        map_sha(tx, map)?;
    }
    let field = match req.field {
        Field::Top { n } if n < 2 => {
            return Err(ApiError::bad_request("field.n must be at least 2"));
        }
        Field::List { team_ids } => {
            let mut ids = Vec::new();
            for t in &team_ids {
                let id = find_team(tx, t)?.id;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            if ids.len() < 2 {
                return Err(ApiError::bad_request(
                    "field.team_ids needs at least two teams",
                ));
            }
            Field::List { team_ids: ids }
        }
        f => f,
    };
    let id = format!("tn{}", random_hex(8));
    tx.execute(
        "INSERT INTO tournaments(id, name, format, field, maps, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6)",
        params![
            id,
            name,
            format_name(req.format),
            serde_json::to_string(&field).map_err(ApiError::internal)?,
            serde_json::to_string(&req.maps).map_err(ApiError::internal)?,
            now
        ],
    )?;
    Ok(id)
}

fn map_sha(tx: &Transaction, name: &str) -> ApiResult<String> {
    tx.query_row("SELECT sha256 FROM maps WHERE name = ?1", [name], |r| {
        r.get(0)
    })
    .optional()?
    .ok_or_else(|| ApiError::bad_request(format!("no map {name:?} (upload it first)")))
}

/// A team on the ladder with its live submission, in seed order.
struct Seeded {
    team_id: String,
    submission_id: String,
    rating: i64,
    provisional: bool,
}

/// Every team with a live submission, in seed order: rated teams by displayed rating, then provisional
/// teams by displayed rating (ties by name, as on the ladder).
fn seeding(tx: &Transaction, hook: &dyn RatingHook) -> ApiResult<Vec<Seeded>> {
    let mut rows: Vec<Seeded> = ratings::ladder_rows(tx, hook)?
        .into_iter()
        .map(|row| Seeded {
            rating: hook.display(&row.standing.rating).round() as i64,
            provisional: hook.is_provisional(&row.standing.rating),
            team_id: row.team_id,
            submission_id: row.submission,
        })
        .collect();
    rows.sort_by_key(|s| s.provisional);
    Ok(rows)
}

/// Freeze the field: resolve it to teams, snapshot each entrant's live submission and seed, fix the map
/// hashes, and pause ranked matchmaking for `pause_ms`. Teams of the field without a live submission, and
/// teams under a moderation sanction, are left out and listed in `excluded`.
pub fn lock(
    tx: &Transaction,
    hook: &dyn RatingHook,
    id: &str,
    pause_ms: i64,
    now: i64,
) -> ApiResult<()> {
    let t = load(tx, id)?;
    if t.status != "draft" {
        return Err(wrong_status(&t, "draft"));
    }
    let shas = t
        .maps
        .iter()
        .map(|m| map_sha(tx, m))
        .collect::<ApiResult<Vec<_>>>()?;
    // Teams under a suspension or ban (moderation) are not entered; they are listed in `excluded`.
    let blocked = super::moderation::blocked_teams(tx, now)?;
    let ladder: Vec<Seeded> = seeding(tx, hook)?
        .into_iter()
        .filter(|s| !blocked.contains(&s.team_id))
        .collect();
    let on_ladder: BTreeSet<&str> = ladder.iter().map(|s| s.team_id.as_str()).collect();
    // The teams the field names explicitly; the top N is taken from the ladder, which only holds teams
    // with a live submission.
    let named: Vec<String> = match &t.field {
        Field::All => all_team_ids(tx)?,
        Field::Top { .. } => Vec::new(),
        Field::List { team_ids } => team_ids.clone(),
    };
    let mut entrants: Vec<&Seeded> = ladder
        .iter()
        .filter(|s| !matches!(t.field, Field::List { .. }) || named.contains(&s.team_id))
        .collect();
    if let Field::Top { n } = t.field {
        entrants.truncate(n as usize);
    }
    if entrants.len() < 2 {
        return Err(ApiError::conflict(
            "not_enough_entrants",
            format!(
                "{} team(s) of the field have a live submission; a bracket needs two",
                entrants.len()
            ),
        ));
    }
    for (seed, e) in entrants.iter().enumerate() {
        tx.execute(
            "INSERT INTO tournament_entrants(tournament_id, seed, team_id, submission_id, rating, provisional)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, seed as i64 + 1, e.team_id, e.submission_id, e.rating, e.provisional],
        )?;
    }
    let mut excluded = Vec::new();
    for team_id in named.iter().filter(|id| !on_ladder.contains(id.as_str())) {
        let team = find_team(tx, team_id)?;
        let reason = if blocked.contains(team_id) {
            "sanctioned"
        } else {
            "no live submission"
        };
        excluded.push(json!({"team_id": team.id, "name": team.name, "reason": reason}));
    }
    tx.execute(
        "UPDATE tournaments SET status = 'locked', locked_at = ?2, map_shas = ?3, excluded = ?4 WHERE id = ?1",
        params![id, now, json!(shas).to_string(), json!(excluded).to_string()],
    )?;
    let until = now.saturating_add(pause_ms.max(0));
    if ranked_paused(tx, now)?.is_none_or(|current| current < until) {
        set_setting(tx, RANKED_PAUSED_UNTIL, &until.to_string())?;
    }
    Ok(())
}

fn all_team_ids(tx: &Transaction) -> ApiResult<Vec<String>> {
    let mut st = tx.prepare("SELECT id FROM teams ORDER BY name")?;
    let rows = st.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Build the bracket and queue the first pairings.
pub fn start(tx: &Transaction, id: &str, now: i64) -> ApiResult<()> {
    let t = load(tx, id)?;
    if t.status != "locked" {
        return Err(wrong_status(&t, "locked"));
    }
    if current_patch(tx)?.is_none() {
        return Err(ApiError::conflict("no_patch", "no current patch is set"));
    }
    let entrants = load_entrants(tx, id)?;
    let shape = bracket::build(t.format, entrants.len() as u32);
    for idx in 0..shape.len() {
        tx.execute(
            "INSERT INTO tournament_matches(tournament_id, idx) VALUES (?1, ?2)",
            params![id, idx as i64],
        )?;
    }
    tx.execute(
        "UPDATE tournaments SET status = 'running', started_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    advance(tx, &t, now)
}

/// Make the maps public on the tournament and add them to the ranked pool.
pub fn reveal(tx: &Transaction, id: &str, now: i64) -> ApiResult<()> {
    let t = load(tx, id)?;
    if t.times[4].is_some() {
        return Err(ApiError::conflict(
            "already_revealed",
            format!("the maps of tournament {id:?} are already public"),
        ));
    }
    for map in &t.maps {
        tx.execute("UPDATE maps SET in_pool = 1 WHERE name = ?1", [map])?;
    }
    tx.execute(
        "UPDATE tournaments SET revealed_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    Ok(())
}

/// Lift the ranked pause at once.
pub fn resume_ranked(tx: &Transaction) -> ApiResult<()> {
    tx.execute("DELETE FROM settings WHERE key = ?1", [RANKED_PAUSED_UNTIL])?;
    Ok(())
}

/// `Some(until)` while ranked matchmaking is paused by a tournament lock.
pub fn ranked_paused(tx: &Transaction, now: i64) -> ApiResult<Option<i64>> {
    Ok(setting(tx, RANKED_PAUSED_UNTIL)?
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|&until| until > now))
}

// ---- the bracket ----

struct Entrant {
    team_id: String,
    name: String,
    submission_id: String,
    rating: Option<i64>,
    provisional: bool,
}

/// Entrants in seed order (index = 0-based seed).
fn load_entrants(tx: &Transaction, id: &str) -> ApiResult<Vec<Entrant>> {
    let mut st = tx.prepare_cached(
        "SELECT e.team_id, t.name, e.submission_id, e.rating, e.provisional FROM tournament_entrants e
         JOIN teams t ON t.id = e.team_id WHERE e.tournament_id = ?1 ORDER BY e.seed",
    )?;
    let rows = st.query_map([id], |r| {
        Ok(Entrant {
            team_id: r.get(0)?,
            name: r.get(1)?,
            submission_id: r.get(2)?,
            rating: r.get(3)?,
            provisional: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The stored state of one bracket match.
#[derive(Default)]
struct MatchRow {
    series_id: Option<String>,
    map_halves: Option<[u32; 2]>,
    rounds: Option<[u32; 2]>,
    winner: Option<Side>,
    decided_by: Option<String>,
}

fn load_matches(tx: &Transaction, id: &str) -> ApiResult<Vec<MatchRow>> {
    let mut st = tx.prepare_cached(
        "SELECT series_id, map_halves_a, map_halves_b, rounds_a, rounds_b, winner_side, decided_by
         FROM tournament_matches WHERE tournament_id = ?1 ORDER BY idx",
    )?;
    let rows = st.query_map([id], |r| {
        let pair = |i: usize| -> rusqlite::Result<Option<[u32; 2]>> {
            Ok(
                match (r.get::<_, Option<u32>>(i)?, r.get::<_, Option<u32>>(i + 1)?) {
                    (Some(a), Some(b)) => Some([a, b]),
                    _ => None,
                },
            )
        };
        Ok(MatchRow {
            series_id: r.get(0)?,
            map_halves: pair(1)?,
            rounds: pair(3)?,
            winner: r
                .get::<_, Option<i64>>(5)?
                .map(|s| if s == 0 { Side::A } else { Side::B }),
            decided_by: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The bracket with the results so far.
fn bracket_state(
    t: &Tournament,
    entrants: usize,
    rows: &[MatchRow],
) -> (Vec<BracketMatch>, Vec<MatchState>) {
    let shape = bracket::build(t.format, entrants as u32);
    let results: Vec<Option<Side>> = rows.iter().map(|r| r.winner).collect();
    let states = bracket::resolve(&shape, &results);
    (shape, states)
}

/// Queue every pairing whose two seats are known and that has no series yet (the bracket reset only once
/// the losers champion has won the grand final); close the tournament once its champion is decided.
fn advance(tx: &Transaction, t: &Tournament, now: i64) -> ApiResult<()> {
    // A finished tournament keeps its result, even if its bracket would now grow (a reset under newer rules).
    if t.status == "finished" {
        return Ok(());
    }
    let entrants = load_entrants(tx, &t.id)?;
    let rows = load_matches(tx, &t.id)?;
    let (_, states) = bracket_state(t, entrants.len(), &rows);
    for (idx, state) in states.iter().enumerate() {
        // A bracket started before the reset was part of the shape has no row for it yet.
        if !state.playable() || rows.get(idx).is_some_and(|r| r.series_id.is_some()) {
            continue;
        }
        let [Seat::Team(a), Seat::Team(b)] = state.seats else {
            continue;
        };
        let pairing = Pairing {
            a: (
                &entrants[a as usize].team_id,
                &entrants[a as usize].submission_id,
            ),
            b: (
                &entrants[b as usize].team_id,
                &entrants[b as usize].submission_id,
            ),
        };
        let series = queue_pairing(tx, t, idx as i64, &pairing, now)?;
        tx.execute(
            "INSERT INTO tournament_matches(tournament_id, idx, series_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(tournament_id, idx) DO UPDATE SET series_id = excluded.series_id",
            params![t.id, idx as i64, series],
        )?;
    }
    if let Seat::Team(champion) = bracket::champion(&states) {
        tx.execute(
            "UPDATE tournaments SET status = 'finished', finished_at = ?2, winner = ?3 WHERE id = ?1",
            params![t.id, now, entrants[champion as usize].team_id],
        )?;
    }
    Ok(())
}

/// Two seats of a pairing: (team id, locked submission id).
struct Pairing<'a> {
    a: (&'a str, &'a str),
    b: (&'a str, &'a str),
}

/// Queue one pairing as a `tournament` series: one game per tournament map, in map order, with the locked
/// submissions; engine slots alternate from a coin flip on the series id, seeds as in ranked series.
fn queue_pairing(
    tx: &Transaction,
    t: &Tournament,
    idx: i64,
    p: &Pairing,
    now: i64,
) -> ApiResult<String> {
    let patch = current_patch(tx)?
        .ok_or_else(|| ApiError::conflict("no_patch", "no current patch is set"))?;
    let shas = t
        .map_shas
        .as_ref()
        .ok_or_else(|| ApiError::internal("tournament started without map hashes"))?;
    let bot = |sub: &str| -> ApiResult<BotRef> {
        Ok(tx.query_row(
            "SELECT sha256, fuel_mult FROM submissions WHERE id = ?1",
            [sub],
            |r| {
                Ok(BotRef {
                    sha256: r.get(0)?,
                    fuel_mult: r.get(1)?,
                })
            },
        )?)
    };
    let (bot_a, bot_b) = (bot(p.a.1)?, bot(p.b.1)?);
    let number = u64::from_str_radix(&random_hex(8), 16).expect("16 hex digits");
    let id = format!("s{number:016x}");
    tx.execute(
        "INSERT INTO series(id, kind, team_a, team_b, sub_a, sub_b, games, state, created_at)
         VALUES (?1, 'tournament', ?2, ?3, ?4, ?5, ?6, 'running', ?7)",
        params![id, p.a.0, p.b.0, p.a.1, p.b.1, t.maps.len() as i64, now],
    )?;
    tx.execute(
        "INSERT INTO tournament_series(series_id, tournament_id, idx) VALUES (?1, ?2, ?3)",
        params![id, t.id, idx],
    )?;
    let a_first = mix64(number) & 1 == 0;
    for (i, (map_name, map_sha)) in t.maps.iter().zip(shas).enumerate() {
        let swapped = a_first != (i % 2 == 0);
        let (first, second) = if swapped {
            (&bot_b, &bot_a)
        } else {
            (&bot_a, &bot_b)
        };
        let seed = ibc_rating::game_seed(number, i as u32);
        let spec = JobSpec {
            patch: patch.clone(),
            bot_a: first.clone(),
            bot_b: second.clone(),
            map: MapRef {
                sha256: map_sha.clone(),
            },
            seed,
            max_ticks: None,
        }
        .normalize()
        .map_err(ApiError::internal)?;
        let Enqueued { job_id, .. } = jobs::enqueue(tx, Class::Tournament, &spec, now)?;
        tx.execute(
            "INSERT INTO games(id, series_id, idx, job_id, map_name, seed, swapped, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                format!("{id}-{i}"),
                id,
                i as i64,
                job_id,
                map_name,
                seed.to_string(),
                swapped,
                now
            ],
        )?;
    }
    Ok(id)
}

/// Series one pairing may use before it is decided on the games that finished, when games keep failing to
/// run (dead-lettered: timeouts on two machines, missing blobs). Patch changes do not count against it.
const MAX_PAIRING_SERIES: i64 = 3;

/// Called with the series `league::finalize_ready` just closed: decide the bracket matches they belong to
/// and queue what follows. A pairing that lost games to a patch change is played again, whole, on the
/// current patch (tournament series are never voided by a patch); one with games that could not be run is
/// played again, whole, until it has used `MAX_PAIRING_SERIES` series.
pub fn on_series_closed(tx: &Transaction, series: &[String], now: i64) -> ApiResult<()> {
    for series_id in series {
        let found: Option<(String, i64)> = tx
            .query_row(
                "SELECT tournament_id, idx FROM tournament_matches WHERE series_id = ?1 AND winner_side IS NULL",
                [series_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((tournament_id, idx)) = found else {
            continue;
        };
        let t = load(tx, &tournament_id)?;
        let games = series_games(tx, series_id)?;
        let (team_a, team_b, sub_a, sub_b): (String, String, String, String) = tx.query_row(
            "SELECT team_a, team_b, sub_a, sub_b FROM series WHERE id = ?1",
            [series_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        if needs_replay(tx, &tournament_id, idx, &games)? {
            let pairing = Pairing {
                a: (&team_a, &sub_a),
                b: (&team_b, &sub_b),
            };
            let replay = queue_pairing(tx, &t, idx, &pairing, now)?;
            tx.execute(
                "UPDATE tournament_matches SET series_id = ?3 WHERE tournament_id = ?1 AND idx = ?2",
                params![tournament_id, idx, replay],
            )?;
            continue;
        }
        let seed = |team: &str| -> ApiResult<u32> {
            Ok(tx.query_row(
                "SELECT seed FROM tournament_entrants WHERE tournament_id = ?1 AND team_id = ?2",
                params![tournament_id, team],
                |r| r.get(0),
            )?)
        };
        let maps: Vec<MapResult> = games.iter().filter_map(|g| g.result).collect();
        let r = bracket::decide(&maps, [seed(&team_a)?, seed(&team_b)?]);
        tx.execute(
            "UPDATE tournament_matches SET map_halves_a = ?3, map_halves_b = ?4, rounds_a = ?5, rounds_b = ?6,
               winner_side = ?7, decided_by = ?8 WHERE tournament_id = ?1 AND idx = ?2",
            params![
                tournament_id,
                idx,
                r.map_halves[0],
                r.map_halves[1],
                r.rounds_won[0],
                r.rounds_won[1],
                i64::from(r.winner == Side::B),
                r.decided_by.name()
            ],
        )?;
        advance(tx, &t, now)?;
    }
    Ok(())
}

/// Whether a closed pairing series is played again instead of decided.
fn needs_replay(
    tx: &Transaction,
    tournament_id: &str,
    idx: i64,
    games: &[SeriesGame],
) -> ApiResult<bool> {
    if games.iter().any(|g| g.superseded) {
        return Ok(true);
    }
    if !games.iter().any(|g| g.dead) {
        return Ok(false);
    }
    let used: i64 = tx.query_row(
        "SELECT COUNT(*) FROM tournament_series WHERE tournament_id = ?1 AND idx = ?2",
        params![tournament_id, idx],
        |r| r.get(0),
    )?;
    Ok(used < MAX_PAIRING_SERIES)
}

struct SeriesGame {
    /// From the series' team A; `None` if the game was not played.
    result: Option<MapResult>,
    /// Dead-lettered: it could not be run.
    dead: bool,
    /// Dead-lettered by a patch change.
    superseded: bool,
}

fn series_games(tx: &Transaction, series_id: &str) -> ApiResult<Vec<SeriesGame>> {
    let mut st = tx.prepare_cached(
        "SELECT g.swapped, j.state, j.result, j.error FROM games g JOIN jobs j ON j.id = g.job_id
         WHERE g.series_id = ?1 ORDER BY g.idx",
    )?;
    let rows = st.query_map([series_id], |r| {
        Ok((
            r.get::<_, bool>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (swapped, state, result, error) = row?;
        let result = (state == "done")
            .then(|| result.and_then(|s| serde_json::from_str::<Value>(&s).ok()))
            .flatten();
        out.push(SeriesGame {
            result: result.map(|v| map_result(&v, swapped)),
            dead: state == "dead",
            superseded: state == "dead" && error.as_deref() == Some(PATCH_SUPERSEDED),
        });
    }
    Ok(out)
}

/// One finished game from the series' team A (`swapped`: team A played in engine slot B). `score` is
/// rounds won by engine slot; an unfinished game keeps its rounds but has no outcome.
fn map_result(result: &Value, swapped: bool) -> MapResult {
    let score = |i: usize| {
        result
            .get("score")
            .and_then(|s| s.get(i))
            .and_then(Value::as_u64)
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    let (slot_a, slot_b) = (score(0), score(1));
    let outcome = slot_winner(result).map(|w| match (w, swapped) {
        (Winner::Draw, _) => Outcome::Draw,
        (Winner::SlotA, false) | (Winner::SlotB, true) => Outcome::AWins,
        (Winner::SlotA, true) | (Winner::SlotB, false) => Outcome::BWins,
    });
    MapResult {
        outcome,
        rounds: if swapped {
            [slot_b, slot_a]
        } else {
            [slot_a, slot_b]
        },
    }
}
