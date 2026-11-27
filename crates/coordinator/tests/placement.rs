//! Placement: a team with a new live submission plays its first ranked series back to back, and the ladder
//! shows it as placing (rating, no tier) until they are done.

mod common;

use axum::http::StatusCode;
use common::{bot_wasm, Harness, MockWorker};
use serde_json::Value;
use std::collections::BTreeMap;

/// Placement series started per team (by team name).
fn placements(h: &Harness) -> BTreeMap<String, i64> {
    h.app
        .db
        .tx(|tx| {
            let mut st = tx.prepare(
                "SELECT t.name, COUNT(*) FROM series s JOIN teams t ON t.id = s.placement_for GROUP BY t.name",
            )?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap()
}

/// Play every queued job, close the finished series and run the placement task until it has nothing
/// left to start. Returns how many placement passes created something.
async fn place_all(h: &Harness, w: &MockWorker) -> usize {
    let mut passes = 0;
    loop {
        let created = h.app.placement_round().unwrap();
        if created.is_empty() {
            return passes;
        }
        passes += 1;
        assert!(
            h.app.placement_round().unwrap().is_empty(),
            "one placement series at a time per team"
        );
        w.drain(h).await;
        h.app.sweep().unwrap();
    }
}

#[tokio::test]
async fn new_submissions_place_back_to_back_then_wait_for_rounds() {
    // Every series here falls in one cap window; the cap has its own test in `ratings.rs`.
    let h = Harness::new(|c| c.ranked_cap_series = 100);
    let names = ["alpha", "beta", "gamma", "delta", "epsilon"];
    let teams = h.league(&names).await;
    let w = MockWorker::register(&h, "w", true).await;
    for name in names {
        let row = h.standing(name).await;
        assert_eq!(
            (row["provisional"].as_bool(), row["rating"].as_i64()),
            (Some(true), Some(1500))
        );
    }

    assert!(place_all(&h, &w).await >= 2);
    let ladder = h.ok("GET", "/v1/ladder", None, Value::Null).await;
    for row in ladder.as_array().unwrap() {
        assert_eq!(row["provisional"], false, "{ladder}");
        assert!(
            row["tier"].is_string() && row["series"].as_i64() >= Some(3),
            "{ladder}"
        );
    }
    let before = placements(&h);
    assert!(before.values().all(|&n| (1..=3).contains(&n)), "{before:?}");

    // A new bot places again, alone: one series at a time until three have finished with it.
    let (status, _) = h
        .raw(
            "POST",
            "/v1/submissions",
            Some(&teams[0].1),
            bot_wasm("alpha v2"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let row = h.standing("alpha").await;
    assert_eq!(
        (row["provisional"].as_bool(), row["tier"].is_null()),
        (Some(true), true)
    );
    assert_eq!(place_all(&h, &w).await, 3);
    let after = placements(&h);
    assert_eq!(after["alpha"], before["alpha"] + 3);
    for name in &names[1..] {
        assert_eq!(after[*name], before[*name], "{name}");
    }
    assert_eq!(h.standing("alpha").await["provisional"], false);
}

#[tokio::test]
async fn placement_stops_at_the_daily_cap_and_can_be_turned_off() {
    let h = Harness::new(|c| c.placement_daily_series = 1);
    h.league(&["alpha", "beta"]).await;
    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(place_all(&h, &w).await, 1);
    assert_eq!(placements(&h).values().sum::<i64>(), 2);
    // Two series finished each, one short of placed: they wait for the scheduler's rounds now.
    assert_eq!(h.standing("alpha").await["provisional"], true);

    let off = Harness::new(|c| c.placement_series = 0);
    off.league(&["alpha", "beta"]).await;
    assert!(off.app.placement_round().unwrap().is_empty());
    let row = off.standing("alpha").await;
    assert_eq!(
        (row["provisional"].as_bool(), row["tier"].is_string()),
        (Some(false), true)
    );
}
