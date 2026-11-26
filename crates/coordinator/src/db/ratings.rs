//! Ratings, their event ledger and the ladder.
//!
//! `rating_events` is the history every rating is a fold of: one `series` event per rated series (in the
//! order the series were rated; `outcome` lists its map results in game order, e.g. `a,draw,b`) and one
//! `submission` event per change of a team's live submission. A ledger written before ratings moved per
//! series holds `game` events instead, one map each; they replay as one-map series. The `ratings` table
//! holds the result, computed once, as each event happens, and stored: ratings are `f64` and may differ in
//! the last bit across platforms, so they are never recomputed on read. They are recomputed from the ledger,
//! here, in two cases only: a quarantine invalidated results that were already rated (their events are
//! dropped first), and the configured rating system changed.

use super::{set_setting, setting};
use crate::config::Config;
use crate::error::ApiResult;
use crate::rating::{Outcome, Rating, RatingHook};
use crate::util::rfc3339;
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A team's stored rating with its game record.
#[derive(Clone, Copy, Debug)]
pub struct Standing {
    pub rating: Rating,
    /// Wins, losses, draws over rated games (maps).
    pub wld: (i64, i64, i64),
    pub last_played: Option<i64>,
}

impl Standing {
    fn new(hook: &dyn RatingHook) -> Standing {
        Standing {
            rating: hook.initial(),
            wld: (0, 0, 0),
            last_played: None,
        }
    }

    /// Count the rated maps of a series in this team's record; `as_a`: it was the series' team A, from
    /// whose side the outcomes are.
    fn record(&mut self, outcomes: &[Outcome], as_a: bool, at: i64) {
        for &outcome in outcomes {
            let win = matches!(
                (outcome, as_a),
                (Outcome::AWins, true) | (Outcome::BWins, false)
            );
            match outcome {
                Outcome::Draw => self.wld.2 += 1,
                _ if win => self.wld.0 += 1,
                _ => self.wld.1 += 1,
            }
        }
        self.last_played = Some(at);
    }
}

/// Rate one series between two standings: one update from the pre-series ratings.
fn apply_series(
    hook: &dyn RatingHook,
    a: &mut Standing,
    b: &mut Standing,
    outcomes: &[Outcome],
    at: i64,
) {
    (a.rating, b.rating) = hook.update_series(&a.rating, &b.rating, outcomes);
    a.record(outcomes, true, at);
    b.record(outcomes, false, at);
}

pub fn outcome_code(o: Outcome) -> &'static str {
    match o {
        Outcome::AWins => "a",
        Outcome::BWins => "b",
        Outcome::Draw => "draw",
    }
}

fn parse_outcome(code: &str) -> Option<Outcome> {
    match code {
        "a" => Some(Outcome::AWins),
        "b" => Some(Outcome::BWins),
        "draw" => Some(Outcome::Draw),
        _ => None,
    }
}

/// The `outcome` column of a rating event: map results in game order, comma-separated.
fn encode_outcomes(outcomes: &[Outcome]) -> String {
    outcomes
        .iter()
        .map(|&o| outcome_code(o))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_outcomes(text: &str) -> Option<Vec<Outcome>> {
    text.split(',').map(parse_outcome).collect()
}

/// Rated maps recorded by a `game` or `series` event (0 for anything else).
pub fn event_games(kind: &str, outcome: Option<&str>) -> u32 {
    match (kind, outcome.and_then(parse_outcomes)) {
        ("game" | "series", Some(o)) => o.len() as u32,
        _ => 0,
    }
}

pub fn load(tx: &Transaction, hook: &dyn RatingHook, team_id: &str) -> ApiResult<Standing> {
    Ok(tx
        .query_row(
            "SELECT mu, sigma, games, wins, losses, draws, last_played FROM ratings WHERE team_id = ?1",
            [team_id],
            |r| {
                Ok(Standing {
                    rating: Rating {
                        mu: r.get(0)?,
                        sigma: r.get(1)?,
                        games: r.get(2)?,
                    },
                    wld: (r.get(3)?, r.get(4)?, r.get(5)?),
                    last_played: r.get(6)?,
                })
            },
        )
        .optional()?
        .unwrap_or_else(|| Standing::new(hook)))
}

fn save(tx: &Transaction, team_id: &str, s: &Standing, now: i64) -> ApiResult<()> {
    let r = &s.rating;
    tx.execute(
        "INSERT INTO ratings(team_id, mu, sigma, games, wins, losses, draws, updated_at, last_played)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(team_id) DO UPDATE SET mu = excluded.mu, sigma = excluded.sigma, games = excluded.games,
           wins = excluded.wins, losses = excluded.losses, draws = excluded.draws,
           updated_at = excluded.updated_at, last_played = excluded.last_played",
        params![team_id, r.mu, r.sigma, r.games, s.wld.0, s.wld.1, s.wld.2, now, s.last_played],
    )?;
    Ok(())
}

/// A finished ranked series: the results of its finished maps, in game order, seen from team A.
pub struct RatedSeries<'a> {
    pub series_id: &'a str,
    pub team_a: &'a str,
    pub team_b: &'a str,
    pub outcomes: &'a [Outcome],
}

