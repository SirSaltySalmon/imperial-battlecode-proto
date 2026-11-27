//! Pinned submissions, the live-submission switch, `ibc-rating` behind the coordinator (systems, ladder,
//! matchmaker, budget) and rating rollback on quarantine.

mod common;

use axum::http::StatusCode;
use common::{bot_wasm, Harness, MockWorker, ADMIN};
use ibc_coordinator::util::sha256_hex;
use serde_json::{json, Value};

/// The team whose module hash is smaller wins every fake match.
fn stronger<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if sha256_hex(&bot_wasm(a)) < sha256_hex(&bot_wasm(b)) {
        (a, b)
    } else {
        (b, a)
    }
}

/// The organisers' view of a series (with both pinned submissions).
async fn series(h: &Harness, id: &Value) -> Value {
    h.ok(
        "GET",
        &format!("/v1/series/{}", id.as_str().unwrap()),
        Some(ADMIN),
        Value::Null,
    )
    .await
}

/// The team's live submission id.
async fn live(h: &Harness, token: &str) -> Value {
    h.ok("GET", "/v1/team", Some(token), Value::Null).await["live_submission"].clone()
}

async fn upload(h: &Harness, token: &str, tag: &str, query: &str) -> Value {
    let (status, out) = h
        .raw(
            "POST",
            &format!("/v1/submissions{query}"),
            Some(token),
            bot_wasm(tag),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&out)
    );
    serde_json::from_slice(&out).unwrap()
}

#[tokio::test]
async fn a_series_plays_and_is_rated_with_the_bots_live_when_it_was_created() {
    let h = Harness::new(|_| {});
    let teams = h.league(&["alpha", "beta"]).await;
    let batch = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [{"a": "alpha", "b": "beta", "games": 2}]}),
        )
        .await;
    let before = series(&h, &batch["series"][0]).await;
    let pinned = before["team_a"]["sha256"].as_str().unwrap().to_string();
    assert_eq!(pinned, sha256_hex(&bot_wasm("alpha")));

    // Alpha uploads a new bot (live at once) while the series is still being played.
    let v2 = upload(&h, &teams[0].1, "alpha v2", "").await;
    assert_eq!(v2["live"], true);
    let w = MockWorker::register(&h, "w", true).await;
    let jobs = w.lease(&h, 8, 0).await;
    assert_eq!(jobs.len(), 2);
    for job in &jobs {
        let bots = [&job["bot_a"]["sha256"], &job["bot_b"]["sha256"]];
        assert!(
            bots.contains(&&json!(pinned)),
            "the series plays the pinned bot"
        );
        assert!(!bots.contains(&&v2["sha256"]));
        assert_eq!(w.complete(&h, job).await.0, StatusCode::OK);
    }
    let after = series(&h, &batch["series"][0]).await;
    assert_eq!(
        (
            after["state"].as_str(),
            after["rated"].as_bool(),
            &after["note"]
        ),
        (Some("done"), Some(true), &Value::Null),
        "always rated, whatever was uploaded since"
    );
    assert_eq!(after["team_a"]["sha256"], pinned.as_str());
    assert_eq!(h.standing("alpha").await["games"], 2);

    // The next series uses the new bot.
    let next = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [["alpha", "beta"]]}),
        )
        .await;
    assert_eq!(
        series(&h, &next["series"][0]).await["team_a"]["sha256"],
        v2["sha256"]
    );
}

