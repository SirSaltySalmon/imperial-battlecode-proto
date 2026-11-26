//! What the tournament endpoints show: the summary list, one tournament with its bracket, hidden maps.

use super::{
    bracket_state, format_name, from_json, load, load_entrants, load_matches, ranked_paused,
};
use super::{Entrant, MatchRow, Tournament};
use crate::error::ApiResult;
use crate::util::{opt_rfc3339, rfc3339};
use ibc_rating::bracket::{Format, MatchState, Seat, Stage};
use rusqlite::Transaction;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Maps that belong to a tournament whose maps are not revealed and are not in the ranked pool.
pub fn hidden_maps(tx: &Transaction) -> ApiResult<BTreeSet<String>> {
    let mut st = tx.prepare_cached("SELECT maps FROM tournaments WHERE revealed_at IS NULL")?;
    let lists: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut hidden = BTreeSet::new();
    for list in lists {
        hidden.extend(from_json::<Vec<String>>(&list)?);
    }
    let mut st = tx.prepare_cached("SELECT name FROM maps WHERE in_pool = 1")?;
    for name in st.query_map([], |r| r.get::<_, String>(0))? {
        hidden.remove(&name?);
    }
    Ok(hidden)
}

/// All tournaments, newest first.
pub fn list(tx: &Transaction) -> ApiResult<Vec<Value>> {
    let ids: Vec<String> = {
        let mut st =
            tx.prepare("SELECT id FROM tournaments ORDER BY created_at DESC, id LIMIT 200")?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    ids.iter()
        .map(|id| {
            let t = load(tx, id)?;
            let entrants: i64 = tx.query_row(
                "SELECT COUNT(*) FROM tournament_entrants WHERE tournament_id = ?1",
                [id],
                |r| r.get(0),
            )?;
            let mut v = summary(&t);
            v["entrants"] = json!(entrants);
            Ok(v)
        })
        .collect()
}

fn summary(t: &Tournament) -> Value {
    let [created, locked, started, finished, revealed] = t.times;
    json!({
        "id": t.id, "name": t.name, "format": format_name(t.format), "status": t.status,
        "field": t.field, "winner": t.winner, "maps_revealed": revealed.is_some(),
        "created_at": opt_rfc3339(created), "locked_at": opt_rfc3339(locked), "started_at": opt_rfc3339(started),
        "finished_at": opt_rfc3339(finished), "revealed_at": opt_rfc3339(revealed),
    })
}

/// One tournament with its entrants and bracket. `maps` is `null` until they are revealed, unless `admin`
/// (who also sees the locked submission of each entrant).
pub fn view(tx: &Transaction, id: &str, admin: bool, now: i64) -> ApiResult<Value> {
    let t = load(tx, id)?;
    let entrants = load_entrants(tx, id)?;
    let rows = load_matches(tx, id)?;
    let mut v = summary(&t);
    v["maps"] = if admin || t.times[4].is_some() {
        json!(t.maps)
    } else {
        Value::Null
    };
    v["map_count"] = json!(t.maps.len());
    v["excluded"] = t.excluded.clone();
    v["entrants"] = entrants
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut row = json!({"seed": i + 1, "team_id": e.team_id, "name": e.name,
                                 "rating": e.rating, "provisional": e.provisional});
            if admin {
                row["submission_id"] = json!(e.submission_id);
            }
            row
        })
        .collect();
    v["rounds"] = if rows.is_empty() {
        json!([])
    } else {
        rounds_view(&t, &entrants, &rows)
    };
    v["ranked_paused_until"] = json!(ranked_paused(tx, now)?.map(rfc3339));
    Ok(v)
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Winners => "winners",
        Stage::Losers => "losers",
        Stage::Final => "final",
    }
}

/// The bracket grouped by (stage, round), in play order within each stage.
fn rounds_view(t: &Tournament, entrants: &[Entrant], rows: &[MatchRow]) -> Value {
    let (shape, states) = bracket_state(t, entrants.len(), rows);
    let mut rounds: Vec<Value> = Vec::new();
    let mut current: Option<(Stage, u32)> = None;
    for (idx, (m, state)) in shape.iter().zip(&states).enumerate() {
        if current != Some((m.stage, m.round)) {
            current = Some((m.stage, m.round));
            let name = match m.stage {
                Stage::Final => "Grand final".to_string(),
                Stage::Winners if t.format == Format::Single => format!("Round {}", m.round),
                Stage::Winners => format!("Winners round {}", m.round),
                Stage::Losers => format!("Losers round {}", m.round),
            };
            rounds.push(json!({"name": name, "stage": stage_name(m.stage), "round": m.round, "pairings": []}));
        }
        let pairing = pairing_view(idx, state, entrants, &rows[idx]);
        if let Some(Value::Array(list)) = rounds.last_mut().map(|r| &mut r["pairings"]) {
            list.push(pairing);
        }
    }
    Value::Array(rounds)
}

fn pairing_view(idx: usize, state: &MatchState, entrants: &[Entrant], row: &MatchRow) -> Value {
    let team = |s: Seat| match s {
        Seat::Team(i) => json!(entrants[i as usize].team_id),
        _ => Value::Null,
    };
    let name = |s: Seat| match s {
        Seat::Team(i) => json!(entrants[i as usize].name),
        _ => Value::Null,
    };
    let seed = |s: Seat| match s {
        Seat::Team(i) => json!(i + 1),
        _ => Value::Null,
    };
    let [a, b] = state.seats;
    let status = match (a, b, state.winner) {
        (Seat::Pending, _, _) | (_, Seat::Pending, _) => "waiting",
        (Seat::Empty, Seat::Empty, _) => "empty",
        (Seat::Empty, _, _) | (_, Seat::Empty, _) => "bye",
        (_, _, Seat::Team(_)) => "done",
        _ if row.series_id.is_some() => "running",
        _ => "waiting",
    };
    json!({
        "match": idx, "state": status,
        "a": team(a), "b": team(b), "a_name": name(a), "b_name": name(b), "seed_a": seed(a), "seed_b": seed(b),
        "series_id": row.series_id,
        "maps_won": row.map_halves.map(|h| [f64::from(h[0]) / 2.0, f64::from(h[1]) / 2.0]),
        "rounds_won": row.rounds,
        "winner": team(state.winner),
        "decided_by": row.decided_by,
    })
}