/// Rate a series once and record it in the ledger. A series without a rated map changes nothing.
pub fn rate_series(
    tx: &Transaction,
    hook: &dyn RatingHook,
    s: &RatedSeries,
    now: i64,
) -> ApiResult<()> {
    if s.outcomes.is_empty() {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO rating_events(kind, team_a, team_b, series_id, outcome, created_at)
         VALUES ('series', ?1, ?2, ?3, ?4, ?5)",
        params![
            s.team_a,
            s.team_b,
            s.series_id,
            encode_outcomes(s.outcomes),
            now
        ],
    )?;
    let (mut a, mut b) = (load(tx, hook, s.team_a)?, load(tx, hook, s.team_b)?);
    apply_series(hook, &mut a, &mut b, s.outcomes, now);
    save(tx, s.team_a, &a, now)?;
    save(tx, s.team_b, &b, now)
}

/// The team's live submission was replaced: widen its uncertainty, and record that in the ledger.
pub fn submission_changed(
    tx: &Transaction,
    hook: &dyn RatingHook,
    team_id: &str,
    now: i64,
) -> ApiResult<()> {
    let mut s = load(tx, hook, team_id)?;
    let widened = hook.on_new_submission(&s.rating);
    // Already as uncertain as a change makes it (e.g. switching back and forth): nothing to record, so the
    // ledger cannot be grown by toggling the live submission.
    if widened.mu == s.rating.mu && widened.sigma == s.rating.sigma {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO rating_events(kind, team_a, created_at) VALUES ('submission', ?1, ?2)",
        params![team_id, now],
    )?;
    s.rating = widened;
    save(tx, team_id, &s, now)
}

/// Drop the rating events of these series (their results were invalidated). Call `recompute` afterwards.
pub fn forget_series(tx: &Transaction, series_ids: &[String]) -> ApiResult<usize> {
    let mut st = tx.prepare_cached(
        "DELETE FROM rating_events WHERE kind IN ('game', 'series') AND series_id = ?1",
    )?;
    let mut n = 0;
    for id in series_ids {
        n += st.execute([id])?;
    }
    Ok(n)
}

/// A row of `rating_events`.
struct LedgerEvent {
    kind: String,
    team_a: String,
    team_b: Option<String>,
    outcome: Option<String>,
    at: i64,
}

/// Rebuild every rating from the ledger, in ledger order. Returns the number of events replayed.
pub fn recompute(tx: &Transaction, hook: &dyn RatingHook, now: i64) -> ApiResult<usize> {
    let mut teams: BTreeMap<String, Standing> = BTreeMap::new();
    let events: Vec<LedgerEvent> = {
        let mut st = tx.prepare(
            "SELECT kind, team_a, team_b, outcome, created_at FROM rating_events ORDER BY seq",
        )?;
        let rows = st.query_map([], |r| {
            Ok(LedgerEvent {
                kind: r.get(0)?,
                team_a: r.get(1)?,
                team_b: r.get(2)?,
                outcome: r.get(3)?,
                at: r.get(4)?,
            })
        })?;
        rows.collect::<Result<_, _>>()?
    };
    for e in &events {
        let mut standing = |t: &str| *teams.entry(t.to_string()).or_insert(Standing::new(hook));
        match (
            e.kind.as_str(),
            &e.team_b,
            e.outcome.as_deref().and_then(parse_outcomes),
        ) {
            ("game" | "series", Some(team_b), Some(outcomes)) => {
                let (mut a, mut b) = (standing(&e.team_a), standing(team_b));
                apply_series(hook, &mut a, &mut b, &outcomes, e.at);
                teams.insert(e.team_a.clone(), a);
                teams.insert(team_b.clone(), b);
            }
            ("submission", _, _) => {
                let mut s = standing(&e.team_a);
                s.rating = hook.on_new_submission(&s.rating);
                teams.insert(e.team_a.clone(), s);
            }
            _ => {}
        }
    }
    tx.execute("DELETE FROM ratings", [])?;
    for (team, s) in &teams {
        save(tx, team, s, now)?;
    }
    Ok(events.len())
}

