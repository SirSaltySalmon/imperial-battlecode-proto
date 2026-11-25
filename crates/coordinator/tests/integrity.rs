//! Things a single bad or sloppy participant must not be able to do: kill a healthy job by being slow,
//! pass spot verification while lying about who won, get a won game dropped from rating, or grow the
//! rating ledger by toggling the live submission.

mod common;

use axum::http::StatusCode;
use common::{bot_wasm, fake_result, Harness, MockWorker};
use serde_json::{json, Value};

fn id(v: &Value) -> String {
    v["job_id"].as_str().unwrap().to_string()
}

async fn job_state(h: &Harness, job_id: &str) -> Value {
    h.admin("GET", &format!("/v1/admin/jobs/{job_id}"), Value::Null).await
}

/// Complete `job` as `w` with exactly this result object.
async fn post_result(h: &Harness, w: &MockWorker, job: &Value, result: Value) -> (StatusCode, Value) {
    let body = json!({"lease_token": job["lease_token"], "result": result});
    h.call("POST", &format!("/v1/jobs/{}/complete", id(job)), Some(&w.token), body).await
}

#[tokio::test]
async fn one_slow_worker_cannot_dead_letter_a_job_by_timing_out_twice() {
    // No retry grace, so the same worker is offered the job again at once.
    let h = Harness::new(|c| c.retry_grace_ms = 0);
    h.set_patch().await;
    let job_id = id(&h.enqueue("ranked", 1).await);
    let (slow, other) =
        (MockWorker::register(&h, "slow", false).await, MockWorker::register(&h, "other", false).await);

    let job = slow.lease_one(&h).await;
    assert_eq!(slow.time_out(&h, &job).await.0, StatusCode::OK);
    let job = slow.lease_one(&h).await;
    assert_eq!(slow.time_out(&h, &job).await.0, StatusCode::OK);
    let state = job_state(&h, &job_id).await;
    assert_eq!(
        (state["state"].as_str(), state["attempts"].as_i64(), state["timeouts"].as_i64()),
        (Some("queued"), Some(2), Some(1)),
        "the same machine timing out again costs an attempt but is not a second timeout"
    );

    // A second machine timing out is evidence about the match: now it is dead-lettered.
    let job = other.lease_one(&h).await;
    assert_eq!(other.time_out(&h, &job).await.0, StatusCode::OK);
    let state = job_state(&h, &job_id).await;
    assert_eq!((state["state"].as_str(), state["timeouts"].as_i64()), (Some("dead"), Some(2)));
}

/// Play one ranked game between alpha and beta. The untrusted worker uploads the true replay; with `lie`
/// it reports the other team as the winner. A trusted worker then re-runs it. Returns (wins_a, wins_b).
async fn verified_series(lie: bool) -> (i64, i64) {
    let h = Harness::new(|c| c.verify_fraction = 1.0);
    h.league(&["alpha", "beta"]).await;
    let batch = h
        .admin("POST", "/v1/admin/ranked-batch", json!({"pairings": [{"a": "alpha", "b": "beta", "games": 1}]}))
        .await;
    let path = format!("/v1/series/{}", batch["series"][0].as_str().unwrap());

    let rented = MockWorker::register(&h, "rented", false).await;
    let job = rented.lease_one(&h).await;
    let (replay, mut result) = fake_result(&job, false);
    h.blob(&replay).await;
    if lie {
        let flipped = 1 - result["winner"].as_i64().unwrap();
        let outcome = if flipped == 0 { "a" } else { "b" };
        result["winner"] = json!(flipped);
        result["outcome"] = json!(outcome);
        result["summary"]["outcome"] = json!(outcome);
        result["score"] = if flipped == 0 { json!([7, 3]) } else { json!([3, 7]) };
    }
    assert_eq!(post_result(&h, &rented, &job, result).await.0, StatusCode::OK);

    let trusted = MockWorker::register(&h, "trusted", true).await;
    while trusted.drain(&h).await > 0 {}
    let series = h.ok("GET", &path, None, Value::Null).await;
    assert_eq!((series["state"].as_str(), series["rated"].as_bool()), (Some("done"), Some(true)));
    if lie {
        // Same replay hash, so the worker is not caught; but its word is not what gets rated.
        assert_eq!(h.metric("ibc_verify_mismatch_total").await, 0.0);
    }
    (series["wins_a"].as_i64().unwrap(), series["wins_b"].as_i64().unwrap())
}