#[tokio::test]
async fn a_team_chooses_its_live_submission_and_a_change_widens_its_uncertainty() {
    let h = Harness::new(|c| c.rating_system = "elo".into());
    let teams = h.league(&["alpha", "beta"]).await;
    let (alpha, beta) = (&teams[0].1, &teams[1].1);
    let v1 = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await["live_submission"].clone();

    // Twelve games: Elo's provisional window (ten games) closes.
    h.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": [{"a": "alpha", "b": "beta", "games": 12}]}),
    )
    .await;
    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(w.drain(&h).await, 12);
    let settled = h.rating("alpha").rating;
    assert_eq!(settled.sigma, 0.0);
    assert!(!h.app.rating.is_provisional(&settled));

    // Uploaded but not live: nothing changes.
    let v2 = upload(&h, alpha, "alpha v2", "?live=false").await;
    assert_eq!(v2["live"], false);
    assert_eq!(
        h.ok("GET", "/v1/team", Some(alpha), Value::Null).await["live_submission"],
        v1
    );
    assert_eq!(h.rating("alpha").rating.sigma, 0.0);

    // Selected: live, and the rating is provisional again (Elo reopens half its window).
    let path = format!(
        "/v1/submissions/{}/live",
        v2["submission_id"].as_str().unwrap()
    );
    let out = h.ok("POST", &path, Some(alpha), Value::Null).await;
    assert_eq!(out["changed"], true);
    assert_eq!(live(&h, alpha).await, v2["submission_id"]);
    let switched = h.rating("alpha").rating;
    assert_eq!(switched.sigma, 0.5);
    assert!(h.app.rating.is_provisional(&switched));
    assert_eq!(switched.mu, settled.mu, "the estimate is kept");
    // Selecting it again changes nothing; the organisers can switch a team back.
    assert_eq!(
        h.ok("POST", &path, Some(alpha), Value::Null).await["changed"],
        false
    );
    let back = format!("/v1/submissions/{}/live", v1.as_str().unwrap());
    assert_eq!(
        h.ok("POST", &back, Some(ADMIN), Value::Null).await["changed"],
        true
    );
    assert_eq!(live(&h, alpha).await, v1);

    // Someone else's submission, an unknown one, a rejected one.
    assert_eq!(
        h.call("POST", &path, Some(beta), Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.call(
            "POST",
            "/v1/submissions/subnope/live",
            Some(ADMIN),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let bad = wat::parse_str(r#"(module (memory (export "memory") 1))"#).unwrap();
    let (status, err) = h.raw("POST", "/v1/submissions", Some(alpha), bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err: Value = serde_json::from_slice(&err).unwrap();
    let rejected = err["message"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    let (status, err) = h
        .call(
            "POST",
            &format!("/v1/submissions/{rejected}/live"),
            Some(alpha),
            Value::Null,
        )
        .await;
    assert_eq!(
        (status, err["error"].as_str()),
        (StatusCode::CONFLICT, Some("not_validated"))
    );
}

#[tokio::test]
async fn the_matchmaker_pairs_free_teams_within_the_budget() {
    let h = Harness::new(|c| {
        c.ranked_games = 2;
        c.ranked_series_budget = 16;
    });
    // Nothing to schedule without teams.
    assert!(h.app.ranked_round(Some(8), 2).unwrap().is_empty());
    h.league(&["a", "b", "c", "d", "e"]).await;

    let first = h.app.ranked_round(Some(8), 2).unwrap();
    assert_eq!(first.len(), 2, "five teams: two pairs, one sits out");
    assert!(
        h.app.ranked_round(Some(8), 2).unwrap().is_empty(),
        "four teams are busy, one alone cannot be paired"
    );
    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(w.drain(&h).await, 4);
    let batch = h
        .admin("POST", "/v1/admin/ranked-batch", json!({"budget": 1}))
        .await;
    assert_eq!(
        batch["series"].as_array().unwrap().len(),
        1,
        "the budget caps a round"
    );
    while w.drain(&h).await > 0 {}

    // Over many rounds every team plays, and none past its cap plus the overflow (all in one window).
    for _ in 0..12 {
        h.app.ranked_round(Some(8), 2).unwrap();
        w.drain(&h).await;
    }
    let ladder = h.ok("GET", "/v1/ladder", None, Value::Null).await;
    assert!(ladder
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["games"].as_i64().unwrap() > 0));
    let (_, list) = h
        .call("GET", "/v1/matches?limit=500", None, Value::Null)
        .await;
    let mut series = std::collections::BTreeSet::new();
    for m in list["matches"].as_array().unwrap() {
        for side in ["bot_a", "bot_b"] {
            series.insert((
                m[side]["name"].as_str().unwrap().to_string(),
                m["series_id"].as_str().unwrap().to_string(),
            ));
        }
    }
    let mut series_per_team = std::collections::BTreeMap::new();
    for (team, _) in series {
        *series_per_team.entry(team).or_insert(0) += 1;
    }
    let limit = h.app.cfg.ranked_cap_series + h.app.cfg.ranked_cap_overflow;
    assert!(
        series_per_team.values().all(|&n| n <= limit),
        "{series_per_team:?}"
    );
}

#[tokio::test]
async fn two_teams_rematch_until_their_cap() {
    let h = Harness::new(|c| c.ranked_games = 2);
    h.league(&["a", "b"]).await;
    let w = MockWorker::register(&h, "w", true).await;
    let mut created = 0;
    for _ in 0..20 {
        created += h.app.ranked_round(Some(8), 2).unwrap().len();
        w.drain(&h).await;
        h.app.sweep().unwrap();
    }
    // Both start under the cap and reach it together, so neither may start a sixth series.
    assert_eq!(created as i64, h.app.cfg.ranked_cap_series);
}

#[tokio::test]
async fn the_fill_up_tops_the_queue_up_to_the_backlog() {
    let h = Harness::new(|c| {
        c.ranked_games = 2;
        c.ranked_series_budget = 5000;
    });
    h.league(&["a", "b", "c", "d", "e"]).await;
    let fill = |h: &Harness| {
        h.app
            .db
            .tx(|tx| {
                h.app
                    .ranked_fill_budget(tx, 2, ibc_coordinator::util::now_ms())
            })
            .unwrap()
    };
    assert_eq!(fill(&h), 0, "no workers: nothing to fill");
    // One 4-core worker, nothing measured (10 s a game): 120 s of work is 48 games, 24 series.
    let _w = MockWorker::register(&h, "w", true).await;
    assert_eq!(fill(&h), 24);
    let created = h.app.ranked_fill_round(2).unwrap();
    assert_eq!(created.len(), 2, "five teams: two pairs, one sits out");
    assert_eq!(fill(&h), 22, "the four queued games leave less room");

    let off = Harness::new(|c| c.ranked_backlog_ms = 0);
    off.league(&["a", "b"]).await;
    let _w = MockWorker::register(&off, "w", true).await;
    assert_eq!(
        off.app
            .db
            .tx(|tx| off
                .app
                .ranked_fill_budget(tx, 2, ibc_coordinator::util::now_ms()))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn the_ranked_budget_follows_the_live_workers() {
    let h = Harness::new(|c| {
        c.ranked_games = 2;
        c.ranked_series_budget = 50_000;
        c.ranked_interval_ms = 600_000;
    });
    h.league(&["a", "b"]).await;
    let budget = |h: &Harness| {
        h.app
            .db
            .tx(|tx| h.app.ranked_budget(tx, 2, ibc_coordinator::util::now_ms()))
            .unwrap()
    };
    assert_eq!(budget(&h), 0, "no workers: the ladder pauses");
    // One 4-core worker, nothing measured: the prior of 2 core-seconds per game.
    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(budget(&h), 4 * 3600 * 7 / 10 / 6 / (2 * 2));
    // Measured: 40 ms per game, below the 0.05 s floor.
    h.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": [["a", "b"]]}),
    )
    .await;
    w.drain(&h).await;
    assert_eq!(budget(&h), (4.0 * 3600.0 * 0.7 / 6.0 / (2.0 * 0.05)) as u32);
}

#[tokio::test]
async fn a_quarantine_rolls_back_ratings_and_the_series_is_rated_again_from_the_re_runs() {
    let h = Harness::new(|_| {});
    h.league(&["alpha", "beta", "gamma"]).await;
    let (winner, loser) = stronger("alpha", "beta");
    let batch = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [{"a": "alpha", "b": "beta", "games": 2}]}),
        )
        .await;
    // An untrusted cheat plays both games (nothing is spot-verified here) and the series is rated.
    let mut cheat = MockWorker::register(&h, "cheat", false).await;
    cheat.cheat = true;
    assert_eq!(cheat.drain(&h).await, 2);
    assert_eq!(series(&h, &batch["series"][0]).await["rated"], true);
    assert_eq!(h.standing(loser).await["wins"], 2, "the tampered result");
    // An honest series between other teams, rated after the cheat's.
    h.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": [{"a": "gamma", "b": winner, "games": 2}]}),
    )
    .await;
    let honest = MockWorker::register(&h, "honest", true).await;
    assert_eq!(honest.drain(&h).await, 2);
    let honest_only = h.standing("gamma").await;

    // Caught: the series is reopened and its games leave the ratings.
    let out = h
        .admin(
            "POST",
            &format!("/v1/admin/workers/{}/quarantine", cheat.id),
            json!({"quarantined": true}),
        )
        .await;
    assert_eq!(
        (
            out["requeued_results"].as_i64(),
            out["reopened_series"].as_i64()
        ),
        (Some(2), Some(1))
    );
    let reopened = series(&h, &batch["series"][0]).await;
    assert_eq!(
        (reopened["state"].as_str(), reopened["rated"].as_bool()),
        (Some("running"), Some(false))
    );
    assert!(reopened["note"].as_str().unwrap().starts_with("reopened"));
    assert_eq!(h.standing(loser).await["games"], 0);
    let rolled_back = h.standing(winner).await;
    assert_eq!(rolled_back["games"], 2, "only the honest series is left");
    // Gamma never met the cheat: its record is unchanged.
    assert_eq!(h.standing("gamma").await["wins"], honest_only["wins"]);

    // The re-runs rate the series again, this time with the true results.
    assert_eq!(honest.drain(&h).await, 2);
    let rerated = series(&h, &batch["series"][0]).await;
    assert_eq!(
        (rerated["state"].as_str(), rerated["rated"].as_bool()),
        (Some("done"), Some(true))
    );
    assert!(rerated["note"].as_str().unwrap().contains("re-rated"));
    let (w, l) = (h.standing(winner).await, h.standing(loser).await);
    assert_eq!(
        (w["games"].as_i64(), w["wins"].as_i64()),
        (Some(4), Some(rolled_back["wins"].as_i64().unwrap() + 2))
    );
    assert_eq!(
        (
            l["games"].as_i64(),
            l["wins"].as_i64(),
            l["losses"].as_i64()
        ),
        (Some(2), Some(0), Some(2))
    );
}

#[tokio::test]
async fn switching_the_rating_system_recomputes_every_rating_from_the_ledger() {
    let mut h = Harness::new(|_| {});
    h.league(&["alpha", "beta"]).await;
    let (winner, _) = stronger("alpha", "beta");
    h.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": [{"a": "alpha", "b": "beta", "games": 4}]}),
    )
    .await;
    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(w.drain(&h).await, 4);
    let wl = h.rating(winner);
    assert_eq!(h.app.rating.name(), "weng-lin-bt");

    h.reopen(|c| c.rating_system = "elo".into());
    assert_eq!(h.app.rating.name(), "elo");
    let ladder = h.ok("GET", "/v1/ladder", None, Value::Null).await;
    let teams = ladder.as_array().unwrap();
    assert_eq!(teams[0]["team_name"], winner);
    assert_eq!(
        (teams[0]["games"].as_i64(), teams[0]["wins"].as_i64()),
        (Some(4), Some(4))
    );
    let sum = h.rating("alpha").rating.mu + h.rating("beta").rating.mu;
    assert!(
        (sum - 3000.0).abs() < 1e-9,
        "Elo conserves the rating sum: {sum}"
    );

    // And back: the same Weng-Lin numbers as before, bit for bit.
    h.reopen(|c| c.rating_system = "weng-lin".into());
    assert_eq!(h.rating(winner).rating, wl.rating);
    let mut cfg = h.app.cfg.clone();
    cfg.rating_system = "glicko".into();
    let nowhere = std::env::temp_dir().join("ibc-coordinator-test-never-created");
    assert!(ibc_coordinator::App::open(cfg, &nowhere).is_err());
    assert!(!nowhere.exists());
}