/// Record which system produced the stored ratings. If the configured one differs from the one that
/// produced them, recompute everything with the configured one (and write a snapshot). Returns true then.
pub fn adopt_system(
    tx: &Transaction,
    hook: &dyn RatingHook,
    now: i64,
    snapshots_kept: i64,
) -> ApiResult<bool> {
    let stored = setting(tx, "rating_system")?;
    set_setting(tx, "rating_system", hook.name())?;
    match stored {
        Some(name) if name != hook.name() => {
            let n = recompute(tx, hook, now)?;
            eprintln!(
                "rating system changed from {name} to {}: {n} ledger events replayed",
                hook.name()
            );
            write_snapshot(tx, hook, now, snapshots_kept)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

// ---- ladder ----

pub struct LadderRow {
    pub team_id: String,
    pub name: String,
    pub standing: Standing,
    /// Rated series played.
    pub series: i64,
}

/// Rated series per team.
fn rated_series(tx: &Transaction) -> ApiResult<BTreeMap<String, i64>> {
    let mut st = tx.prepare_cached(
        "SELECT team, COUNT(*) FROM (SELECT team_a AS team FROM series WHERE rated = 1
                                     UNION ALL SELECT team_b FROM series WHERE rated = 1) GROUP BY team",
    )?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Teams with a live submission and their ratings, best displayed rating first.
pub fn ladder_rows(tx: &Transaction, hook: &dyn RatingHook) -> ApiResult<Vec<LadderRow>> {
    let mut st = tx.prepare(
        "SELECT t.id, t.name FROM teams t JOIN submissions s ON s.team_id = t.id AND s.live = 1",
    )?;
    let teams: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let series = rated_series(tx)?;
    let mut rows = Vec::with_capacity(teams.len());
    for (team_id, name) in teams {
        let standing = load(tx, hook, &team_id)?;
        rows.push(LadderRow {
            series: series.get(&team_id).copied().unwrap_or(0),
            team_id,
            name,
            standing,
        });
    }
    rows.sort_by(|a, b| {
        hook.display(&b.standing.rating)
            .total_cmp(&hook.display(&a.standing.rating))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(rows)
}

/// The public ladder (`GET /v1/ladder`): rated teams by displayed rating, then the provisional ones (fewer
/// than `cfg.provisional_series` rated series), which show no rating and no tier. Nothing here identifies
/// a submission.
pub fn public_ladder(
    tx: &Transaction,
    hook: &dyn RatingHook,
    cfg: &Config,
) -> ApiResult<Vec<Value>> {
    let (rated, provisional): (Vec<LadderRow>, Vec<LadderRow>) = ladder_rows(tx, hook)?
        .into_iter()
        .partition(|row| row.series >= cfg.provisional_series);
    let rows = rated.iter().map(|row| (row, false));
    let rows = rows.chain(provisional.iter().map(|row| (row, true)));
    Ok(rows
        .enumerate()
        .map(|(i, (row, provisional))| {
            let (r, wld) = (&row.standing.rating, row.standing.wld);
            let shown = hook.display(r);
            json!({"rank": i + 1, "team_id": row.team_id, "team_name": row.name,
                   "rating": (!provisional).then(|| shown.round() as i64),
                   "tier": (!provisional).then(|| cfg.tier(shown)),
                   "provisional": provisional, "games": r.games, "series": row.series,
                   "wins": wld.0, "losses": wld.1, "draws": wld.2})
        })
        .collect())
}

/// Write the ladder as it is now, for the organisers' history (it is not served: `public_ladder` is
/// computed on read). `rating` is the displayed number, rounded; `provisional` is the rating system's own
/// flag; `mu`/`sigma` are the raw estimate of the system named in `system`.
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
            let (r, wld) = (&row.standing.rating, row.standing.wld);
            json!({"rank": i + 1, "team_id": row.team_id, "name": row.name,
                   "rating": hook.display(r).round() as i64, "provisional": hook.is_provisional(r),
                   "mu": r.mu, "sigma": r.sigma, "games": r.games, "series": row.series,
                   "wins": wld.0, "losses": wld.1, "draws": wld.2})
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
