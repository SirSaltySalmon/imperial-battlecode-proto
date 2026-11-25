//! Ratings, their event ledger and the ladder.
//!
//! `rating_events` is the history every rating is a fold of: one `game` event per rated game (in the order
//! the games were rated), one `submission` event per change of a team's live submission and one `balance`
//! event per team whose uncertainty a balance patch widened (`outcome` holds the factor). The `ratings`
//! table holds the result, computed once, as each event happens, and stored: ratings are `f64` and may
//! differ in the last bit across platforms, so they are never recomputed on read. They are recomputed from
//! the ledger, here, in two cases only: a quarantine invalidated results that were already rated (their
//! events are dropped first), and the configured rating system changed.

use super::{set_setting, setting};
use crate::error::ApiResult;
use crate::rating::{Outcome, Rating, RatingHook};
use crate::util::rfc3339;
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::json;
use std::collections::BTreeMap;

/// A team's stored rating with its game record.
#[derive(Clone, Copy, Debug)]
pub struct Standing {
    pub rating: Rating,
    /// Wins, losses, draws over rated games.
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

    /// Record one rated game of this team; `as_a`: it was the series' team A, from whose side `outcome` is.
    fn played(&mut self, rating: Rating, outcome: Outcome, as_a: bool, at: i64) {
        self.rating = rating;
        let win = matches!(
            (outcome, as_a),
            (Outcome::AWins, true) | (Outcome::BWins, false)
        );
        match outcome {
            Outcome::Draw => self.wld.2 += 1,
            _ if win => self.wld.0 += 1,
            _ => self.wld.1 += 1,
        }
        self.last_played = Some(at);
    }
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

/// One finished game of a ranked series, `outcome` seen from the series' team A.
pub struct RatedGame<'a> {
    pub series_id: &'a str,
    pub game_idx: i64,
    pub team_a: &'a str,
    pub team_b: &'a str,
    pub outcome: Outcome,
}

/// Rate one game and record it in the ledger.
pub fn rate_game(
    tx: &Transaction,
    hook: &dyn RatingHook,
    g: &RatedGame,
    now: i64,
) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO rating_events(kind, team_a, team_b, series_id, game_idx, outcome, created_at)
         VALUES ('game', ?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            g.team_a,
            g.team_b,
            g.series_id,
            g.game_idx,
            outcome_code(g.outcome),
            now
        ],
    )?;
    let (mut a, mut b) = (load(tx, hook, g.team_a)?, load(tx, hook, g.team_b)?);
    let (ra, rb) = hook.update(&a.rating, &b.rating, g.outcome);
    a.played(ra, g.outcome, true, now);
    b.played(rb, g.outcome, false, now);
    save(tx, g.team_a, &a, now)?;
    save(tx, g.team_b, &b, now)
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

/// A balance patch went live: widen the uncertainty of every rated team by `factor`
/// (`RatingSystem::on_balance_patch`) and record one `balance` event per team that changed. Returns how
/// many teams changed.
pub fn balance_patch(
    tx: &Transaction,
    hook: &dyn RatingHook,
    factor: f64,
    now: i64,
) -> ApiResult<usize> {
    let teams: Vec<String> = {
        let mut st = tx.prepare("SELECT team_id FROM ratings ORDER BY team_id")?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut changed = 0;
    for team_id in &teams {
        let mut s = load(tx, hook, team_id)?;
        let widened = hook.on_balance_patch(&s.rating, factor);
        if widened.mu == s.rating.mu && widened.sigma == s.rating.sigma {
            continue;
        }
        // `f64`'s `Display` is the shortest text that parses back to the same value.
        tx.execute(
            "INSERT INTO rating_events(kind, team_a, outcome, created_at) VALUES ('balance', ?1, ?2, ?3)",
            params![team_id, factor.to_string(), now],
        )?;
        s.rating = widened;
        save(tx, team_id, &s, now)?;
        changed += 1;
    }
    Ok(changed)
}

/// Drop the rating events of these series (their results were invalidated). Call `recompute` afterwards.
pub fn forget_series(tx: &Transaction, series_ids: &[String]) -> ApiResult<usize> {
    let mut st =
        tx.prepare_cached("DELETE FROM rating_events WHERE kind = 'game' AND series_id = ?1")?;
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
            e.outcome.as_deref().and_then(parse_outcome),
        ) {
            ("game", Some(team_b), Some(outcome)) => {
                let (mut a, mut b) = (standing(&e.team_a), standing(team_b));
                let (ra, rb) = hook.update(&a.rating, &b.rating, outcome);
                a.played(ra, outcome, true, e.at);
                b.played(rb, outcome, false, e.at);
                teams.insert(e.team_a.clone(), a);
                teams.insert(team_b.clone(), b);
            }
            ("submission", _, _) => {
                let mut s = standing(&e.team_a);
                s.rating = hook.on_new_submission(&s.rating);
                teams.insert(e.team_a.clone(), s);
            }
            ("balance", _, _) => {
                if let Some(factor) = e.outcome.as_deref().and_then(|f| f.parse().ok()) {
                    let mut s = standing(&e.team_a);
                    s.rating = hook.on_balance_patch(&s.rating, factor);
                    teams.insert(e.team_a.clone(), s);
                }
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
    pub submission: String,
    pub standing: Standing,
}

/// Teams with a live submission and their ratings, best displayed rating first.
pub fn ladder_rows(tx: &Transaction, hook: &dyn RatingHook) -> ApiResult<Vec<LadderRow>> {
    let mut st = tx.prepare(
        "SELECT t.id, t.name, s.id FROM teams t JOIN submissions s ON s.team_id = t.id AND s.live = 1",
    )?;
    let teams: Vec<(String, String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let mut rows = Vec::with_capacity(teams.len());
    for (team_id, name, submission) in teams {
        let standing = load(tx, hook, &team_id)?;
        rows.push(LadderRow {
            team_id,
            name,
            submission,
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

/// Write the ladder as it is now. `rating` is the displayed number, rounded; `provisional` says whether it
/// is still too uncertain to mean much (clients show "provisional" in its place). `mu`/`sigma` are the raw
/// estimate of the system named in `system`.
pub fn write_snapshot(
    tx: &Transaction,
    hook: &dyn RatingHook,
    now: i64,
    kept: i64,
) -> ApiResult<()> {
    let teams: Vec<serde_json::Value> = ladder_rows(tx, hook)?
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let (r, wld) = (&row.standing.rating, row.standing.wld);
            json!({"rank": i + 1, "team_id": row.team_id, "name": row.name,
                   "rating": hook.display(r).round() as i64, "provisional": hook.is_provisional(r),
                   "mu": r.mu, "sigma": r.sigma, "games": r.games,
                   "wins": wld.0, "losses": wld.1, "draws": wld.2, "submission_id": row.submission})
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