#[tokio::test]
async fn a_verified_game_is_rated_from_the_trusted_result_not_the_untrusted_workers_word() {
    let honest = verified_series(false).await;
    assert_eq!(honest.0 + honest.1, 1);
    assert_eq!(verified_series(true).await, honest, "a true replay with a false winner changed the series");
}

#[tokio::test]
async fn a_result_must_say_who_won_somewhere_and_consistently() {
    let h = Harness::new(|_| {});
    h.set_patch().await;
    h.enqueue("ranked", 1).await;
    let w = MockWorker::register(&h, "w", true).await;
    let job = w.lease_one(&h).await;
    let (replay, good) = fake_result(&job, false);
    h.blob(&replay).await;
    let other = if good["outcome"] == "a" { "b" } else { "a" };

    // No `outcome` and none in the summary: would be taken for an unfinished game and never rated.
    let mut none = good.clone();
    none.as_object_mut().unwrap().remove("outcome");
    none["summary"].as_object_mut().unwrap().remove("outcome");
    assert_eq!(post_result(&h, &w, &job, none).await.0, StatusCode::BAD_REQUEST);

    // No `outcome`, and the summary's disagrees with `winner`.
    let mut split = good.clone();
    split.as_object_mut().unwrap().remove("outcome");
    split["summary"]["outcome"] = json!(other);
    assert_eq!(post_result(&h, &w, &job, split).await.0, StatusCode::BAD_REQUEST);

    // An old worker (no `outcome`, a consistent summary) is still accepted.
    let mut old = good;
    old.as_object_mut().unwrap().remove("outcome");
    assert_eq!(post_result(&h, &w, &job, old).await.0, StatusCode::OK);
}

#[tokio::test]
async fn toggling_the_live_submission_does_not_grow_the_rating_ledger() {
    let h = Harness::new(|c| c.rating_system = "elo".into());
    let teams = h.league(&["alpha", "beta"]).await;
    let alpha = &teams[0].1;
    let v1 = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await["live_submission"].as_str().unwrap().to_string();
    let (status, out) = h.raw("POST", "/v1/submissions?live=false", Some(alpha), bot_wasm("alpha v2")).await;
    assert_eq!(status, StatusCode::CREATED);
    let v2 = serde_json::from_slice::<Value>(&out).unwrap()["submission_id"].as_str().unwrap().to_string();

    // Settle alpha's rating first, so that the first change really widens it.
    h.admin("POST", "/v1/admin/ranked-batch", json!({"pairings": [{"a": "alpha", "b": "beta", "games": 12}]})).await;
    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(w.drain(&h).await, 12);

    let events = || {
        h.app
            .db
            .tx(|tx| {
                Ok(tx.query_row("SELECT COUNT(*) FROM rating_events WHERE kind = 'submission'", [], |r| {
                    r.get::<_, i64>(0)
                })?)
            })
            .unwrap()
    };
    assert_eq!(events(), 0);
    for round in 0..6 {
        let pick = if round % 2 == 0 { &v2 } else { &v1 };
        let out = h.ok("POST", &format!("/v1/submissions/{pick}/live"), Some(alpha), Value::Null).await;
        assert_eq!(out["changed"], true, "the live submission does change each time");
    }
    assert_eq!(events(), 1, "only the change that widened the rating is recorded");
    assert_eq!(h.standing("alpha").await["sigma"], 0.5);
}
