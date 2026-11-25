//! The additions to lease protocol v1 (docs/JOBS.md): `release`, `fail` with `code: "timeout"`,
//! `lease_secs`, `runner_blobs`, and the `outcome` / `wall_ms` of a result.

mod common;

use axum::http::StatusCode;
use common::{fake_result, Harness, MockWorker};
use serde_json::{json, Value};

fn id(v: &Value) -> String {
    v["job_id"].as_str().unwrap().to_string()
}

async fn job_state(h: &Harness, job_id: &str) -> Value {
    h.admin("GET", &format!("/v1/admin/jobs/{job_id}"), Value::Null)
        .await
}

#[tokio::test]
async fn release_gives_the_job_back_without_spending_an_attempt() {
    let h = Harness::new(|c| c.retry_grace_ms = 60_000);
    h.set_patch().await;
    let job_id = id(&h.enqueue("ranked", 1).await);
    let (old, new) = (
        MockWorker::register(&h, "old-patch", false).await,
        MockWorker::register(&h, "new-patch", false).await,
    );
    // Many more releases than there are attempts: none of them counts.
    for _ in 0..5 {
        let job = old.lease_one(&h).await;
        assert_eq!(
            old.release(&h, &job, "patch mismatch").await.0,
            StatusCode::OK
        );
        assert_eq!(
            old.release(&h, &job, "patch mismatch").await.0,
            StatusCode::OK,
            "release is idempotent for the same lease token"
        );
        let state = job_state(&h, &job_id).await;
        assert_eq!(
            (state["state"].as_str(), state["attempts"].as_i64()),
            (Some("queued"), Some(0))
        );
        assert!(state["error"]
            .as_str()
            .unwrap()
            .ends_with(": patch mismatch"));
        // The releasing worker is not offered the job again for the retry grace period...
        assert!(old.lease(&h, 1, 0).await.is_empty());
        // ...so let it have it back for the next round.
        h.app
            .db
            .tx(|tx| {
                Ok(tx.execute(
                    "UPDATE jobs SET queued_at = queued_at - 60000 WHERE id = ?1",
                    [&job_id],
                )?)
            })
            .unwrap();
    }
    // Another worker takes it and finishes it on its first counted attempt.
    let job = new.lease_one(&h).await;
    assert_eq!(new.complete(&h, &job).await.0, StatusCode::OK);
    assert_eq!(job_state(&h, &job_id).await["attempts"], 1);
    // A release on a lease that is gone is a 409, like every other call.
    assert_eq!(old.release(&h, &job, "late").await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_match_that_times_out_twice_is_dead_lettered_not_retried_three_times() {
    let h = Harness::new(|_| {});
    h.set_patch().await;
    let job_id = id(&h.enqueue("ranked", 1).await);
    let (a, b) = (
        MockWorker::register(&h, "a", false).await,
        MockWorker::register(&h, "b", false).await,
    );
    let job = a.lease_one(&h).await;
    assert_eq!(a.time_out(&h, &job).await.0, StatusCode::OK);
    let state = job_state(&h, &job_id).await;
    assert_eq!(
        (state["state"].as_str(), state["timeouts"].as_i64()),
        (Some("queued"), Some(1)),
        "one timeout may be an overloaded machine: retried"
    );
    let job = b.lease_one(&h).await;
    assert_eq!(b.time_out(&h, &job).await.0, StatusCode::OK);
    let state = job_state(&h, &job_id).await;
    assert_eq!(
        (
            state["state"].as_str(),
            state["attempts"].as_i64(),
            state["timeouts"].as_i64()
        ),
        (Some("dead"), Some(2), Some(2))
    );
    // A human requeue starts the count again.
    h.admin(
        "POST",
        &format!("/v1/admin/jobs/{job_id}/requeue"),
        Value::Null,
    )
    .await;
    assert_eq!(job_state(&h, &job_id).await["timeouts"], 0);
    // Other failures do not count as timeouts.
    let job = a.lease_one(&h).await;
    assert_eq!(a.fail(&h, &job, true).await.0, StatusCode::OK);
    assert_eq!(job_state(&h, &job_id).await["state"], "queued");
}

#[tokio::test]
async fn jobs_and_heartbeats_carry_the_lease_length_and_the_patch_runners() {
    let h = Harness::new(|c| c.lease_ms = 20_000);
    let runner = h.blob(b"pretend this is an ibc binary").await;
    let patch =
        json!({"engine": "0.1.0", "ruleset_hash": "00112233aabbccdd", "wasmtime": "51.0.0"});
    let mut with_runner = patch.clone();
    with_runner["runner_blobs"] = json!({"linux-x86_64": runner});
    h.admin("POST", "/v1/admin/patch", with_runner).await;
    // Setting the patch again without runners keeps them; an unknown blob is refused.
    h.admin("POST", "/v1/admin/patch", patch.clone()).await;
    let mut bad = patch;
    bad["runner_blobs"] = json!({"linux-x86_64": "0".repeat(64)});
    assert_eq!(
        h.call("POST", "/v1/admin/patch", Some(common::ADMIN), bad)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );

    h.enqueue("ranked", 1).await;
    let w = MockWorker::register(&h, "w", false).await;
    let job = w.lease_one(&h).await;
    assert_eq!(job["lease_secs"], 20.0);
    assert_eq!(job["runner_blobs"], json!({"linux-x86_64": runner}));
    let (status, beat) = w.heartbeat(&h, &job).await;
    assert_eq!(
        (status, &beat["lease_secs"]),
        (StatusCode::OK, &json!(20.0))
    );

    // A job of another patch carries no runner blobs.
    h.admin(
        "POST",
        "/v1/admin/jobs",
        json!({"patch": {"engine": "0.0.9", "ruleset_hash": "ff", "wasmtime": "1"},
               "bot_a": job["bot_a"], "bot_b": job["bot_b"], "map": job["map"], "seed": 2}),
    )
    .await;
    let other = w.lease_one(&h).await;
    assert_eq!(other["patch"]["engine"], "0.0.9");
    assert!(other.get("runner_blobs").is_none());
}

#[tokio::test]
async fn results_must_state_a_consistent_outcome_and_unfinished_games_are_not_rated() {
    let h = Harness::new(|_| {});
    h.league(&["alpha", "beta"]).await;
    let batch = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [{"a": "alpha", "b": "beta", "games": 2}]}),
        )
        .await;
    let w = MockWorker::register(&h, "w", true).await;
    let jobs = w.lease(&h, 2, 0).await;
    let post = |job: &Value, result: Value| {
        let body = json!({"lease_token": job["lease_token"], "result": result});
        let path = format!("/v1/jobs/{}/complete", id(job));
        let token = w.token.clone();
        let h = &h;
        async move { h.call("POST", &path, Some(&token), body).await }
    };
    let (replay, mut result) = fake_result(&jobs[0], false);
    h.blob(&replay).await;
    for (outcome, winner) in [
        (json!("b"), json!(0)),
        (json!("draw"), json!(1)),
        (json!("won"), json!(0)),
        (Value::Null, json!(0)),
    ] {
        result["outcome"] = outcome.clone();
        result["winner"] = winner;
        let (status, err) = post(&jobs[0], result.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{outcome}: {err}");
    }
    result["wall_ms"] = json!(-3);
    assert_eq!(post(&jobs[0], result).await.0, StatusCode::BAD_REQUEST);

    // An old worker without `outcome`: the summary's is used. An unfinished game (winner null too) is
    // stored and shown but not rated, and not counted as a draw.
    let (replay, mut old) = fake_result(&jobs[0], false);
    old.as_object_mut().unwrap().remove("outcome");
    old.as_object_mut().unwrap().remove("wall_ms");
    h.blob(&replay).await;
    assert_eq!(post(&jobs[0], old).await.0, StatusCode::OK);
    let (replay, mut unfinished) = fake_result(&jobs[1], false);
    unfinished["winner"] = Value::Null;
    unfinished["outcome"] = json!("unfinished");
    unfinished["summary"]["outcome"] = json!("unfinished");
    h.blob(&replay).await;
    assert_eq!(post(&jobs[1], unfinished).await.0, StatusCode::OK);

    let series = h
        .ok(
            "GET",
            &format!("/v1/series/{}", batch["series"][0].as_str().unwrap()),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(
        (
            series["rated"].as_bool(),
            series["wins_a"].as_i64().unwrap() + series["wins_b"].as_i64().unwrap(),
            series["draws"].as_i64()
        ),
        (Some(true), 1, Some(0))
    );
    assert!(series["note"]
        .as_str()
        .unwrap()
        .contains("1 game(s) ended unfinished"));
    let outcomes: Vec<&str> = series["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["outcome"].as_str().unwrap())
        .collect();
    assert!(outcomes.contains(&"unfinished"), "{outcomes:?}");
    assert_eq!(h.standing("alpha").await["games"], 1);
}
